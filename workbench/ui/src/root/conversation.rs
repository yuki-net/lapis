use super::*;
use crate::commands::UiCommand;
use crate::features::conversation::ConversationEvent;
use crate::workbench::WorkbenchMode;
use gpui::Context;

impl DesktopRoot {
    /// 一覧のTask選択は共通APIへ渡し、作業環境を変える操作だけWindow数で制限する。
    pub(crate) fn handle_conversation_event(
        &mut self,
        event: ConversationEvent,
        cx: &mut Context<Self>,
    ) {
        if let ConversationEvent::Switch(id) = event {
            self.dispatch_workbench_command(UiCommand::SelectTask(id), cx);
            return;
        }
        if self.group.read(cx).members.len() > 1 || self.group.read(cx).closing {
            self.set_status(
                "別Windowのタブを閉じるか再ドックしてから会話を切り替えてください".into(),
                cx,
            );
            return;
        }
        let view = self.persisted_view(cx);
        self.editor.update(cx, |editor, cx| {
            editor.submit(
                false,
                move |client| {
                    client.persist_view(view)?;
                    match event {
                        ConversationEvent::RegisterProject => {
                            if let Some(path) = client.choose_workspace_path() {
                                client
                                    .conversation
                                    .register_project(&mut client.editor, path)
                                    .map_err(|error| error.to_string())?;
                            }
                            Ok(())
                        }
                        ConversationEvent::RegisterSource(project) => {
                            if let Some(path) = client.choose_workspace_path() {
                                client
                                    .conversation
                                    .register_source(&mut client.editor, &project, path)
                                    .map_err(|error| error.to_string())?;
                            }
                            Ok(())
                        }
                        ConversationEvent::Create {
                            project,
                            primary,
                            sub,
                        } => client
                            .conversation
                            .create_task(&mut client.editor, &project, primary, sub)
                            .map(|_| ())
                            .map_err(|error| error.to_string()),
                        ConversationEvent::Source(role) => client
                            .conversation
                            .switch_source(&mut client.editor, role)
                            .map_err(|error| error.to_string()),
                        ConversationEvent::Switch(id) => client
                            .conversation
                            .switch(&id, &mut client.editor)
                            .map(|_| ())
                            .map_err(|error| error.to_string()),
                    }
                },
                cx,
            )
        });
    }

    /// 左のTask一覧、Command、ショートカットが共有するTask選択APIです。
    pub(super) fn select_task(&mut self, id: editor_core::ConversationId, cx: &mut Context<Self>) {
        if self.group.read(cx).closing || self.group.read(cx).synchronization_required {
            self.set_status(
                "終了・再同期処理の完了後にTaskを選択してください".into(),
                cx,
            );
            return;
        }
        if self.editor.read(cx).context_transition_pending()
            || (self.workbench.mode() == WorkbenchMode::Agent && self.editor.read(cx).has_pending())
        {
            self.set_status("処理の完了後にTaskを選択してください".into(), cx);
            return;
        }
        if self.workbench.mode() == WorkbenchMode::Ide {
            let Some(snapshot) = self.group.read(cx).latest.clone() else {
                return;
            };
            let Some(record) = snapshot.conversations.iter().find(|record| record.id == id) else {
                self.set_status("Taskが見つかりません".into(), cx);
                return;
            };
            self.notify_task_url(&id);
            self.workbench.select_task(id, record.view.clone());
            self.synchronize_group(cx);
            return;
        }
        if self.workbench.context_task() == Some(&id) {
            self.notify_task_url(&id);
            return;
        }
        if self.group.read(cx).members.len() > 1 || self.group.read(cx).closing {
            self.set_status(
                "別Windowのタブを閉じるか再ドックしてからTaskを切り替えてください".into(),
                cx,
            );
            return;
        }
        self.capture_workbench_view(cx);
        let view = self.persisted_view(cx);
        self.editor.update(cx, |editor, cx| {
            editor.submit(
                false,
                move |client| {
                    client.persist_view(view)?;
                    client
                        .conversation
                        .switch(&id, &mut client.editor)
                        .map(|_| ())
                        .map_err(|error| error.to_string())
                },
                cx,
            )
        });
    }

    pub(super) fn notify_task_url(&mut self, id: &editor_core::ConversationId) {
        self.last_task_for_url = Some(id.clone());
        crate::web_bridge::notify_task_selected(id.as_str());
    }
}
