use super::*;
use crate::client_adapter::{ClientWorker, DocumentClient, PersistedView};
use lapis_desktop_client::{DocumentAction, DocumentCloseDisposition};

type ClosedGroup = (Entity<WindowGroup>, ClientSnapshot, Vec<HostId>);

type Decision = (
    DocumentId,
    DocumentCloseDisposition,
    lapis_document::Revision,
);

struct CloseRequest {
    group: Entity<WindowGroup>,
    worker: ClientWorker<DocumentClient>,
    view: PersistedView,
    snapshot: Option<std::sync::mpsc::Receiver<Result<ClientSnapshot, String>>>,
    host: HostId,
    document: Option<DocumentId>,
    placements: HashMap<DocumentId, HostId>,
    windows: Vec<HostId>,
    remote: bool,
}

struct ClosePlan {
    request: CloseRequest,
    decisions: Vec<Decision>,
    saves: Vec<DocumentId>,
}

impl DesktopRoot {
    /// Captures close targets and queues a snapshot after earlier worker operations.
    ///
    /// `document` targets one owned tab; otherwise the root targets its entire group
    /// and a child targets itself. Returns an error if closing, recovery is required,
    /// the document is not owned by the host, or the worker cannot accept the request.
    fn close_request(
        &self,
        host: HostId,
        document: Option<DocumentId>,
        cx: &gpui::App,
    ) -> Result<CloseRequest, String> {
        let group = self.group.read(cx);
        if group.closing {
            return Err("Windowの終了処理中です".into());
        }
        if group
            .members
            .values()
            .any(|member| member.editor.read(cx).needs_recovery())
        {
            return Err("未確定の入力を新しい文書へ退避・復旧してから閉じてください".into());
        }
        let owned = group.documents(host);
        if document.as_ref().is_some_and(|id| !owned.contains(id)) {
            return Err("このWindowには対象のタブがありません".into());
        }
        let windows = if document.is_some() {
            vec![]
        } else if host == ROOT_HOST {
            group.members.keys().copied().collect()
        } else {
            vec![host]
        };
        Ok(CloseRequest {
            group: self.group.clone(),
            worker: self.worker.clone(),
            view: self.persisted_view(cx),
            snapshot: Some(self.worker.try_request(|client| {
                // 先行入力が失敗していれば、UIへのエラー到着前でも終了を止める。
                client
                    .edit_chain
                    .apply(client.edit_chain.epoch(), || Ok(()))?;
                client.snapshot()
            })?),
            host,
            document,
            placements: group
                .layout
                .tabs
                .iter()
                .filter_map(|(tab, (owner, _))| match tab {
                    PanelTab::Document(id) => Some((id.clone(), *owner)),
                    _ => None,
                })
                .collect(),
            windows,
            remote: self.remote_path_scope(cx).is_some(),
        })
    }

    /// Starts the approval and save flow for one tab, a child window, or the root's entire group.
    /// Request failures become status messages without starting the close flow.
    pub(crate) fn close_hosts(
        &mut self,
        host: HostId,
        document: Option<DocumentId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.close_request(host, document, cx) {
            Ok(request) => self.start_close(vec![request], window, cx),
            Err(error) => self.set_status(error, cx),
        }
    }

    /// Collects all root groups for a shared approval and save flow before closing their windows.
    /// A request or window-access failure reports status and aborts before closing starts.
    pub(crate) fn close_application(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut requests = match self.close_request(ROOT_HOST, None, cx) {
            Ok(request) => vec![request],
            Err(error) => {
                self.set_status(error, cx);
                return;
            }
        };
        let own = cx.entity_id();
        let own_windows = self
            .group
            .read(cx)
            .members
            .values()
            .map(|member| member.handle)
            .collect::<Vec<_>>();
        for handle in cx.windows() {
            if own_windows.contains(&handle) {
                continue;
            }
            let result = handle.update(cx, |view, _, cx| {
                let entity = view.downcast::<DesktopRoot>().ok()?;
                if entity.entity_id() == own {
                    return None;
                }
                Some(entity.read(cx).close_request(ROOT_HOST, None, cx))
            });
            match result {
                Ok(Some(Ok(request))) => requests.push(request),
                Ok(Some(Err(error))) => {
                    self.set_status(error, cx);
                    return;
                }
                Err(error) => {
                    self.set_status(error.to_string(), cx);
                    return;
                }
                Ok(None) => {}
            }
        }
        self.start_close(requests, window, cx);
    }

