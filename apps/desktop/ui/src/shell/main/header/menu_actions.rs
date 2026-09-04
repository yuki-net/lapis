use super::*;

use super::menu_definition::MenuAction;
use crate::shell::HeaderMenuSection as MenuId;

impl Editor {
    pub(super) fn toggle_header_menu(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.shell.header_menu_open = !self.shell.header_menu_open;
        self.shell.header_menu_anchor = position;
        self.shell.header_menu_section = None;
        if self.shell.header_menu_open {
            self.shell.command_palette_open = false;
            self.shell.tool_picker = None;
            self.shell.settings_menu_open = false;
            self.shell.theme_picker_open = false;
        } else {
            self.shell.header_menu_section = None;
        }
        cx.notify();
    }

    pub(super) fn close_header_menu(&mut self, cx: &mut Context<Self>) {
        if self.shell.header_menu_open {
            self.shell.header_menu_open = false;
            self.shell.header_menu_section = None;
            cx.notify();
        }
    }

    pub(super) fn select_header_menu(&mut self, menu: MenuId, cx: &mut Context<Self>) {
        if self.shell.header_menu_open && self.shell.header_menu_section != Some(menu) {
            self.shell.header_menu_section = Some(menu);
            cx.notify();
        }
    }

    pub(crate) fn header_menu_key_down(
        &mut self,
        event: &KeyDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.shell.header_menu_open && event.keystroke.key == "escape" {
            self.close_header_menu(cx);
        }
    }

    pub(super) fn execute_header_menu_action(
        &mut self,
        action: MenuAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_header_menu(cx);
        match action {
            MenuAction::NewFile => self.new_document(&New, window, cx),
            MenuAction::NewFolder => self.create_new_folder(window, cx),
            MenuAction::NewWindow => self.open_new_window(cx),
            MenuAction::OpenProject => self.open_project(window, cx),
            MenuAction::OpenFile => self.open_file(window, cx),
            MenuAction::CloseProject => self.close_project(window, cx),
            MenuAction::CloseWindow => window.remove_window(),
            MenuAction::Undo => self.undo(&Undo, window, cx),
            MenuAction::Redo => self.redo(&Redo, window, cx),
            MenuAction::Cut => self.cut(&Cut, window, cx),
            MenuAction::Copy => self.copy(&Copy, window, cx),
            MenuAction::Paste => self.paste(&Paste, window, cx),
            MenuAction::ToggleLeftPanel => {
                self.toggle_header_panel(crate::extension_ui::PanelPosition::Left, cx)
            }
            MenuAction::ToggleBottomPanel => {
                self.toggle_bottom_panel(&ToggleBottomPanel, window, cx)
            }
            MenuAction::ToggleInspector => self.toggle_inspector_window(window, cx),
            MenuAction::ToggleRightPanel => {
                self.toggle_header_panel(crate::extension_ui::PanelPosition::Right, cx)
            }
            MenuAction::Placeholder => {}
        }
    }
}
