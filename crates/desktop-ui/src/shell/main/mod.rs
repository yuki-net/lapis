use super::*;
use crate::{
    extension_ui::PanelPosition,
    shell::{PanelHost, ResizeTarget},
};

impl Editor {
    /// 四つの panel を描画する。中央は Document タブを収容し、他の panel は Tool タブを収容する。
    pub(super) fn render_main(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
        _compact_layout: bool,
    ) -> impl IntoElement {
        let document_tabs = self.session.tabs();
        let editor_focused = self.focus_handle.is_focused(window);
        let viewport_width = f32::from(window.viewport_size().width);
        let left_width = self
            .shell
            .left_panel
            .open
            .then_some(self.shell.left_panel.size + theme::CANVAS_GAP)
            .unwrap_or_default();
        let right_width = self
            .shell
            .right_panel
            .open
            .then_some(self.shell.right_panel.size + theme::CANVAS_GAP)
            .unwrap_or_default();
        let center_width =
            (viewport_width - theme::CANVAS_GAP * 2.0 - left_width - right_width).max(320.0);

        div()
            .h(px(0.0))
            .w_full()
            .min_h(px(0.0))
            .flex()
            .flex_1()
            .px(px(theme::CANVAS_GAP))
            .when(self.shell.left_panel.open, |body| {
                body.child(self.render_tool_panel(&self.shell.left_panel, cx))
                    .child(self.render_resize_handle(ResizeTarget::LeftPanel, false, cx))
            })
            .child(
                div()
                    .w(px(center_width))
                    .h_full()
                    .min_h(px(0.0))
                    .min_w(px(0.0))
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .child(self.render_center_panel(cx, editor_focused, document_tabs))
                    .when(self.shell.bottom_panel.open, |center| {
                        center
                            .child(self.render_resize_handle(ResizeTarget::BottomPanel, true, cx))
                            .child(self.render_tool_panel(&self.shell.bottom_panel, cx))
                    }),
            )
            .when(self.shell.right_panel.open, |body| {
                body.child(self.render_resize_handle(ResizeTarget::RightPanel, false, cx))
                    .child(self.render_tool_panel(&self.shell.right_panel, cx))
            })
    }

    fn render_resize_handle(
        &self,
        target: ResizeTarget,
        horizontal: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let mut handle = div()
            .flex_shrink_0()
            .hover(|style| style.bg(theme::accent_soft()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| this.start_resize(target, cx)),
            );
        if horizontal {
            handle = handle
                .h(px(theme::CANVAS_GAP))
                .cursor(CursorStyle::ResizeUpDown);
        } else {
            handle = handle
                .w(px(theme::CANVAS_GAP))
                .cursor(CursorStyle::ResizeLeftRight);
        }
        handle
    }

    fn render_tool_panel(&self, panel: &PanelHost, cx: &mut Context<Self>) -> gpui::Div {
        let is_bottom = panel.position == PanelPosition::Bottom;
        let size = if is_bottom {
            div().h(px(panel.size)).w_full()
        } else {
            div().w(px(panel.size)).h_full()
        };
        size.flex_shrink_0()
            .overflow_hidden()
            .rounded(px(theme::ISLAND_RADIUS))
            .border_1()
            .border_color(theme::border())
            .bg(theme::island())
            .flex()
            .flex_col()
            .child(self.render_tool_panel_header(panel, cx))
            .child(
                panel
                    .active
                    .as_ref()
                    .map(|view| self.render_tool_content(view, cx))
                    .unwrap_or_else(|| self.render_empty_panel(panel.position, cx)),
            )
    }

