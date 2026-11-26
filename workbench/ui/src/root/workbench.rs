use super::*;
use crate::client_adapter::PersistedView;
use crate::commands::UiCommand;
use crate::features::id;
use crate::workbench::{ViewInstanceKey, ViewScope, WorkbenchMode};
use extension_ui_contract::ViewId;
use gpui::{Context, Window};

impl DesktopRoot {
    /// Task切替前のUI状態だけをWorkbenchへ保存します。Editor/Terminalの
    /// Resource所有権や実行中Sessionはこの経路から変更しません。
    pub(super) fn capture_workbench_view(&mut self, cx: &gpui::App) {
        let task = self.workbench.context_task().cloned();
        let view = self.shell.read(cx).view_state();
        self.workbench.prepare_task_switch(task.as_ref(), &view);
    }

    /// 外部 Resource を参照するタブを開く。Resource の意味は View の宣言が決める。
    pub(super) fn activate_view_resource(
        &mut self,
        view: ViewId,
        resource: String,
        cx: &mut Context<Self>,
    ) {
        self.shell.update(cx, |shell, cx| {
            shell.open_resource_tab(view.clone(), resource, cx)
        });
        self.sync_resource_tabs(cx);
        cx.notify();
    }

    /// Resource Manager パネルの表示状態を Feature へ伝える。非表示の間は取得を止める。
    pub(super) fn sync_resource_manager_visibility(&mut self, cx: &mut Context<Self>) {
        let visible = self
            .shell
            .read(cx)
            .shell
            .view_visible_in_layout(id::VIEW_RESOURCES);
        self.resources
            .update(cx, |resources, cx| resources.set_visible(visible, cx));
    }

    /// 復元したLayoutのタブと実体を一致させ、表示中のタブへ自動起動を促す。
    ///
    /// 参照先 Resource はタブ自身が保持するため、ここで別のタブへ付け替えない。
    /// 一致させるのは Layout 上のタブと実体だけ。View の種類には依存しない。
    pub(super) fn sync_resource_tabs(&mut self, cx: &mut Context<Self>) {
        self.shell
            .update(cx, |shell, cx| shell.bind_view_resources(cx));
    }

    pub(super) fn dispatch_workbench_command(
        &mut self,
        command: UiCommand,
        cx: &mut Context<Self>,
    ) {
        match command {
            UiCommand::BindViewToSelectedTask(view) => self.bind_view_to_selected_task(view, cx),
            UiCommand::SelectTask(id) => self.select_task(id, cx),
            UiCommand::SetWorkbenchMode(mode) => self.set_workbench_mode(mode, cx),
            UiCommand::SetViewBinding { view, scope } => self.set_view_binding(view, scope, cx),
            _ => {}
        }
    }

