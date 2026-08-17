use super::*;

impl Editor {
    pub(super) fn render_empty_panel(
        &self,
        position: PanelPosition,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let title = match position {
            PanelPosition::Left => "Left Panel",
            PanelPosition::Bottom => "Bottom Panel",
            PanelPosition::Right => "Right Panel",
            PanelPosition::Main => "Main Panel",
        };
        panel_empty_state(
            "▤",
            title,
            "Open new tool or drag-n-drop tool from other panels",
        )
        .child(
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
                    this.open_tool_picker(position, cx);
                }))
                .child("Open Tool"),
        )
    }

    pub(super) fn render_document_content(
        &self,
        document_id: &lapis_editor_core::DocumentId,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let active = self.session.active_document_id() == Some(document_id);
        if active {
            div()
                .h(px(0.0))
                .min_h(px(0.0))
                .flex()
                .flex_col()
                .flex_1()
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
                                    .child(
                                        div()
                                            .text_size(px(18.0))
                                            .text_color(theme::text())
                                            .child("Open a file or project"),
                                    )
                                    .child(
                                        div()
                                            .mb_1()
                                            .text_size(px(12.0))
                                            .text_color(theme::subtle())
                                            .child("Choose a project to restore its last workspace, or open a file to start here."),
                                    )
                                    .child(quick_action("Open file", "Ctrl O").on_click(
                                        cx.listener(|this, _, window, cx| this.open_file(window, cx)),
                                    ))
                                    .child(quick_action("Open project", "").on_click(
                                        cx.listener(|this, _, window, cx| this.open_project(window, cx)),
                                    ))
                                    .child(quick_action("New file", "Ctrl N").on_click(
                                        cx.listener(|this, _, window, cx| {
                                            this.new_document(&New, window, cx)
                                        }),
                                    )),
                            )
                        }),
                )
        } else {
            panel_empty_state("F", "Document", "Select the document tab to edit")
        }
    }

    pub(super) fn render_tool_content(&self, view: &ViewId, cx: &mut Context<Self>) -> gpui::Div {
        match view.as_str() {
            id::VIEW_FILES => self.render_files_content(cx),
            id::VIEW_SEARCH => self.render_search_content(cx),
            id::VIEW_GIT => self.render_git_content(cx),
            id::VIEW_HISTORY => self.render_history_content(),
            id::VIEW_PREVIEW => self.render_preview_content(),
            id::VIEW_ASSISTANT => self.render_assistant_content(cx),
            id::VIEW_TERMINAL => crate::features::terminal::render_content(&self.terminal),
            id::VIEW_PROBLEMS => crate::features::problems::render_content(&self.problems),
            id::VIEW_OUTPUT => crate::features::problems::render_output(&self.status),
            id::VIEW_COMMAND_SEARCH => div().flex_1().child(self.quick_search.clone()),
            id::VIEW_SETTINGS => self.render_settings_content(),
            _ => panel_empty_state("?", "Unknown view", view.as_str().to_owned()),
        }
    }

    pub(super) fn render_settings_content(&self) -> gpui::Div {
        div()
            .flex_1()
            .w_full()
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .text_center()
            .child(Icon::new(IconName::Settings))
            .child(
                div()
                    .text_size(px(18.0))
                    .text_color(theme::text())
                    .child("Settings"),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme::muted())
                    .child("Settings will be available here."),
            )
    }

    pub(super) fn render_history_content(&self) -> gpui::Div {
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

    pub(super) fn render_preview_content(&self) -> gpui::Div {
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
                div().children(crate::features::preview::preview_lines(&self.session))
            })
    }
}
