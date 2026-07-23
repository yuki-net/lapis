use super::*;

impl Editor {
    pub(super) fn render_command_palette(&self, cx: &mut Context<Self>) -> gpui::Div {
        let commands = self
            .feature_registry
            .contributions(UiSlot::CommandPalette)
            .into_iter()
            .enumerate()
            .filter_map(|(index, contribution)| {
                let command = contribution.command.clone()?;
                Some((
                    index,
                    self.locale.resolve(&contribution.title),
                    self.keymap.shortcut_label(&command),
                    command,
                ))
            })
            .collect::<Vec<_>>();

        div()
            .absolute()
            .top(px(49.0))
            .left(relative(0.5))
            .ml(px(-220.0))
            .w(px(440.0))
            .p(px(8.0))
            .rounded(px(9.0))
            .border_1()
            .border_color(rgb(0x3d4050))
            .bg(theme::surface())
            .shadow_lg()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .h(px(31.0))
                    .px_2()
                    .rounded(px(6.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .bg(theme::island())
                    .text_size(px(12.0))
                    .text_color(theme::muted())
                    .child(crate::components::Icon::new(
                        crate::components::IconName::Search,
                    ))
                    .child("Commands"),
            )
            .children(
                commands
                    .into_iter()
                    .map(|(index, label, shortcut, command)| {
                        command_item(index, label, shortcut).on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.execute_command(command.clone(), window, cx);
                            },
                        ))
                    }),
            )
            .child(
                div()
                    .pt_1()
                    .px_2()
                    .text_size(px(10.0))
                    .text_color(theme::subtle())
                    .child("Esc で閉じる"),
            )
    }

    pub(super) fn execute_command(
        &mut self,
        command: crate::extension_ui::CommandId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match command.as_str() {
            id::COMMAND_NEW_DOCUMENT => self.new_document(&New, window, cx),
            id::COMMAND_OPEN_WORKSPACE => {
                self.shell.command_palette_open = false;
                self.open_file(window, cx);
            }
            id::COMMAND_SAVE_DOCUMENT => {
                self.shell.command_palette_open = false;
                self.save_file(window, cx);
            }
            id::COMMAND_TOGGLE_PREVIEW => self.toggle_preview(&TogglePreview, window, cx),
            id::COMMAND_TOGGLE_BOTTOM => {
                self.toggle_bottom_panel(&ToggleBottomPanel, window, cx);
            }
            id::COMMAND_TOGGLE_ASSISTANT => {
                self.toggle_assistant(&ToggleAssistant, window, cx);
            }
            #[cfg(debug_assertions)]
            id::COMMAND_TOGGLE_INSPECTOR => {
                self.shell.command_palette_open = false;
                cx.defer_in(window, |this, window, cx| {
                    this.status = match crate::devtools::toggle_inspector(window, cx) {
                        Ok(true) => "Inspectorを別ウィンドウで開きました".to_owned(),
                        Ok(false) => "Inspectorを閉じました".to_owned(),
                        Err(error) => error,
                    };
                    cx.notify();
                });
            }
            _ => {
                self.status = format!("Unknown command: {}", command.as_str());
                cx.notify();
            }
        }
    }
}