    /// Mode変更もTask選択と同じWorkbench APIを経由します。
    pub(super) fn set_workbench_mode(&mut self, mode: WorkbenchMode, cx: &mut Context<Self>) {
        if self.workbench.mode() == mode {
            return;
        }
        if self.group.read(cx).closing
            || self.group.read(cx).synchronization_required
            || self.editor.read(cx).has_pending()
        {
            self.set_status("処理の完了後にModeを切り替えてください".into(), cx);
            return;
        }
        let task = self.workbench.context_task().cloned();
        let current = self.shell.read(cx).view_state();
        let mut candidate = self.workbench.clone();
        let restored = candidate.set_mode(mode, task.as_ref(), &current);
        let target = candidate.mode_context().cloned();
        if target == task || target.is_none() {
            self.workbench = candidate;
            if let Some(view) = restored {
                self.restore_workbench_layout(view, cx);
            }
            self.schedule_persist(cx);
            return;
        }
        if self.group.read(cx).members.len() > 1 {
            self.set_status(
                "別Windowのタブを閉じるか再ドックしてからModeを切り替えてください".into(),
                cx,
            );
            return;
        }
        let previous = self.persisted_view(cx);
        if !self
            .editor
            .update(cx, |editor, _| editor.begin_context_transition())
        {
            return;
        }
        let target = target.unwrap();
        let admitted = self.worker.submit(
            cx,
            move |client| {
                let result = client.persist_view(previous).and_then(|_| {
                    client
                        .conversation
                        .switch(&target, &mut client.editor)
                        .map_err(|error| error.to_string())
                });
                // 失敗時も確認済みの実状態を通知し、部分成功や切断を表示終了と混同しない。
                Ok((result, client.reconcile_snapshot()))
            },
            move |root, outcome, cx| {
                let (result, snapshot) = match outcome {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        root.editor
                            .update(cx, |editor, cx| editor.finish_context_transition(None, cx));
                        root.group.update(cx, |group, cx| {
                            group.synchronization_required = true;
                            group.status = error;
                            cx.notify();
                        });
                        return;
                    }
                };
                let success = result.is_ok();
                if success {
                    let current = root.shell.read(cx).view_state();
                    root.workbench.commit_mode_transition(candidate, &current);
                }
                root.editor.update(cx, |editor, cx| {
                    editor.finish_context_transition(Some(&snapshot), cx)
                });
                root.group
                    .update(cx, |group, cx| group.publish(snapshot, cx));
                root.synchronize_group(cx);
                if success {
                    if mode == WorkbenchMode::Ide
                        && let Some(view) = restored
                    {
                        root.restore_workbench_layout(view, cx);
                    }
                    root.schedule_persist(cx);
                } else if let Err(error) = result {
                    root.set_status(error, cx);
                }
            },
        );
        if let Err(error) = admitted {
            self.editor
                .update(cx, |editor, cx| editor.finish_context_transition(None, cx));
            self.set_status(error, cx);
        }
    }

    pub(super) fn bind_view_to_selected_task(
        &mut self,
        view: ViewInstanceKey,
        cx: &mut Context<Self>,
    ) {
        if let Some(task) = self.workbench.selected_task().cloned() {
            self.set_view_binding(view, ViewScope::Task(task), cx);
        }
    }

    fn sync_view_scopes(&mut self, cx: &mut Context<Self>) {
        for (tab, _) in self.shell.read(cx).shell.layout.placements() {
            let key = ViewInstanceKey::new(tab.stable_view_key()).unwrap();
            let scope = if tab
                .view_id()
                .is_some_and(|view| view.as_str() == id::VIEW_ASSISTANT)
            {
                ViewScope::FollowSelectedTask
            } else if (self.workbench.mode() == WorkbenchMode::Agent
                || matches!(tab, crate::shell::PanelTab::Document(_)))
                && let Some(task) = self.workbench.context_task().cloned()
            {
                ViewScope::Task(task)
            } else {
                ViewScope::SharedWorkbench
            };
            self.workbench.ensure_view_binding(key, scope);
        }
        let hidden = self.workbench.hidden_views();
        self.shell
            .update(cx, |shell, cx| shell.apply_scope_visibility(hidden, cx));
    }

    pub(super) fn restore_workbench_layout(
        &mut self,
        view: desktop_client::ConversationViewState,
        cx: &mut Context<Self>,
    ) {
        let retained = self.workbench.retained_views();
        let hidden = self.workbench.hidden_views();
        self.shell.update(cx, |shell, cx| {
            shell.restore_workbench_view(view, &retained, cx);
            shell.apply_scope_visibility(hidden, cx);
        });
    }

    /// View所属の変更は表示状態だけに限定し、Host Resourceの終了・複製を行いません。
    pub(super) fn set_view_binding(
        &mut self,
        view: ViewInstanceKey,
        scope: ViewScope,
        cx: &mut Context<Self>,
    ) {
        let key = view.clone();
        if !self
            .shell
            .read(cx)
            .shell
            .layout
            .placements()
            .iter()
            .any(|(tab, _)| tab.stable_view_key() == view.as_str())
        {
            self.set_status("表示対象のViewがありません".into(), cx);
            return;
        }
        if let ViewScope::Task(task) = &scope
            && !self
                .conversation
                .read(cx)
                .records
                .iter()
                .any(|record| &record.id == task)
        {
            self.set_status("所属先のTaskがありません".into(), cx);
            return;
        }
        if let ViewScope::Resource(resource) = &scope {
            // 外部 Resource を参照するタブだけが Resource 固定参照を持つ。個体IDは
            // タブごとに異なるため、種別はタブの View ではなく Resource の有無で判定する。
            let resource_tab = self
                .shell
                .read(cx)
                .shell
                .layout
                .placements()
                .into_iter()
                .find(|(tab, _)| tab.stable_view_key() == view.as_str())
                .map(|(tab, _)| tab)
                .is_some_and(|tab| tab.resource().is_some());
            let supported = resource_tab
                && (self
                    .terminal
                    .borrow()
                    .entities()
                    .iter()
                    .flat_map(|tab| tab.read(cx).summaries())
                    .any(|terminal| terminal.id.as_str() == resource)
                    || self.conversation.read(cx).records.iter().any(|record| {
                        record.terminals.iter().any(|terminal| {
                            terminal
                                .id
                                .as_ref()
                                .is_some_and(|id| id.as_str() == resource)
                        })
                    }));
            if !supported {
                self.set_status("このViewのResource固定参照には対応していません".into(), cx);
                return;
            }
        }
        self.workbench.bind_view(view, scope);
        debug_assert!(self.workbench.view_scope(&key).is_some());
        self.sync_view_scopes(cx);
        self.synchronize_group(cx);
        self.schedule_persist(cx);
        cx.notify();
    }

    fn execution_for_persistence(&self, cx: &gpui::App) -> Option<editor_core::ExecutionId> {
        execution_for_context(
            self.task_context_resolution
                .confirmed_task(self.task_context.as_ref().map(|(task, _)| task)),
            self.workbench.context_task(),
            self.tasks.read(cx).selected_execution.clone(),
            self.conversation
                .read(cx)
                .records
                .iter()
                .find(|record| Some(&record.id) == self.workbench.context_task())
                .and_then(|record| record.selected_execution.clone()),
        )
    }

    /// 外部 Resource を所有する機能の永続化用サマリー。全タブが同じコンテキストを共有する。
    fn terminal_owner_for_persistence(
        &self,
        cx: &gpui::App,
    ) -> Option<editor_core::ConversationId> {
        self.shell
            .read(cx)
            .view_resource_owner(&self.terminal_view, cx)
    }

    /// 外部 Resource の永続化用サマリー。Resource の意味は所有する機能が決める。
    fn terminal_persistence_summaries(
        &self,
        cx: &gpui::App,
    ) -> Vec<desktop_client::RestoredTerminal> {
        self.shell
            .read(cx)
            .persisted_view_resources(&self.terminal_view, cx)
    }

    /// Captures this root's editor, layout, task, terminal, and draft state for later persistence.
    pub(crate) fn persisted_view(&self, cx: &gpui::App) -> PersistedView {
        let mut layout = self.shell.read(cx).view_state();
        self.workbench.persisted_view(&mut layout);
        let terminals = self
            .terminal
            .borrow()
            .entities()
            .iter()
            .flat_map(|tab| tab.read(cx).summaries())
            .collect();
        PersistedView {
            document: self.editor.read(cx).view_state(),
            layout,
            execution: self.execution_for_persistence(cx),
            terminals,
            terminal_owner: self.terminal_owner_for_persistence(cx),
            input: self
                .editor
                .read(cx)
                .document_id()
                .map(|_| self.editor.read(cx).document().content_for_save()),
        }
    }

    /// Starts closing this root's entire group and returns false to defer native window removal.
    pub(super) fn request_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.close_hosts(crate::shell::window_group::ROOT_HOST, None, window, cx);
        false
    }

    /// Debounces persistence by 500 milliseconds when document versions, editor view, or layout change.
    /// Skips the write while closing; asynchronous persistence errors become status messages.
    pub(crate) fn schedule_persist(&mut self, cx: &mut Context<Self>) {
        self.sync_view_scopes(cx);
        let mut layout = self.shell.read(cx).view_state();
        let task = self.workbench.context_task().cloned();
        self.workbench.prepare_task_switch(task.as_ref(), &layout);
        self.workbench.persisted_view(&mut layout);
        // 表示の反映は500msの保存debounceから分離し、変更があれば同じフレームで通知する。
        let workbench = workbench_publish_view(&layout);
        if self.workbench_publish_signature.as_ref() != Some(&workbench)
            && self.workbench.context_task().is_some()
            && !self.group.read(cx).closing
            && !self.group.read(cx).synchronization_required
            && self.task_context_resolution != TaskContextResolution::Pending
        {
            self.workbench_publish_signature = Some(workbench.clone());
            let published = self.worker.submit(
                cx,
                move |client| client.publish_workbench(workbench),
                |root, result, _| {
                    if let Err(error) = result {
                        root.workbench_publish_signature = None;
                        eprintln!("WorkbenchをHostへ通知できません: {error}");
                    }
                },
            );
            if published.is_err() {
                self.workbench_publish_signature = None;
            }
        }
        let versions = self
            .group
            .read(cx)
            .latest
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .documents
                    .iter()
                    .map(|cache| {
                        (
                            cache.id.clone(),
                            cache.document.revision(),
                            cache.document.is_dirty(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.schedule_session_view_publish(cx);
        let owner = self.terminal_owner_for_persistence(cx);
        let summaries = if owner.is_some() {
            self.terminal_persistence_summaries(cx)
        } else {
            Vec::new()
        };
        let signature = (
            versions,
            self.editor.read(cx).view_state(),
            layout,
            self.execution_for_persistence(cx),
            owner,
            summaries,
        );
        if self.persist_signature.as_ref() == Some(&signature) {
            return;
        }
        self.persist_signature = Some(signature);
        self.persist_task = Some(cx.spawn(async move |this, cx| {
            crate::timer::Timer::after(std::time::Duration::from_millis(500)).await;
            let _ = this.update(cx, |root, cx| {
                if root.group.read(cx).closing
                    || root.group.read(cx).synchronization_required
                    || root.editor.read(cx).has_in_flight()
                    || root.task_context_resolution == TaskContextResolution::Pending
                {
                    // 指紋はデバウンス用であり、見送った書き込みは再同期後に再試行できるようにする。
                    // Workspace切替の確認前の表示を、Worker側の新Taskへ保存しない。
                    root.persist_signature = None;
                    return;
                }
                let view = root.persisted_view(cx);
                let _ = root.worker.submit(
                    cx,
                    move |client| {
                        client
                            .edit_chain
                            .apply(client.edit_chain.epoch(), || Ok(()))?;
                        client.persist_view(view)
                    },
                    |root, result, cx| {
                        if let Err(error) = result {
                            let title = root.shell.read(cx).t("notification.error.persistence");
                            crate::notification::publish_error(
                                cx,
                                "workspace.persistence_failed",
                                &title,
                                error,
                            );
                        }
                    },
                );
            });
        }));
    }
}
