use super::*;

use gpui::{TitlebarOptions, WindowBounds, WindowOptions};

impl Editor {
    pub(super) fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.session.is_empty() || self.last_line_layouts.is_empty() {
            return 0;
        }
        let Some(bounds) = self.last_editor_bounds else {
            return self.cursor_offset();
        };
        let line_index = if position.y <= bounds.top() {
            0
        } else {
            ((f32::from(position.y - bounds.top()) / 24.0).floor() as usize)
                .min(self.session.len_lines().saturating_sub(1))
        };
        let Some(layout) = self
            .last_line_layouts
            .iter()
            .min_by_key(|layout| layout.line_index.abs_diff(line_index))
        else {
            return self.cursor_offset();
        };
        let byte = layout
            .line
            .closest_index_for_x(position.x - layout.origin.x);
        layout.start_char
            + layout.line.text[..byte.min(layout.line.text.len())]
                .chars()
                .count()
    }

    pub(super) fn editor_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.is_selecting = true;
        window.focus(&self.focus_handle);
        let offset = self.index_for_mouse_position(event.position);
        if event.modifiers.shift {
            self.select_to(offset, cx);
        } else {
            self.move_to(offset, cx);
        }
    }

    pub(super) fn editor_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_selecting && event.dragging() {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    pub(super) fn editor_mouse_up(
        &mut self,
        _: &MouseUpEvent,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
        self.is_selecting = false;
    }

    pub(super) fn open_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.session.choose_workspace() {
            Ok(DocumentAction::Completed) => {
                if let Ok(Some(view)) = self
                    .conversation
                    .session
                    .restore_matching_workspace(&mut self.session)
                {
                    self.apply_conversation_view(view);
                }
                self.selected_range = 0..0;
                self.shell.synchronize_documents(&self.session.tabs());
                self.status = "Workspaceを開きました".to_owned();
                window.focus(&self.focus_handle);
                cx.notify();
            }
            Ok(DocumentAction::Cancelled) => {}
            Err(error) => self.status = format!("読み込み失敗: {error}"),
        }
    }

    pub(super) fn open_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.session.choose_file() {
            Ok(DocumentAction::Completed) => {
                self.selected_range = 0..0;
                self.shell.synchronize_documents(&self.session.tabs());
                self.status = "ファイルを開きました".to_owned();
                window.focus(&self.focus_handle);
                self.refresh_feature_activation();
                cx.notify();
            }
            Ok(DocumentAction::Cancelled) => {}
            Err(error) => {
                self.status = format!("ファイルを開けませんでした: {error}");
                cx.notify();
            }
        }
    }

    pub(super) fn save_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.session.save_document() {
            Ok(DocumentAction::Completed) => {
                self.status = "保存しました".to_owned();
                window.focus(&self.focus_handle);
                cx.notify();
            }
            Ok(DocumentAction::Cancelled) => {}
            Err(error) => self.status = format!("保存失敗: {error}"),
        }
    }

    pub(super) fn create_new_folder(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(root) = self.session.workspace_root() {
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let folder_name = format!("new-folder-{}", timestamp % 10000);
            let new_dir = root.join(&folder_name);
            if let Err(error) = std::fs::create_dir_all(&new_dir) {
                self.status = format!("フォルダ作成失敗: {error}");
            } else {
                let _ = self.session.refresh_file_tree();
                self.status = format!("フォルダを作成しました: {folder_name}");
            }
            cx.notify();
        } else {
            self.status = "Workspaceを開いてからフォルダを作成してください".to_owned();
            cx.notify();
        }
    }

    pub(super) fn open_new_window(&mut self, cx: &mut Context<Self>) {
        let repository = self.session.repository();
        let file_dialog = self.session.file_dialog();
        let state_repository = self.session.state_repository();
        let new_session =
            lapis_app_services::EditorSession::new(repository, file_dialog, state_repository);
        let snapshot = new_session.snapshot();
        let task = lapis_app_services::TaskSession::new(self.tasks.session.backend());
        let git = lapis_app_services::GitSession::new(self.git.session.backend());
        let lsp = lapis_app_services::LspSession::new(self.problems.lsp.backend());
        let terminal = lapis_app_services::TerminalSession::new(self.terminal.session.backend());
        let search =
            lapis_app_services::WorkspaceSearchSession::new(self.search.workspace.backend());
        let conversation = lapis_app_services::ConversationSession::new(
            self.conversation.session.repository(),
            snapshot,
        );
        let settings = self.settings.clone();
        let services = crate::app::DesktopServices::new(
            task,
            git,
            lsp,
            terminal,
            search,
            conversation,
            settings,
        );

        let bounds = Bounds::centered(None, size(px(1440.0), px(900.0)), cx);
        let _ = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Lapis".into()),
                    appears_transparent: true,
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |window, cx| {
                let editor = cx.new(|cx| {
                    Editor::new(
                        new_session,
                        services,
                        crate::app::InitialView::default(),
                        cx,
                    )
                });
                window.focus(&editor.read(cx).editor_focus_handle());
                editor
            },
        );
    }

    pub(super) fn close_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(error) = self.session.close_workspace() {
            self.status = format!("プロジェクト終了失敗: {error}");
        } else {
            self.selected_range = 0..0;
            self.shell.synchronize_documents(&self.session.tabs());
            self.status = "プロジェクトを閉じました".to_owned();
            window.focus(&self.focus_handle);
            self.refresh_feature_activation();
        }
        cx.notify();
    }

    pub(crate) fn toggle_inspector_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.shell.command_palette_open = false;
        #[cfg(debug_assertions)]
        {
            self.status = match crate::devtools::toggle_inspector(window, cx) {
                Ok(true) => "Inspectorを別ウィンドウで開きました".to_owned(),
                Ok(false) => "Inspectorを閉じました".to_owned(),
                Err(error) => error,
            };
        }
        #[cfg(not(debug_assertions))]
        {
            self.status = "Inspectorは開発ビルドでのみ利用可能です".to_owned();
        }
        cx.notify();
    }
}