    /// Disables group editors while collecting decisions, saving, validating, and closing attachments.
    ///
    /// Decisions and saves finish before any close; shared documents are prompted,
    /// saved, and discarded at most once. Cancellation or errors restore input and
    /// report status. Completed saves and attachment closes are not rolled back
    /// after a later failure. Native windows are removed only after every plan succeeds.
    fn start_close(
        &mut self,
        requests: Vec<CloseRequest>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for request in &requests {
            set_closing(&request.group, true, cx);
        }
        let handle = window.window_handle();
        let groups = requests
            .iter()
            .map(|request| request.group.clone())
            .collect::<Vec<_>>();
        cx.spawn(async move |this, cx| {
            let result: Result<Vec<ClosedGroup>, String> = async {
                let mut plans = Vec::new();
                let mut approvals = HashMap::new();
                let mut save_paths = HashMap::new();
                // 全グループの判断が揃うまで保存・破棄・closeを実行しない。
                for mut request in requests {
                    let snapshot = wait_client(request.snapshot.take().unwrap()).await?;
                    let mut decisions = Vec::new();
                    let mut saves = Vec::new();
                    for tab in snapshot.tabs.iter().filter(|tab| request.document.as_ref().map_or_else(
                        || request.host == ROOT_HOST || request.placements.get(&tab.id).copied().or_else(|| snapshot.document_owners.get(&tab.id).copied()).unwrap_or(ROOT_HOST) == request.host, |id| id == &tab.id))
                    {
                        let cache = snapshot.documents.iter().find(|cache| cache.id == tab.id).ok_or("文書の状態を取得できません")?;
                        let answer = if let Some((answer, revision)) = approvals.get(&tab.id) {
                            if *revision != cache.document.revision() { return Err("Windowの確認中に文書が変更されました".into()); }
                            *answer
                        } else if tab.dirty {
                            let title = format!("{} を閉じますか？", tab.display_name);
                            let receiver = this.update(cx, |_, cx| handle.update(cx, |_, window, cx| {
                                window.prompt(gpui::PromptLevel::Warning, &title,
                                    Some("未保存の変更を保存するか、破棄するかを選択してください。"),
                                    &[gpui::PromptButton::new("Save"), gpui::PromptButton::new("Discard"), gpui::PromptButton::cancel("Cancel")], cx)
                            })).map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
                            receiver.await.map_err(|e| e.to_string())?
                        } else { 0 };
                        approvals.insert(tab.id.clone(), (answer, cache.document.revision()));
                        if answer > 1 { return Err("終了をキャンセルしました".into()); }
                        if tab.dirty && answer == 0 && tab.path.is_none() && request.remote && !save_paths.contains_key(&tab.id) {
                            let root = snapshot.root.clone().ok_or("保存先のWorkspaceがありません")?;
                            let receiver = handle.update(cx, |_, window, cx| this.update(cx, |owner, cx| {
                                owner.request_close_path(window, cx)
                            })).map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
                            let input = wait_client(receiver).await?;
                            save_paths.insert(tab.id.clone(), (root, input));
                        }
                        if tab.dirty && answer == 0 { saves.push(tab.id.clone()); }
                        decisions.push((tab.id.clone(), if answer == 1 { DocumentCloseDisposition::DiscardChanges } else { DocumentCloseDisposition::PreserveChanges }, cache.document.revision()));
                    }
                    plans.push(ClosePlan { request, decisions, saves });
                }
                let mut saved = std::collections::HashSet::new();
                for plan in &plans {
                    let decisions = plan.decisions.clone();
                    let saves = plan.saves.iter().filter(|id| saved.insert((*id).clone())).cloned().collect::<Vec<_>>();
                    let save_paths = save_paths.clone();
                    let receiver = plan.request.worker.try_request(move |client| {
                        for (id, _, revision) in &decisions {
                            if client.editor.document_connection().attach(id).map_err(|e| e.to_string())?.document.revision() != *revision {
                                return Err("確認後に文書が変更されました".into());
                            }
                        }
                        for id in saves {
                            if !client.editor.select_document_for_operation(&id) { return Err("対象文書が閉じられました".into()); }
                            if let Some((root, input)) = save_paths.get(&id) {
                                let path = client.editor.workspace_path(root, input).map_err(|e| e.to_string())?;
                                client.editor.save_document_to(path).map_err(|e| e.to_string())?;
                            } else if client.editor.save_document().map_err(|e| e.to_string())? == DocumentAction::Cancelled {
                                return Err("保存をキャンセルしました".into());
                            }
                        }
                        client.editor.validate_document_closes(&decisions).map_err(|e| e.to_string())
                    })?;
                    wait_client(receiver).await?;
                }
                let mut results = Vec::new();
                let mut committed = std::collections::HashSet::new();
                for plan in plans {
                    let ClosePlan { request, mut decisions, .. } = plan;
                    let ids = decisions.iter().map(|(id, _, _)| id.clone()).collect::<Vec<_>>();
                    let saved_in_plan = ids.iter().any(|id| saved.contains(id));
                    let previous = committed.clone();
                    let view = request.view;
                    let receiver = request.worker.try_request(move |client| {
                        for (id, disposition, revision) in &mut decisions {
                            if previous.contains(id) {
                                // 別Rootと共有する正本の破棄は一度だけ。以降はcleanなattachmentだけを閉じる。
                                *disposition = DocumentCloseDisposition::PreserveChanges;
                                *revision = client.editor.document_connection().attach(id).map_err(|e| e.to_string())?.document.revision();
                            }
                        }
                        client.editor.close_documents(&decisions).map_err(|error| {
                            if saved_in_plan {
                                format!("保存しました。文書を閉じられませんでした: {error}")
                            } else {
                                error.to_string()
                            }
                        })?;
                        client.persist_view(view)?;
                        client.snapshot()
                    })?;
                    let snapshot = wait_client(receiver).await?;
                    committed.extend(ids);
                    results.push((request.group, snapshot, request.windows));
                }
                Ok(results)
            }.await;
            let _ = this.update(cx, |root, cx| {
                for group in groups { set_closing(&group, false, cx); }
                match result {
                    Err(error) => {
                        root.set_status(error, cx);
                        root.editor.update(cx, |editor, cx| { editor.submit(true, |_| Ok(()), cx); });
                    }
                    Ok(results) => {
                        let mut handles = Vec::new();
                        for (group, snapshot, windows) in results {
                            group.update(cx, |group, cx| {
                                group.publish(snapshot, cx);
                                for id in windows {
                                    if let Some(member) = group.members.remove(&id) { handles.push(member.handle); }
                                    group.layout.remove_window(id);
                                }
                                cx.notify();
                            });
                        }
                        for handle in handles { let _ = handle.update(cx, |_, window, _| window.remove_window()); }
                    }
                }
            });
        }).detach();
    }
}

/// Publishes a group's closing state and disables or enables all member editors accordingly.
fn set_closing(group: &Entity<WindowGroup>, closing: bool, cx: &mut gpui::App) {
    let editors = group.update(cx, |group, cx| {
        group.closing = closing;
        cx.notify();
        group
            .members
            .values()
            .map(|member| member.editor.clone())
            .collect::<Vec<_>>()
    });
    for editor in editors {
        editor.update(cx, |editor, cx| editor.set_enabled(!closing, cx));
    }
}
