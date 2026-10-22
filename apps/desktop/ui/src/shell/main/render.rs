use super::*;

impl Render for Shell {
    /// Builds the window shell and input handlers, clearing panel drop bounds for the new layout pass.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(all(feature = "hot-reload", debug_assertions))]
        {
            subsecond::HotFn::current(Self::render_hot).call((self, window, cx))
        }

        #[cfg(not(all(feature = "hot-reload", debug_assertions)))]
        {
            self.render_content(window, cx)
        }
    }
}

impl Shell {
    #[cfg(all(feature = "hot-reload", debug_assertions))]
    #[inline(never)]
    fn render_hot(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        self.render_content(window, cx).into_any_element()
    }

    fn render_content(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.shell
            .layout
            .finish_due_closes(std::time::Instant::now());
        self.zone_panel_bounds.borrow_mut().clear();
        self.panel_bounds.borrow_mut().clear();
        self.zone_bounds.borrow_mut().clear();
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(theme::colors().background_primary)
            .text_color(theme::colors().text_primary)
            .track_focus(&self.focus_handle(cx))
            .key_context("Shell")
            .on_action(cx.listener(Self::open))
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::save_as))
            .on_action(cx.listener(Self::new_document))
            .on_action(cx.listener(Self::show_commands))
            .on_action(cx.listener(Self::dismiss))
            .on_action(cx.listener(Self::toggle_preview))
            .on_action(cx.listener(Self::toggle_bottom_zone))
            .on_action(cx.listener(Self::toggle_left_zone))
            .on_action(cx.listener(Self::toggle_right_zone))
            .on_action(cx.listener(Self::toggle_assistant))
            .on_key_down(cx.listener(Self::tool_picker_key_down))
            .on_key_down(cx.listener(Self::settings_menu_key_down))
            .on_key_down(cx.listener(Self::header_menu_key_down))
            .on_mouse_move(cx.listener(Self::resize_panels))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.stop_resize(cx)),
            )
            .child(self.tab_drag_events(cx))
            .child(self.render_header(cx))
            .child(self.render_main(window, cx))
            .child(self.render_footer(cx))
            .when(self.shell.command_palette_open, |root| {
                root.child(self.render_command_palette(cx))
            })
            .when_some(self.shell.tool_picker.clone(), |root, target| {
                root.child(self.render_tool_picker(target, cx))
            })
            .when(self.shell.settings_menu_open, |root| {
                root.child(self.render_settings_menu(cx))
            })
            .when(self.shell.header_menu_open, |root| {
                root.child(self.render_header_menu(cx))
            })
    }
}
