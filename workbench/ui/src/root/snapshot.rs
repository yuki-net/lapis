use super::*;
use crate::client_adapter::ClientSnapshot;
use crate::features::id;
use crate::features::terminal::TerminalContext;
use crate::shell::main::ShellPresentation;
use crate::workbench::{ViewInstanceKey, WorkbenchMode};
use gpui::Context;

impl DesktopRoot {
    pub(super) fn sync_preview(&mut self, cx: &mut Context<Self>) {
        if !self.shell.read(cx).preview_visible() {
            return;
        }
        let editor = self.editor.read(cx);
        let Some(document_id) = editor.document_id() else {
            if self.preview_revision.take().is_some() {
                self.preview
                    .update(cx, |preview, cx| preview.update(String::new(), cx));
            }
            return;
        };
        let version = (document_id, editor.document().revision());
        if self.preview_revision.as_ref() == Some(&version) {
            return;
        }
        let text = editor.document().content_for_save();
        self.preview_revision = Some(version);
        self.preview
            .update(cx, |preview, cx| preview.update(text, cx));
    }

    /// Updates root feature contexts and shell presentation from a snapshot filtered for this host.
    /// Restores the saved shell layout when the active conversation changes.
    pub(crate) fn apply_snapshot(&mut self, snapshot: &ClientSnapshot, cx: &mut Context<Self>) {
        let switched = self.workbench.context_task() != Some(&snapshot.conversation);
        let workbench_view = self.workbench.apply_task_snapshot(
            snapshot.conversation.clone(),
            snapshot.view.clone(),
            switched,
        );
        let record = snapshot
            .conversations
            .iter()
            .find(|record| record.id == snapshot.conversation);
        // URL の fragment から渡された Task は、初回 snapshot で存在を確認してから選ぶ。
        if let Some(task) = self.pending_initial_task.take()
            && let Some(record) = snapshot
                .conversations
                .iter()
                .find(|record| record.id == task)
        {
            if self.workbench.mode() == WorkbenchMode::Ide {
                self.workbench
                    .select_task(task.clone(), record.view.clone());
            } else {
                let root = cx.entity().downgrade();
                cx.defer(move |cx| {
                    let _ = root.update(cx, |root, cx| root.select_task(task.clone(), cx));
                });
            }
        }
        self.files
            .update(cx, |files, cx| files.update(snapshot, cx));
        self.resources.update(cx, |resources, cx| {
            resources.set_workspace(snapshot.workspace.clone(), cx)
        });
        self.git.update(cx, |git, cx| {
            git.set_workspace(snapshot.workspace.clone(), cx);
            let role = record
                .map(|record| format!("{:?}", record.source_role))
                .unwrap_or_default();
            git.set_context_label(format!(
                "{} · {}",
                role,
                snapshot
                    .root
                    .as_ref()
                    .map(|root| root.display().to_string())
                    .unwrap_or_default()
            ));
        });
        let selected = self
            .workbench
            .selected_task()
            .cloned()
            .filter(|id| snapshot.conversations.iter().any(|record| &record.id == id))
            .unwrap_or_else(|| snapshot.conversation.clone());
        if self.workbench.selected_task() != Some(&selected) {
            self.workbench
                .select_task(selected.clone(), snapshot.view.clone());
        }
        if self.task_url_initialized && self.last_task_for_url.as_ref() != Some(&selected) {
            self.notify_task_url(&selected);
        }
        self.last_task_for_url = Some(selected.clone());
        self.task_url_initialized = true;
        let tasks_key = ViewInstanceKey::new(
            crate::shell::PanelTab::tool(id::VIEW_ASSISTANT).stable_view_key(),
        )
        .unwrap();
        let task_reference = self
            .workbench
            .task_reference(&tasks_key)
            .cloned()
            .unwrap_or_else(|| selected.clone());
        self.sync_task_context(snapshot, &task_reference, cx);
        self.search.update(cx, |search, cx| {
            search.set_workspace(snapshot.root.clone(), cx)
        });
        self.settings.update(cx, |settings, cx| {
            settings.set_workspace(
                snapshot.root.clone(),
                snapshot
                    .workspace
                    .as_ref()
                    .map(|workspace| workspace.as_str().to_owned()),
                cx,
            )
        });
        // Terminal はタブごとに個体が異なるため、代表として表示中の Terminal タブの
        // 所属を使う。Resource 固定の参照先は各タブ自身が保持する。
        let terminal_key = self
            .shell
            .read(cx)
            .first_active_tab_of(self.terminal_view.as_str())
            .and_then(|tab| ViewInstanceKey::new(tab.stable_view_key()));
        let terminal_scope = terminal_key
            .as_ref()
            .and_then(|key| self.workbench.view_scope(key));
        let terminal_target = terminal_context_target(
            terminal_scope,
            &selected,
            &snapshot.conversation,
            &snapshot.conversations,
        );
        if let Some(target) = terminal_target {
            let root = target
                .record
                .and_then(|record| record.workspace.root.clone());
            let summaries = target
                .record
                .map(|record| record.terminals.clone())
                .unwrap_or_default();
            let conversation = target.task;
            self.terminal.borrow_mut().set_context(TerminalContext {
                root: root.clone(),
                conversation: conversation.clone(),
            });
            for entity in self.terminal.borrow().entities() {
                entity.update(cx, |feature, cx| {
                    feature.set_context(root.clone(), conversation.clone(), summaries.clone(), cx)
                });
            }
        }
        self.shell
            .update(cx, |shell, cx| shell.bind_view_resources(cx));
        self.conversation.update(cx, |conversation, cx| {
            conversation.context = Some(snapshot.conversation.clone());
            conversation.update(
                snapshot.conversations.clone(),
                snapshot.projects.clone(),
                snapshot.project_error.clone(),
                selected,
                cx,
            )
        });
        let editor = self.editor.read(cx);
        let line_ending_version = editor
            .document_id()
            .map(|id| (id, editor.document().revision()));
        let line_ending = match self.line_ending_cache.as_ref() {
            Some((cached, ending)) if line_ending_version.as_ref() == Some(cached) => *ending,
            _ => {
                let ending = if editor.document().contains_crlf() {
                    "CRLF"
                } else {
                    "LF"
                };
                self.line_ending_cache = line_ending_version.map(|version| (version, ending));
                ending
            }
        };
        let workspace_name = self
            .connection
            .as_ref()
            .and_then(|connection| connection.workspace_name.clone())
            .unwrap_or_else(|| snapshot.workspace_name.clone());
        let presentation = ShellPresentation {
            documents: snapshot.tabs.clone(),
            active: snapshot.active.clone(),
            root: snapshot.root.clone(),
            workspace_name,
            workspace_id: snapshot
                .workspace
                .as_ref()
                .map(|workspace| workspace.as_str().to_owned()),
            connection: self.connection.clone(),
            cursor: editor.cursor_line_column(),
            line_ending,
            encoding: editor.document().encoding(),
            recent_workspaces: snapshot
                .conversations
                .iter()
                .filter_map(|record| record.workspace.root.clone())
                .collect(),
        };
        let retained = self.workbench.retained_views();
        let hidden = self.workbench.hidden_views();
        self.shell.update(cx, |shell, cx| {
            shell.update_presentation(presentation, cx);
            if let Some(view) = workbench_view {
                shell.restore_workbench_view(view, &retained, cx);
            }
            shell.apply_scope_visibility(hidden, cx);
        });
        self.sync_resource_tabs(cx);
        self.sync_resource_manager_visibility(cx);
        self.sync_preview(cx);
    }

