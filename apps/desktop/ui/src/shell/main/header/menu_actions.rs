use super::*;

use super::menu_definition::MenuAction;
use crate::shell::HeaderMenuSection as MenuId;

impl Shell {
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
            MenuAction::NewFile => {
                self.execute_command(id::COMMAND_NEW_DOCUMENT.into(), window, cx)
            }
            MenuAction::NewFolder => self.create_new_folder(window, cx),
            MenuAction::NewWindow => self.open_new_window(cx),
            MenuAction::ConnectHost => cx.emit(UiCommand::ConnectHost),
            MenuAction::OpenProject => {
                self.execute_command(id::COMMAND_OPEN_WORKSPACE.into(), window, cx)
            }
            MenuAction::OpenFile => {
                self.execute_command(id::COMMAND_OPEN_DOCUMENT.into(), window, cx)
            }
            MenuAction::Save => self.execute_command(id::COMMAND_SAVE_DOCUMENT.into(), window, cx),
            MenuAction::SaveAs => self.execute_command(id::COMMAND_SAVE_AS.into(), window, cx),
            MenuAction::CloseProject => self.close_project(window, cx),
            MenuAction::CloseWindow => cx.emit(UiCommand::CloseWindow),
            MenuAction::Undo => self.execute_command(id::COMMAND_UNDO.into(), window, cx),
            MenuAction::Redo => self.execute_command(id::COMMAND_REDO.into(), window, cx),
            MenuAction::Cut => self.cut(&Cut, window, cx),
            MenuAction::Copy => self.copy(&Copy, window, cx),
            MenuAction::Paste => self.paste(&Paste, window, cx),
            MenuAction::ToggleLeftPanel => {
                self.execute_command(id::COMMAND_TOGGLE_LEFT_ZONE.into(), window, cx)
            }
            MenuAction::ToggleBottomZone => {
                self.execute_command(id::COMMAND_TOGGLE_BOTTOM_ZONE.into(), window, cx)
            }
            MenuAction::ToggleInspector => self.toggle_inspector_window(window, cx),
            MenuAction::ToggleRightPanel => {
                self.execute_command(id::COMMAND_TOGGLE_RIGHT_ZONE.into(), window, cx)
            }
            MenuAction::TogglePanelControls => {
                self.execute_command(id::COMMAND_TOGGLE_PANEL_CONTROLS.into(), window, cx)
            }
            MenuAction::Placeholder => {}
        }
    }
}
