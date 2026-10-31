use super::*;

use crate::workbench::{ViewInstanceKey, ViewScope, WorkbenchMode};

impl Shell {
    pub(crate) fn render_command_palette(
        &self,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
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

        crate::components::floating_panel(
            "command-palette",
            crate::components::SurfaceVariant::Popover,
            cx,
        )
        .absolute()
        .top(px(49.0))
        .left(relative(0.5))
        .ml(px(-220.0))
        .w(px(440.0))
        .p(tokens::spacing::SM)
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
                .bg(theme::colors(cx).background_secondary)
                .text_size(px(12.0))
                .text_color(theme::colors(cx).text_secondary)
                .child(crate::components::Icon::new(
                    crate::components::IconName::Search,
                ))
                .child("Commands"),
        )
        .children(
            commands
                .into_iter()
                .map(|(index, label, shortcut, command)| {
                    command_item(index, label, shortcut, cx).on_click(cx.listener(
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
                .text_color(theme::colors(cx).text_tertiary)
                .child("Esc で閉じる"),
        )
    }

    pub(crate) fn execute_command(
        &mut self,
        command: crate::extension_ui::CommandId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match command.as_str() {
            id::COMMAND_NEW_DOCUMENT => self.new_document(&New, window, cx),
            id::COMMAND_OPEN_WORKSPACE => {
                self.shell.command_palette_open = false;
                self.open_project(window, cx);
            }
            id::COMMAND_SAVE_DOCUMENT => {
                self.shell.command_palette_open = false;
                self.save_file(window, cx);
            }
            id::COMMAND_TOGGLE_PREVIEW => self.toggle_preview(&TogglePreview, window, cx),
            id::COMMAND_TOGGLE_BOTTOM_ZONE => {
                self.execute_toggle_zone(ZonePosition::Bottom, window, cx);
            }
            id::COMMAND_TOGGLE_LEFT_ZONE => {
                self.execute_toggle_zone(ZonePosition::Left, window, cx)
            }
            id::COMMAND_TOGGLE_RIGHT_ZONE => {
                self.execute_toggle_zone(ZonePosition::Right, window, cx)
            }
            id::COMMAND_TOGGLE_ASSISTANT => {
                self.toggle_assistant(&ToggleAssistant, window, cx);
            }
            id::COMMAND_TOGGLE_PANEL_CONTROLS => self.toggle_panel_controls(cx),
            id::COMMAND_WORKBENCH_IDE => {
                self.shell.command_palette_open = false;
                cx.emit(UiCommand::SetWorkbenchMode(WorkbenchMode::Ide));
            }
            id::COMMAND_WORKBENCH_AGENT => {
                self.shell.command_palette_open = false;
                cx.emit(UiCommand::SetWorkbenchMode(WorkbenchMode::Agent));
            }
            id::COMMAND_BIND_VIEW_SHARED => {
                self.emit_view_binding(ViewScope::SharedWorkbench, cx);
            }
            id::COMMAND_BIND_VIEW_TASK => {
                let Some(panel) = self
                    .shell
                    .focused_panel
                    .as_ref()
                    .and_then(|id| self.shell.layout.panel(id))
                else {
                    return;
                };
                if let Some(tab) = self.shell.visible_active(panel)
                    && let Some(view) = ViewInstanceKey::new(tab.stable_view_key())
                {
                    cx.emit(UiCommand::BindViewToSelectedTask(view));
                }
            }
            #[cfg(debug_assertions)]
            id::COMMAND_TOGGLE_INSPECTOR => self.toggle_inspector_window(window, cx),
            _ => {
                self.status = format!("Unknown command: {}", command.as_str());
                cx.notify();
            }
        }
    }

    fn emit_view_binding(&mut self, scope: ViewScope, cx: &mut Context<Self>) {
        let Some(panel) = self
            .shell
            .focused_panel
            .as_ref()
            .and_then(|id| self.shell.layout.panel(id))
        else {
            return;
        };
        let Some(view) = panel
            .active
            .as_ref()
            .and_then(|tab| ViewInstanceKey::new(tab.stable_view_key()))
        else {
            self.status = "所属を変更するViewが選択されていません".into();
            cx.notify();
            return;
        };
        self.shell.command_palette_open = false;
        cx.emit(UiCommand::SetViewBinding { view, scope });
    }
}