    fn sync_task_context(
        &mut self,
        snapshot: &ClientSnapshot,
        selected: &editor_core::ConversationId,
        cx: &mut Context<Self>,
    ) {
        let record = snapshot
            .conversations
            .iter()
            .find(|record| &record.id == selected);
        let key = (
            selected.clone(),
            record.and_then(|record| record.workspace.root.clone()),
        );
        if self.task_context.as_ref() == Some(&key) {
            return;
        }
        self.task_context = Some(key.clone());
        self.task_context_resolution = TaskContextResolution::Pending;
        self.task_context_gate.invalidate();
        let token = self.task_context_gate.issue();
        self.tasks.update(cx, |tasks, cx| {
            tasks.set_conversation(selected.clone());
            tasks.set_workspace(None, cx);
        });
        let selected_execution = record.and_then(|record| record.selected_execution.clone());
        let task = selected.clone();
        let admitted = self.worker.submit(
            cx,
            move |client| {
                client
                    .conversation
                    .task_workspace_id(&task, &client.editor)
                    .map_err(|error| error.to_string())
            },
            move |root, result, cx| {
                // A→B→Aや作業ソース変更後の古い解決結果を適用しない。
                if !root.task_context_gate.accept(token) || root.task_context.as_ref() != Some(&key)
                {
                    return;
                }
                match result {
                    Ok(workspace) => {
                        root.tasks.update(cx, |tasks, cx| {
                            tasks.set_workspace(workspace, cx);
                            tasks.selected_execution = selected_execution;
                        });
                        root.task_context_resolution = TaskContextResolution::Ready;
                    }
                    Err(error) => {
                        root.task_context_resolution = TaskContextResolution::Unavailable;
                        root.set_status(error, cx);
                    }
                }
                root.persist_signature = None;
                root.schedule_persist(cx);
            },
        );
        if let Err(error) = admitted {
            self.task_context_resolution = TaskContextResolution::Unavailable;
            self.set_status(error, cx);
            self.persist_signature = None;
            self.schedule_persist(cx);
        }
    }