    fn render_tool_panel_header(&self, panel: &PanelHost, cx: &mut Context<Self>) -> gpui::Div {
        let tabs = panel.tabs.iter().enumerate().map(|(index, view)| {
            let label = self
                .feature_registry
                .panel_contributions(panel.position)
                .into_iter()
                .find(|contribution| contribution.view.as_ref() == Some(view))
                .map(|contribution| self.locale.resolve(&contribution.title))
                .unwrap_or_else(|| view.as_str().to_owned());
            let active = panel.active.as_ref() == Some(view);
            let view = view.clone();
            let position = panel.position;
            div()
                .id(("panel-tab", panel_key(position) * 100 + index as u32))
                .h(px(30.0))
                .px_2()
                .rounded_t(px(6.0))
                .flex()
                .items_center()
                .bg(if active {
                    theme::surface()
                } else {
                    theme::island()
                })
                .text_size(px(11.0))
                .text_color(if active {
                    theme::text()
                } else {
                    theme::muted()
                })
                .hover(|style| style.bg(theme::surface_hover()).text_color(theme::text()))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.select_view(position, view.clone(), cx);
                }))
                .child(label)
        });
        let position = panel.position;
        div()
            .h(px(39.0))
            .flex_shrink_0()
            .px_2()
            .border_b_1()
            .border_color(theme::border())
            .flex()
            .items_center()
            .gap_1()
            .children(tabs)
            .child(div().flex_1())
            .when(
                panel
                    .active
                    .as_ref()
                    .is_some_and(|view| view.as_str() == id::VIEW_TERMINAL),
                |bar| {
                    bar.child(
                        task_action_button("Start", false)
                            .on_click(cx.listener(|this, _, _, cx| this.start_terminal(cx))),
                    )
                    .child(
                        task_action_button("Send clipboard", false).on_click(
                            cx.listener(|this, _, _, cx| this.send_terminal_clipboard(cx)),
                        ),
                    )
                },
            )
            .child(
                div()
                    .id(("close-panel", panel_key(position)))
                    .size(px(25.0))
                    .rounded(px(5.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(theme::muted())
                    .hover(|style| style.bg(theme::surface_hover()).text_color(theme::text()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(panel) = this.shell.panel_mut(position) {
                            panel.close();
                        }
                        this.refresh_feature_activation();
                        cx.notify();
                    }))
                    .child("×"),
            )
    }

    fn render_empty_panel(&self, position: PanelPosition, cx: &mut Context<Self>) -> gpui::Div {
        let title = match position {
            PanelPosition::Left => "Left Panel",
            PanelPosition::Bottom => "Bottom Panel",
            PanelPosition::Right => "Right Panel",
            PanelPosition::Center => "Center Panel",
        };
        let default_view = self
            .feature_registry
            .panel_contributions(position)
            .into_iter()
            .find_map(|contribution| contribution.view.clone());
        panel_empty_state("▤", title, "Open a tool or drag a tool from another panel").when_some(
            default_view,
            |empty, view| {
                empty.child(
                    div()
                        .id(("open-tool", panel_key(position)))
                        .mt_2()
                        .px_2()
                        .py_1()
                        .rounded(px(5.0))
                        .border_1()
                        .border_color(theme::border())
                        .text_size(px(11.0))
                        .hover(|style| style.bg(theme::surface_hover()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.select_view(position, view.clone(), cx);
                        }))
                        .child("Open Tool"),
                )
            },
        )
    }

    fn render_tool_content(&self, view: &ViewId, cx: &mut Context<Self>) -> gpui::Div {
        match view.as_str() {
            id::VIEW_FILES => self.render_files_content(cx),
            id::VIEW_SEARCH => self.render_search_content(cx),
            id::VIEW_GIT => self.render_git_content(cx),
            id::VIEW_HISTORY => self.render_history_content(),
            id::VIEW_PREVIEW => self.render_preview_content(),
            id::VIEW_ASSISTANT => self.render_assistant_content(cx),
            id::VIEW_TERMINAL => self.render_terminal_content(),
            id::VIEW_PROBLEMS => self.render_problems_content(),
            id::VIEW_OUTPUT => self.render_output_content(),
            id::VIEW_COMMAND_SEARCH => div().flex_1().child(self.quick_search.clone()),
            _ => panel_empty_state("?", "Unknown view", view.as_str().to_owned()),
        }
    }

    fn render_history_content(&self) -> gpui::Div {
        div()
            .flex()
            .flex_col()
            .flex_1()
            .p(px(10.0))
            .gap_2()
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(theme::subtle())
                    .child("DOCUMENT HISTORY"),
            )
            .child(
                div()
                    .p_2()
                    .rounded(px(6.0))
                    .bg(theme::surface())
                    .text_size(px(12.0))
                    .child(format!("Revision {}", self.session.revision())),
            )
    }

    fn render_preview_content(&self) -> gpui::Div {
        div()
            .h(px(0.0))
            .min_h(px(0.0))
            .flex_1()
            .overflow_hidden()
            .p_4()
            .flex()
            .flex_col()
            .gap_2()
            .child(if self.session.is_empty() {
                panel_empty_state(
                    "▱",
                    "No preview available",
                    "Open a Markdown document to preview it",
                )
            } else {
                div().children(self.preview_lines())
            })
    }

    fn render_center_panel(
        &self,
        cx: &mut Context<Self>,
        editor_focused: bool,
        document_tabs: Vec<lapis_app_services::DocumentTab>,
    ) -> impl IntoElement {
        div()
            .w_full()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .flex_1()
            .relative()
            .overflow_hidden()
            .rounded(px(theme::ISLAND_RADIUS))
            .border_1()
            .border_color(if editor_focused { rgb(0x3a3c58) } else { theme::border() })
            .bg(theme::island())
            .child(
                div()
                    .h(px(39.0))
                    .flex_shrink_0()
                    .px(px(7.0))
                    .flex()
                    .items_end()
                    .gap_1()
                    .id("editor-tabs-scroll")
                    .overflow_x_scroll()
                    .border_b_1()
                    .border_color(theme::border())
                    .children(document_tabs.into_iter().enumerate().map(|(tab_index, tab)| {
                        let tab_id = tab.id.clone();
                        div()
                            .id(("editor-tab", tab_index))
                            .h(px(31.0))
                            .w(px(180.0))
                            .flex_shrink_0()
                            .px_2()
                            .rounded_t(px(6.0))
                            .flex()
                            .items_center()
                            .gap_2()
                            .bg(if tab.active { theme::surface() } else { theme::island() })
                            .text_size(px(12.0))
                            .text_color(if tab.active { theme::text() } else { theme::muted() })
                            .hover(|style| style.bg(theme::surface_hover()))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.persist_active_view();
                                if this.session.activate_document(&tab_id) {
                                    this.restore_active_view();
                                    window.focus(&this.focus_handle);
                                    cx.notify();
                                }
                            }))
                            .child(file_badge("F", theme::orange()))
                            .child(tab.display_name)
                            .child(div().flex_1())
                            .child(if tab.dirty { "•" } else { "" })
                    })),
            )
            .child(
                div()
                    .h(px(0.0))
                    .min_h(px(0.0))
                    .flex_1()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id("source-scroll")
                            .h(px(0.0))
                            .min_h(px(0.0))
                            .flex_1()
                            .overflow_scroll()
                            .track_scroll(&self.editor_scroll)
                            .relative()
                            .px(px(18.0))
                            .py(px(10.0))
                            .cursor(CursorStyle::IBeam)
                            .text_size(px(14.0))
                            .text_color(theme::text())
                            .on_mouse_down(MouseButton::Left, cx.listener(Self::editor_mouse_down))
                            .on_mouse_move(cx.listener(Self::editor_mouse_move))
                            .on_mouse_up(MouseButton::Left, cx.listener(Self::editor_mouse_up))
                            .child(EditorElement { editor: cx.entity() })
                            .when(self.session.is_empty(), |canvas| {
                                canvas.child(
                                    div()
                                        .absolute()
                                        .top(px(72.0))
                                        .left(px(54.0))
                                        .w(px(330.0))
                                        .flex()
                                        .flex_col()
                                        .gap_2()
                                        .child(div().text_size(px(18.0)).text_color(theme::text()).child("Open a file or project"))
                                        .child(div().mb_1().text_size(px(12.0)).text_color(theme::subtle()).child("Choose a project to restore its last workspace, or open a file to start here."))
                                        .child(quick_action("Open file", "Ctrl O").on_click(cx.listener(|this, _, window, cx| this.open_file(window, cx))))
                                        .child(quick_action("Open project", "").on_click(cx.listener(|this, _, window, cx| this.open_project(window, cx))))
                                        .child(quick_action("New file", "Ctrl N").on_click(cx.listener(|this, _, window, cx| this.new_document(&New, window, cx)))),
                                )
                            }),
                    ),
            )
    }
}

const fn panel_key(position: PanelPosition) -> u32 {
    match position {
        PanelPosition::Left => 1,
        PanelPosition::Center => 2,
        PanelPosition::Bottom => 3,
        PanelPosition::Right => 4,
    }
}
