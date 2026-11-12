use crate::app::SessionViewHandle;
use desktop_client::{
    ConversationRecord, ConversationSession, ConversationViewState, DocumentTab, EditorSession,
    LspSession, SessionViewWatch,
};
use document::DocumentCache;
use editor_core::{ConversationId, DocumentId, WorkspaceId};
use gpui::Context;
use std::path::PathBuf;
use workspace::FileEntry;

fn session_view_watch(handle: &SessionViewHandle) -> &SessionViewWatch {
    #[cfg(not(target_family = "wasm"))]
    {
        handle.watch()
    }
    #[cfg(target_family = "wasm")]
    {
        handle
    }
}

pub(crate) struct DocumentClient {
    pub editor: EditorSession,
    pub conversation: ConversationSession,
    pub lsp: LspSession,
    pub edit_chain: EditChain,
    pub git: std::sync::Arc<dyn git::GitConnection>,
    pub snapshot_sequence: u64,
    pub document_owners: std::collections::HashMap<DocumentId, u64>,
    /// Workspace の view state 共有。未対応の構成では `None`。
    session_view: Option<SessionViewHandle>,
    /// 最後に共有したタブ構成。カーソル移動だけでは共有を進めない。
    session_view_signature: Option<(Vec<DocumentId>, Option<DocumentId>)>,
    /// 最後に適用した通知順。古い通知の再適用を防ぐ。
    session_view_sequence: u64,
}

impl DocumentClient {
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn new(
        editor: EditorSession,
        conversation: ConversationSession,
        lsp: LspSession,
        git: std::sync::Arc<dyn git::GitConnection>,
        session_view: Option<std::sync::Arc<desktop_client::SessionViewSync>>,
    ) -> Self {
        Self {
            editor,
            conversation,
            lsp,
            edit_chain: EditChain::default(),
            git,
            snapshot_sequence: 0,
            document_owners: Default::default(),
            session_view,
            session_view_signature: None,
            session_view_sequence: 0,
        }
    }

    #[cfg(target_family = "wasm")]
    pub(crate) fn new(
        editor: EditorSession,
        conversation: ConversationSession,
        lsp: LspSession,
        git: std::sync::Arc<dyn git::GitConnection>,
        session_view: Option<SessionViewWatch>,
    ) -> Self {
        Self {
            editor,
            conversation,
            lsp,
            edit_chain: EditChain::default(),
            git,
            snapshot_sequence: 0,
            document_owners: Default::default(),
            session_view,
            session_view_signature: None,
            session_view_sequence: 0,
        }
    }
}

#[derive(Clone)]
pub(crate) struct ClientSnapshot {
    pub sequence: u64,
    pub synchronization_error: Option<String>,
    pub document_owners: std::collections::HashMap<DocumentId, u64>,
    pub tabs: Vec<DocumentTab>,
    pub documents: Vec<DocumentCache>,
    pub active: Option<DocumentId>,
    pub workspace: Option<WorkspaceId>,
    pub root: Option<PathBuf>,
    pub workspace_name: String,
    pub files: Vec<FileEntry>,
    pub conversations: Vec<ConversationRecord>,
    pub projects: Vec<desktop_client::Project>,
    pub project_error: Option<String>,
    pub conversation: ConversationId,
    pub view: ConversationViewState,
    pub edit_epoch: EditEpoch,
    pub workspace_view: desktop_client::WorkspaceViewState,
}

pub(crate) struct DocumentCloseReport {
    pub close: Result<desktop_client::DocumentCloseProgress, desktop_client::DocumentCloseFailure>,
    pub persistence: Result<(), String>,
    pub snapshot: ClientSnapshot,
}