    /// HostのWorkbench変更を、未確定の入力を待ってからUIへ反映する。
    #[cfg(not(target_family = "wasm"))]
    pub(super) fn apply_remote_workbench(&mut self, cx: &mut Context<Self>) {
        let Some(watch) = self.workbench_watch.clone() else {
            return;
        };
        let Some((revision, view)) = watch.take_if_changed(self.workbench_revision) else {
            return;
        };
        if self.editor.read(cx).has_in_flight() {
            // 完了時のIdleで再試行する。revisionは進めず変更を保持する。
            return;
        }
        self.workbench_revision = revision;
        let Some(shared) = view.workspace.clone() else {
            return;
        };
        let layout = desktop_client::ConversationViewState {
            workspace: None,
            ..view
        };
        // 適用したHostの表示をそのまま送り返さない。
        self.workbench_publish_signature = Some(workbench_publish_view(&layout));
        self.shell
            .update(cx, |shell, cx| shell.restore_view(layout, cx));
        self.sync_resource_tabs(cx);
        let worker = self.worker.clone();
        let _ = worker.submit(
            cx,
            move |client| client.restore_workbench(shared),
            |root, result, cx| {
                if let Ok(snapshot) = result {
                    root.apply_snapshot(&snapshot, cx);
                }
                // 反映中に届いた次の変更を、完了後すぐに適用する。
                root.apply_remote_workbench(cx);
            },
        );
    }

    /// 共有 view を Editor と WindowGroup にも反映する。
    ///
    /// Shell のタブ構成だけを更新すると、共有された active 文書が変わっても Editor の
    /// 本文が前の文書のまま残る。Editor と WindowGroup にも同じ snapshot を渡し、
    /// 選択中の文書と本文を一致させる。
    pub(super) fn apply_shared_snapshot(
        &mut self,
        snapshot: &ClientSnapshot,
        cx: &mut Context<Self>,
    ) {
        self.apply_snapshot(snapshot, cx);
        self.group
            .update(cx, |group, cx| group.publish(snapshot.clone(), cx));
        self.editor.update(cx, |editor, cx| {
            editor.receive_shared_snapshot(snapshot, cx)
        });
        self.synchronize_group(cx);
    }
}
