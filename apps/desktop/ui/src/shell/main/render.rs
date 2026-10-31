use super::*;

impl Render for Shell {
    /// Builds the window shell and input handlers, clearing panel drop bounds for the new layout pass.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_content(window, cx)
    }
}

impl Shell {
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
            .bg(theme::colors(cx).background_primary)
            .text_color(theme::colors(cx).text_primary)
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
            .child(
                div()
                    .absolute()
                    .right(px(16.0))
                    .bottom(px(32.0))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(
                        cx.global::<crate::notification::Store>()
                            .popups_for(window.window_handle().window_id())
                            .into_iter()
                            .cloned()
                            .map(|event| {
                                let id = event.id;
                                crate::components::floating_panel(
                                    ("notification-popup", id),
                                    SurfaceVariant::Popover,
                                )
                                .w(px(320.0))
                                .p_3()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(format!(
                                    "{} · {}",
                                    event.severity.label(&self.locale),
                                    event.title
                                ))
                                .child(event.message)
                                .child(
                                    div()
                                        .id(("notification-popup-dismiss", id))
                                        .cursor_pointer()
                                        .child(self.t("notification.close"))
                                        .on_click(cx.listener(move |_, _, _, cx| {
                                            cx.global_mut::<crate::notification::Store>()
                                                .dismiss_popup(id)
                                        })),
                                )
                            }),
                    ),
            )
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
