mod bottom_dock;
mod side_dock;
mod tool_dock;

use super::*;

impl Editor {
    /// メイン領域（tool island・center panel・side panel）を描画する。
    pub(super) fn render_main(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
        _compact_layout: bool,
    ) -> impl IntoElement {
        let document_tabs = self.session.tabs();
        let editor_focused = self.focus_handle.is_focused(window);
        let viewport_width = f32::from(window.viewport_size().width);
        let side_width = self
            .shell
            .side_panel
            .as_ref()
            .map(|_| self.shell.side_panel_width + theme::CANVAS_GAP)
            .unwrap_or_default();
        let center_width =
            (viewport_width - theme::CANVAS_GAP * 3.0 - self.shell.tool_island_width - side_width)
                .max(320.0);
        let tool_tabs = self
            .feature_registry
            .contributions(UiSlot::ToolDock)
            .into_iter()
            .enumerate()
            .filter_map(|(index, contribution)| {
                Some((
                    index,
                    contribution.view.clone()?,
                    self.locale.resolve(&contribution.title),
                ))
            })
            .collect::<Vec<_>>();

        div()
            .h(px(0.0))
            .w_full()
            .min_h(px(0.0))
            .flex()
            .flex_1()
            .px(px(theme::CANVAS_GAP))
            // --- tool island ---
            .child(
                div()
                    .w(px(self.shell.tool_island_width))
                    .flex()
                    .flex_col()
                    .flex_shrink_0()
                    .overflow_hidden()
                    .rounded(px(theme::ISLAND_RADIUS))
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::island())
                    .child(
                        div()
                            .h(px(39.0))
                            .flex_shrink_0()
                            .px(px(7.0))
                            .flex()
                            .items_center()
                            .gap_1()
                            .border_b_1()
                            .border_color(theme::border())
                            .children(tool_tabs.into_iter().map(|(index, view, label)| {
                                let active = self.shell.active_tool == view;
                                tool_tab(index, label, active).on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.select_tool(view.clone(), cx);
                                    },
                                ))
                            }))
                            .child(div().flex_1())
                            .child(
                                div()
                                    .id("tool-search")
                                    .size(px(28.0))
                                    .rounded(px(6.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(
                                        if self.shell.side_panel.as_ref().is_some_and(|view| {
                                            view.as_str() == id::VIEW_COMMAND_SEARCH
                                        }) {
                                            theme::accent_soft()
                                        } else {
                                            theme::island()
                                        },
                                    )
                                    .text_color(theme::muted())
                                    .hover(|style| {
                                        style.bg(theme::surface_hover()).text_color(theme::text())
                                    })
                                    .child(crate::components::Icon::new(
                                        crate::components::IconName::Search,
                                    ))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_quick_search(window, cx);
                                    })),
                            ),
                    )
                    .child(self.render_tool_content(cx)),
            )
            // --- resizer (tool island | center) ---
            .child(
                div()
                    .w(px(theme::CANVAS_GAP))
                    .flex_shrink_0()
                    .cursor(CursorStyle::ResizeLeftRight)
                    .hover(|style| style.bg(theme::accent_soft()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.start_resize(ResizeTarget::ToolIsland, cx);
                        }),
                    ),
            )
            // --- center panel ---
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
                    .when(self.shell.bottom_panel_open, |center| {
                        center
                            .child(
                                div()
                                    .h(px(theme::CANVAS_GAP))
                                    .flex_shrink_0()
                                    .cursor(CursorStyle::ResizeUpDown)
                                    .hover(|style| style.bg(theme::accent_soft()))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.start_resize(ResizeTarget::BottomPanel, cx);
                                        }),
                                    ),
                            )
                            .child(self.render_bottom_panel(cx))
                    }),
            )
            // --- resizer (center | side) + side panel ---
            .when(self.shell.side_panel.is_some(), |body| {
                body.child(
                    div()
                        .w(px(theme::CANVAS_GAP))
                        .flex_shrink_0()
                        .cursor(CursorStyle::ResizeLeftRight)
                        .hover(|style| style.bg(theme::accent_soft()))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.start_resize(ResizeTarget::SidePanel, cx);
                            }),
                        ),
                )
                .child(self.render_side_panel(cx))
            })
    }

    /// center panel 本体（タブ・エディタ）を描画する。
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
            .border_color(if editor_focused {
                rgb(0x3a3c58)
            } else {
                theme::border()
            })
            .bg(theme::island())
            // タブ行
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
                    .children(
                        document_tabs
                            .into_iter()
                            .enumerate()
                            .map(|(tab_index, tab)| {
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
                                    .bg(if tab.active {
                                        theme::surface()
                                    } else {
                                        theme::island()
                                    })
                                    .text_size(px(12.0))
                                    .text_color(if tab.active {
                                        theme::text()
                                    } else {
                                        theme::muted()
                                    })
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
                                    .child(if tab.dirty { "●" } else { "" })
                            }),
                    ),
            )
            // エディタ本体
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
                            .child(EditorElement {
                                editor: cx.entity(),
                            })
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
                                                .child("Markdown を始める"),
                                        )
                                        .child(
                                            div()
                                                .mb_1()
                                                .text_size(px(12.0))
                                                .text_color(theme::subtle())
                                                .child("新規作成するか、既存の文書を開きます"),
                                        )
                                        .child(
                                            quick_action("Workspace を開く…", "Ctrl O").on_click(
                                                cx.listener(|this, _, window, cx| {
                                                    this.open_file(window, cx);
                                                }),
                                            ),
                                        )
                                        .child(quick_action("新しい文書", "Ctrl N").on_click(
                                            cx.listener(|this, _, window, cx| {
                                                this.new_document(&New, window, cx);
                                            }),
                                        ))
                                        .child(
                                            quick_action("すべてのコマンド", "Ctrl Shift K")
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.open_quick_search(window, cx);
                                                })),
                                        ),
                                )
                            }),
                    ),
            )
    }
}
