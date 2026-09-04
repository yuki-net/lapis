use super::*;

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(theme::colors().background_primary)
            .text_color(theme::colors().text_primary)
            .track_focus(&self.focus_handle(cx))
            .key_context("Editor")
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::enter))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::open))
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::new_document))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::find))
            .on_action(cx.listener(Self::find_workspace))
            .on_action(cx.listener(Self::find_next))
            .on_action(cx.listener(Self::complete))
            .on_action(cx.listener(Self::go_to_definition))
            .on_action(cx.listener(Self::show_commands))
            .on_action(cx.listener(Self::dismiss))
            .on_action(cx.listener(Self::toggle_preview))
            .on_action(cx.listener(Self::toggle_bottom_panel))
            .on_action(cx.listener(Self::toggle_assistant))
            .on_modifiers_changed(cx.listener(Self::modifiers_changed))
            .on_key_down(cx.listener(Self::tool_picker_key_down))
            .on_key_down(cx.listener(Self::settings_menu_key_down))
            .on_key_down(cx.listener(Self::header_menu_key_down))
            .on_key_down(cx.listener(Self::normal_key_down))
            .on_mouse_move(cx.listener(Self::resize_panels))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.stop_resize(cx)),
            )
            .child(self.render_header(cx))
            .child(self.render_main(window, cx))
            .child(self.render_footer(cx))
            .when(self.shell.command_palette_open, |root| {
                root.child(self.render_command_palette(cx))
            })
            .when_some(self.shell.tool_picker, |root, position| {
                root.child(self.render_tool_picker(position, cx))
            })
            .when(self.shell.settings_menu_open, |root| {
                root.child(self.render_settings_menu(cx))
            })
            .when(self.shell.header_menu_open, |root| {
                root.child(self.render_header_menu(cx))
            })
    }
}
