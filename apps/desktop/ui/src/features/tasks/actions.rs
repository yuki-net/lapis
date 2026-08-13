use super::*;

impl Editor {
    pub(super) fn start_codex_task(
        &mut self,
        mode: TaskMode,
        isolated: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace_root) = self.session.workspace_root().map(ToOwned::to_owned) else {
            self.status = "Codex Task を開始する前に Workspace を開いてください".to_owned();
            cx.notify();
            return;
        };
        let selected = (!self.selected_range.is_empty())
            .then(|| self.session.slice_chars(self.selected_range.clone()).ok())
            .flatten();
        let clipboard = cx.read_from_clipboard().and_then(|item| item.text());
        let prompt = selected
            .or(clipboard)
            .filter(|text| !text.trim().is_empty())
            .unwrap_or_else(|| {
                "この Workspace の構成を読み取り専用で調査し、重要な点を日本語で簡潔に報告してください。ファイルは変更しないでください。"
                    .to_owned()
            });
        let result = if isolated {
            self.tasks.session.start_codex_in_worktree(
                &mut self.git.session,
                workspace_root,
                prompt,
                mode,
            )
        } else {
            self.tasks
                .session
                .start_codex_with_mode(workspace_root, prompt, mode)
        };
        match result {
            Ok(execution_id) => {
                self.tasks.selected_execution = Some(execution_id);
                self.shell.activate_view(
                    crate::extension_ui::PanelPosition::Right,
                    ViewId::new(id::VIEW_ASSISTANT),
                );
                self.refresh_feature_activation();
                self.status = "Codex Task を開始しました".to_owned();
            }
            Err(error) => self.status = format!("Task 開始失敗: {error}"),
        }
        cx.notify();
    }
}