impl DocumentCloseReport {
    pub(crate) fn error(&self) -> Option<String> {
        let errors = [
            self.close.as_ref().err().map(ToString::to_string),
            self.persistence.as_ref().err().cloned(),
            self.snapshot.synchronization_error.clone(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        (!errors.is_empty()).then(|| errors.join("; "))
    }
}

impl DocumentClient {
    /// HostのWorkbenchを、未保存変更が無い場合だけEditorへ適用する。
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn restore_workbench(
        &mut self,
        shared: desktop_client::SharedWorkspaceView,
    ) -> Result<ClientSnapshot, String> {
        if self.editor.is_dirty() {
            return Err("未保存の変更があるためHostの状態を適用しません".into());
        }
        let workspace = self
            .editor
            .workspace_id()
            .cloned()
            .ok_or("Workspaceがありません")?;
        let root = self
            .editor
            .workspace_root()
            .map(ToOwned::to_owned)
            .ok_or("Workspaceがありません")?;
        let snapshot = desktop_client::WorkspaceViewState::from_shared(&shared, &workspace, root)
            .map_err(|error| error.to_string())?;
        self.editor
            .restore_snapshot(snapshot)
            .map_err(|error| error.to_string())?;
        Ok(self.reconcile_snapshot())
    }

    /// Workspace を閉じたときに購読を解除する。以後の delta は適用しない。
    pub(crate) fn close_session_view(&mut self) {
        if let Some(handle) = &self.session_view {
            #[cfg(not(target_family = "wasm"))]
            handle.close();
            #[cfg(target_family = "wasm")]
            desktop_client::web_host::close_session_view();
            let _ = handle;
        }
        self.session_view_signature = None;
    }

    /// 1 回だけ共有 view を取り込み、適用できた snapshot を返す。
    ///
    /// Workspace を開いていれば購読を開始し、snapshot を現在の WorkspaceViewState に
    /// 適用する。UI はこれを worker で呼び、結果を Editor の snapshot として受け取る。
    /// snapshot が空（まだ誰も共有していない）の場合も購読済みとして扱い、以後の
    /// ローカル変更を共有へ送れるようにする。
    pub(crate) fn apply_session_view(&mut self) -> Result<Option<ClientSnapshot>, String> {
        let Some(workspace) = self.editor.workspace_id().cloned() else {
            self.close_session_view();
            return Ok(None);
        };
        let Some(handle) = &self.session_view else {
            return Ok(None);
        };
        // Desktop は開いた時点で購読を張る。Web は接続時に購読済みで、
        // snapshot 待ちの間は何もしない。
        #[cfg(not(target_family = "wasm"))]
        handle.open(&workspace)?;
        let watch = session_view_watch(handle);
        let applied = watch.take_if_changed(&workspace, self.session_view_sequence);
        let Some((sequence, _, view)) = applied else {
            return Ok(None);
        };
        self.session_view_sequence = sequence;
        // 未保存の文書は共有側で閉じられていても破棄しない。タブ構成だけを合わせ、
        // 未保存内容は apply_shared_view が末尾のタブとして残す。
        self.editor
            .apply_shared_view(&view)
            .map_err(|error| error.to_string())?;
        // 適用した構成をそのまま送り返さない。
        self.session_view_signature = Some(self.editor.tab_signature());
        Ok(Some(self.reconcile_snapshot()))
    }

    /// バックグラウンド復帰などで snapshot を取り直してから共有 view を適用する。
    pub(crate) fn resync_session_view(&mut self) -> Result<Option<ClientSnapshot>, String> {
        let Some(handle) = &self.session_view else {
            return Ok(None);
        };
        #[cfg(not(target_family = "wasm"))]
        if self.editor.workspace_id().is_some() {
            handle.resync()?;
        }
        #[cfg(target_family = "wasm")]
        {
            let _ = handle;
            if self.editor.workspace_id().is_some() {
                // snapshot の反映は event pump から通知される。ここでは要求だけ出す。
                desktop_client::web_host::resync_session_view();
            }
        }
        self.apply_session_view()
    }

    /// 自分のタブ開閉とアクティブ文書を共有へ送る。
    ///
    /// 共有するのはタブの構成とアクティブ文書だけで、カーソル位置やスクロールは送らない。
    /// 直前に適用した共有値と同じタブ構成なら送らないため、自分の更新が event として
    /// 戻ってきても再送や再適用のループにならない。
    pub(crate) fn publish_session_view(&mut self) -> Result<(), String> {
        let Some(handle) = &self.session_view else {
            return Ok(());
        };
        let Some(workspace) = self.editor.workspace_id().cloned() else {
            return Ok(());
        };
        // 購読の完了前は送らない。参加前の送信は共有済みの状態を上書きしてしまう。
        if !session_view_watch(handle).is_synced(&workspace) {
            return Ok(());
        }
        let signature = self.editor.tab_signature();
        if self.session_view_signature.as_ref() == Some(&signature) {
            return Ok(());
        }
        // Web も Desktop と同じくタブ構成とアクティブ文書だけを共有する。
        let snapshot = self.editor.snapshot();
        let view = if cfg!(target_family = "wasm") {
            snapshot.to_shared_tabs()
        } else {
            snapshot.to_shared_view()
        }
        .map_err(|error| error.to_string())?;
        #[cfg(not(target_family = "wasm"))]
        handle.publish(view)?;
        #[cfg(target_family = "wasm")]
        {
            let _ = handle;
            desktop_client::web_host::publish_session_view(view);
        }
        self.session_view_signature = Some(signature);
        Ok(())
    }

    /// close済みIDは、表示状態保存や正本再取得の結果から独立して返す。
    pub(crate) fn close_documents(
        &mut self,
        decisions: &[(
            DocumentId,
            desktop_client::DocumentCloseDisposition,
            document::Revision,
        )],
        view: PersistedView,
    ) -> DocumentCloseReport {
        let close = self.editor.close_documents(decisions);
        let persistence = self
            .editor
            .persist_documents()
            .map_err(|e| e.to_string())
            .and_then(|()| self.persist_view(view));
        DocumentCloseReport {
            close,
            persistence,
            snapshot: self.reconcile_snapshot(),
        }
    }

    /// 残す全Documentの再attachを確認してからSnapshotを返す。
    pub fn snapshot(&mut self) -> Result<ClientSnapshot, String> {
        let snapshot = self.reconcile_snapshot();
        if let Some(error) = &snapshot.synchronization_error {
            return Err(error.clone());
        }
        Ok(snapshot)
    }

    /// 正本の再取得が失敗しても確認済み所属を返す。未確認cacheは復旧まで編集しない。
    pub(crate) fn reconcile_snapshot(&mut self) -> ClientSnapshot {
        self.editor.poll_external_changes();
        let synchronization_error = self.editor.refresh_documents().err().map(|e| e.to_string());
        let tabs = self.editor.tabs();
        let documents = self.editor.document_caches();
        self.document_owners
            .retain(|id, _| tabs.iter().any(|tab| &tab.id == id));
        self.snapshot_sequence += 1;
        ClientSnapshot {
            sequence: self.snapshot_sequence,
            synchronization_error,
            document_owners: self.document_owners.clone(),
            tabs,
            documents,
            active: self.editor.active_document_id().cloned(),
            workspace: self.editor.workspace_id().cloned(),
            root: self.editor.workspace_root().map(ToOwned::to_owned),
            workspace_name: self.editor.workspace_name().to_owned(),
            files: self.editor.file_tree().to_vec(),
            conversations: self.conversation.records().to_vec(),
            projects: self.conversation.projects().to_vec(),
            project_error: self.conversation.load_error().map(ToOwned::to_owned),
            conversation: self.conversation.active_id().clone(),
            view: self.conversation.active_view(),
            edit_epoch: self.edit_chain.epoch(),
            workspace_view: self.editor.snapshot(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct PersistedView {
    pub document: Option<(DocumentId, std::ops::Range<usize>, usize, f32, f32)>,
    pub layout: ConversationViewState,
    pub execution: Option<editor_core::ExecutionId>,
    pub terminals: Vec<terminal::TerminalSnapshot>,
    pub terminal_owner: Option<editor_core::ConversationId>,
    pub input: Option<String>,
}

impl DocumentClient {
    /// Workspace の選択ダイアログを開く。UI 側は `WorkspaceDialog` を直接呼ばない。
    pub(crate) fn choose_workspace_path(&self) -> Option<std::path::PathBuf> {
        self.editor.file_dialog().choose_workspace_path()
    }

    /// Workbenchの表示変更を、debounceされたローカル保存を待たずに共有先へ通知する。
    pub(crate) fn publish_workbench(&mut self, view: ConversationViewState) -> Result<(), String> {
        self.conversation
            .publish_workbench(&self.editor, view)
            .map_err(|error| error.to_string())
    }

    pub(crate) fn persist_view(&mut self, view: PersistedView) -> Result<(), String> {
        let epoch = self.edit_chain.epoch();
        let recovered = self.edit_chain.apply(epoch, || Ok(())).is_err();
        if recovered {
            let text = view.input.as_deref().ok_or("復旧する入力がありません")?;
            self.editor
                .recover_draft(text)
                .map_err(|error| error.to_string())?;
            self.edit_chain.restart();
        }
        if let Some((id, selection, cursor, scroll_x, scroll_y)) = view.document
            && (recovered || self.editor.select_document_for_operation(&id))
        {
            self.editor
                .update_active_view(selection, cursor, scroll_x, scroll_y);
        }
        if self.editor.workspace_root().is_none() && self.editor.tabs().is_empty() {
            return Ok(());
        }
        let terminals = view
            .terminal_owner
            .is_some()
            .then_some(view.terminals.as_slice());
        self.conversation
            .capture_bound(
                &mut self.editor,
                view.layout,
                view.execution,
                view.terminal_owner.as_ref(),
                terminals,
            )
            .map_err(|error| error.to_string())
    }
}

pub(crate) use crate::client_worker::{ClientWorker, EditChain, EditEpoch, ResponseGate};

#[cfg(test)]
pub(crate) mod tests;

impl<State: Send + 'static> ClientWorker<State> {
    pub(crate) fn submit<Entity: 'static, Output: Send + 'static>(
        &self,
        cx: &mut Context<Entity>,
        operation: impl FnOnce(&mut State) -> Result<Output, String> + Send + 'static,
        complete: impl FnOnce(&mut Entity, Result<Output, String>, &mut Context<Entity>) + 'static,
    ) -> Result<(), String> {
        let receiver = self.try_request(operation)?;
        // worker thread の結果をブロックせずに待つ。16ms 間隔のポーリングは行わない。
        let reply = cx
            .background_executor()
            .spawn(async move { receiver.recv() });
        cx.spawn(async move |this, cx| {
            let result = match reply.await {
                Ok(result) => result,
                Err(_) => Err("Client workerが終了しました".to_owned()),
            };
            let _ = this.update(cx, |entity, cx| complete(entity, result, cx));
        })
        .detach();
        Ok(())
    }
}
