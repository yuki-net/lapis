use super::*;

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let display_name = self.session.display_name();
        let document_is_empty = self.session.is_empty();
        let document_tabs = self.session.tabs();
        let editor_focused = self.focus_handle.is_focused(window);
        let compact_layout = f32::from(window.viewport_size().width) < 1080.0;
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
        let status_is_error = self.status.contains("失敗");
        let maximize_label = self.icon_theme.resolve_name(if window.is_maximized() {
            icons::id::RESTORE
        } else {
            icons::id::MAXIMIZE
        });
        let (line, column) = self.cursor_line_column();
        let active_conversation = self.conversation.session.active_id().clone();
        let conversations = self
            .conversation
            .session
            .records()
            .iter()
            .take(if compact_layout { 2 } else { 4 })
            .enumerate()
            .map(|(index, record)| (index, record.id.clone()))
            .collect::<Vec<_>>();
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
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(theme::canvas())
            .text_color(theme::text())
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
            .on_key_down(cx.listener(Self::normal_key_down))
            .on_mouse_move(cx.listener(Self::resize_panels))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.stop_resize(cx)),
            )
            .child(
                div()
                    .h(px(theme::TITLE_BAR_HEIGHT))
                    .w_full()
                    .flex_shrink_0()
                    .pl(px(12.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .bg(theme::title_bar())
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .w(px(if compact_layout { 200.0 } else { 320.0 }))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .size(px(22.0))
                                    .rounded(px(6.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(theme::accent())
                                    .text_color(rgb(0xffffff))
                                    .text_size(px(11.0))
                                    .child("L"),
                            )
                            .child(top_icon(
                                self.icon_theme.resolve_name(icons::id::MENU),
                                false,
                            ))
                            .child(top_icon(
                                self.icon_theme.resolve_name(icons::id::TOOL_DOCK),
                                true,
                            ))
                            .child(
                                top_icon(
                                    self.icon_theme.resolve_name(icons::id::SIDE_DOCK),
                                    self.shell.side_panel.as_ref().is_some_and(|view| {
                                        view.as_str() == id::VIEW_PREVIEW
                                    }),
                                )
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.toggle_preview(&TogglePreview, window, cx);
                                })),
                            )
                            .child(
                                top_icon(
                                    self.icon_theme.resolve_name(icons::id::BOTTOM_DOCK),
                                    self.shell.bottom_panel_open,
                                ).on_click(cx.listener(
                                    |this, _, window, cx| {
                                        this.toggle_bottom_panel(&ToggleBottomPanel, window, cx);
                                    },
                                )),
                            )
                            .children(conversations.into_iter().map(|(index, id)| {
                                let active = id == active_conversation;
                                div()
                                    .id(("conversation", index))
                                    .h(px(24.0))
                                    .min_w(px(24.0))
                                    .px_1()
                                    .rounded(px(6.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_size(px(10.0))
                                    .text_color(if active {
                                        rgb(0xbfc0ff)
                                    } else {
                                        theme::muted()
                                    })
                                    .bg(if active {
                                        theme::accent_soft()
                                    } else {
                                        theme::title_bar()
                                    })
                                    .hover(|style| style.bg(theme::surface_hover()))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.switch_conversation(id.clone(), cx);
                                    }))
                                    .child((index + 1).to_string())
                            }))
                            .child(
                                div()
                                    .id("new-conversation")
                                    .size(px(24.0))
                                    .rounded(px(6.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_size(px(13.0))
                                    .text_color(theme::muted())
                                    .hover(|style| style.bg(theme::surface_hover()))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.create_conversation(cx);
                                    }))
                                    .child("+"),
                            ),
                    )
                    .child(
                        div()
                            .w(px(0.0))
                            .h(px(
                                theme::TITLE_BAR_HEIGHT - theme::WINDOW_RESIZE_BORDER_HEIGHT,
                            ))
                            .mt(px(theme::WINDOW_RESIZE_BORDER_HEIGHT))
                            .flex_1()
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap_2()
                            .text_size(px(12.0))
                            .window_control_area(WindowControlArea::Drag)
                            .child(
                                div()
                                    .text_color(theme::muted())
                                    .child(self.session.workspace_name().to_owned()),
                            )
                            .child(div().text_color(theme::subtle()).child("›"))
                            .child(div().text_color(theme::subtle()).child("local")),
                    )
                    .child(
                        div()
                            .w(px(if compact_layout { 150.0 } else { 220.0 }))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap_1()
                            .child(
                                div()
                                    .id("assistant-toggle")
                                    .h(px(27.0))
                                    .px_2()
                                    .rounded(px(6.0))
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .text_size(px(11.0))
                                    .text_color(rgb(0x8da8ff))
                                    .bg(if self.shell.side_panel.as_ref().is_some_and(|view| {
                                        view.as_str() == id::VIEW_ASSISTANT
                                    }) {
                                        theme::accent_soft()
                                    } else {
                                        theme::title_bar()
                                    })
                                    .hover(|style| style.bg(theme::surface_hover()))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_assistant(&ToggleAssistant, window, cx);
                                    }))
                                    .child(self.icon_theme.resolve_name(icons::id::ASSISTANT))
                                    .child("Note"),
                            )
                            .child(
                                top_icon(self.icon_theme.resolve_name(icons::id::SEARCH), false)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_quick_search(window, cx);
                                    })),
                            )
                            .child(top_icon(
                                self.icon_theme.resolve_name(icons::id::RUN),
                                false,
                            )),
                    )
                    .child(
                        div()
                            .h_full()
                            .flex_shrink_0()
                            .flex()
                            .items_end()
                            .child(window_control_button(
                                "window-minimize",
                                self.icon_theme.resolve_name(icons::id::MINIMIZE),
                                WindowControlArea::Min,
                                false,
                            ))
                            .child(window_control_button(
                                "window-maximize",
                                maximize_label,
                                WindowControlArea::Max,
                                false,
                            ))
                            .child(window_control_button(
                                "window-close",
                                self.icon_theme.resolve_name(icons::id::CLOSE),
                                WindowControlArea::Close,
                                true,
                            )),
                    ),
            )
            .child(
                div()
                    .h(px(0.0))
                    .w_full()
                    .min_h(px(0.0))
                    .flex()
                    .flex_1()
                    .p(px(theme::CANVAS_GAP))
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
                                        top_icon(
                                            self.icon_theme.resolve_name(icons::id::SEARCH),
                                            self.shell.side_panel.as_ref().is_some_and(|view| {
                                                view.as_str() == id::VIEW_COMMAND_SEARCH
                                            }),
                                        )
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.open_quick_search(window, cx);
                                        })),
                                    ),
                            )
                            .child(self.render_tool_content(cx)),
                    )
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
                            .child(
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
                                                let id = tab.id.clone();
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
                                                        if this.session.activate_document(&id) {
                                                            this.restore_active_view();
                                                            window.focus(&this.focus_handle);
                                                            cx.notify();
                                                        }
                                                    }))
                                                    .child(file_badge("F", theme::orange()))
                                                    .child(tab.display_name)
                                                    .child(div().flex_1())
                                                    .child(if tab.dirty { "●" } else { "" })
                                            })),
                                    )
                                    .child(
                                        div()
                                            .h(px(31.0))
                                            .flex_shrink_0()
                                            .px(px(14.0))
                                            .flex()
                                            .items_center()
                                            .gap_3()
                                            .text_size(px(12.0))
                                            .text_color(theme::subtle())
                                            .child("lapis")
                                            .child("›")
                                            .child(
                                                div()
                                                    .text_color(theme::muted())
                                                    .child(display_name.clone()),
                                            )
                                            .child("·")
                                            .child(
                                                div().text_color(rgb(0x8da8ff)).child("✓ Note"),
                                            )
                                            .child(format!("R{}", self.session.revision()))
                                            .child(format!("Ln {line}, Col {column}"))
                                            .child("·")
                                            .child(
                                                div()
                                                    .text_color(if status_is_error {
                                                        rgb(0xf18f96)
                                                    } else {
                                                        theme::subtle()
                                                    })
                                                    .child(self.status.clone()),
                                            ),
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
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(Self::editor_mouse_down),
                                                    )
                                                    .on_mouse_move(cx.listener(Self::editor_mouse_move))
                                                    .on_mouse_up(
                                                        MouseButton::Left,
                                                        cx.listener(Self::editor_mouse_up),
                                                    )
                                                    .child(EditorElement {
                                                        editor: cx.entity(),
                                                    })
                                                    .when(document_is_empty, |canvas| {
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
                                                                    quick_action(
                                                                        "Workspace を開く…",
                                                                        "Ctrl O",
                                                                    )
                                                                    .on_click(cx.listener(
                                                                        |this, _, window, cx| {
                                                                            this.open_file(
                                                                                window, cx,
                                                                            );
                                                                        },
                                                                    )),
                                                                )
                                                                .child(
                                                                    quick_action(
                                                                        "新しい文書",
                                                                        "Ctrl N",
                                                                    )
                                                                    .on_click(cx.listener(
                                                                        |this, _, window, cx| {
                                                                            this.new_document(
                                                                                &New, window, cx,
                                                                            );
                                                                        },
                                                                    )),
                                                                )
                                                                .child(
                                                                    quick_action(
                                                                        "すべてのコマンド",
                                                                        "Ctrl Shift K",
                                                                    )
                                                                    .on_click(cx.listener(
                                                                        |this, _, window, cx| {
                                                                            this.open_quick_search(window, cx);
                                                                        },
                                                                    )),
                                                                ),
                                                        )
                                                    }),
                                            ),
                                    ),
                            )
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
                                                    this.start_resize(
                                                        ResizeTarget::BottomPanel,
                                                        cx,
                                                    );
                                                }),
                                            ),
                                    )
                                    .child(self.render_bottom_panel(cx))
                            }),
                    )
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
                    }),
            )
            .when(self.shell.command_palette_open, |root| {
                root.child(self.render_command_palette(cx))
            })
    }
}
