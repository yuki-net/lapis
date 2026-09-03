use super::*;

struct PanelTabDragPreview {
    label: String,
}

impl Render for PanelTabDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(tokens::spacing::XS)
            .py(px(2.0))
            .rounded(tokens::radius::CONTROL)
            .bg(theme::colors().button_background_focused)
            .border_1()
            .border_color(theme::colors().button_border_focused)
            .text_color(theme::colors().text_primary)
            .text_size(tokens::typography::FONT_XS)
            .child(self.label.clone())
    }
}

impl Editor {
    pub(super) fn render_tool_panel_header(
        &self,
        panel: &PanelHost,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let position = panel.position;
        let is_panel_focused = self.shell.focused_panel == position;
        let scroll_state = self.scroll_states.panel_tabs(position);
        let max_offset = scroll_state.handle().max_offset();
        let offset = scroll_state.handle().offset();
        let has_overflow = max_offset.width > px(0.0);
        let show_right_gradient = has_overflow && -offset.x < max_offset.width - px(1.0);
        let show_left_gradient = has_overflow && -offset.x > px(1.0);
        let has_tabs = !panel.tabs.is_empty();

        let add_button = |id_suffix: &'static str| {
            div()
                .id((
                    "open-tool-tab",
                    panel_key(position) * 10 + if id_suffix == "inline" { 1 } else { 2 },
                ))
                .flex_shrink_0()
                .size(px(24.0))
                .rounded(tokens::radius::CONTROL)
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme::colors().text_secondary)
                .cursor(CursorStyle::PointingHand)
                .hover(|style| {
                    style
                        .bg(theme::colors().button_background_hover)
                        .text_color(theme::colors().text_primary)
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.shell.focused_panel = position;
                    this.open_tool_picker(position, cx);
                }))
                .child("+")
        };

        let mut tab_items: Vec<gpui::AnyElement> = panel
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                let label = match tab {
                    PanelTab::Tool(view) => self
                        .feature_registry
                        .panel_contributions(panel.position)
                        .into_iter()
                        .find(|contribution| contribution.view.as_ref() == Some(view))
                        .map(|contribution| self.locale.resolve(&contribution.title))
                        .unwrap_or_else(|| view.as_str().to_owned()),
                    PanelTab::Document(document_id) => self
                        .session
                        .tabs()
                        .into_iter()
                        .find(|document| document.id == *document_id)
                        .map(|document| document.display_name)
                        .unwrap_or_else(|| "Document".to_owned()),
                };
                let active = panel.active.as_ref() == Some(tab);
                let file_icon = match tab {
                    PanelTab::Document(document_id) => self
                        .session
                        .tabs()
                        .into_iter()
                        .find(|document| document.id == *document_id)
                        .map(|document| {
                            crate::features::files::display_info(
                                document
                                    .path
                                    .as_deref()
                                    .unwrap_or_else(|| std::path::Path::new("")),
                                FileEntryKind::File,
                                &self.problems.languages,
                            )
                            .icon
                        }),
                    PanelTab::Tool(_) => None,
                };
                let tab = tab.clone();
                let close_tab = tab.clone();
                let drag = DraggedPanelTab {
                    source_panel: position,
                    tab: tab.clone(),
                };

                let (bg_color, border_color, text_color) = if active && is_panel_focused {
                    (
                        theme::colors().button_background_focused,
                        theme::colors().button_border_focused,
                        theme::colors().text_primary,
                    )
                } else if active {
                    (
                        theme::colors().button_background_selected,
                        theme::colors().button_border_selected,
                        theme::colors().text_primary,
                    )
                } else {
                    (
                        theme::colors().button_background,
                        gpui::rgba(0x00000000),
                        theme::colors().text_secondary,
                    )
                };

                div()
                    .id(("panel-tab", panel_key(position) * 100 + index as u32))
                    .h(px(26.0))
                    .px(px(4.0))
                    .gap(px(3.0))
                    .rounded(tokens::radius::CONTROL)
                    .border_1()
                    .border_color(border_color)
                    .bg(bg_color)
                    .flex()
                    .items_center()
                    .text_size(tokens::typography::FONT_SM)
                    .text_color(text_color)
                    .hover(|style| {
                        if !active {
                            style
                                .bg(theme::colors().button_background_hover)
                                .text_color(theme::colors().text_primary)
                        } else {
                            style
                        }
                    })
                    .cursor(CursorStyle::PointingHand)
                    .on_drag(drag, |drag: &DraggedPanelTab, _, _, cx| {
                        cx.new(|_| PanelTabDragPreview {
                            label: format!("{:?}", drag.tab),
                        })
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.shell.focused_panel = position;
                        match tab.clone() {
                            PanelTab::Tool(view) => this.select_view(position, view, cx),
                            PanelTab::Document(document_id) => {
                                this.select_panel_tab(
                                    position,
                                    PanelTab::Document(document_id.clone()),
                                    cx,
                                );
                                this.persist_active_view();
                                if this.session.activate_document(&document_id) {
                                    this.restore_active_view();
                                    window.focus(&this.focus_handle);
                                    cx.notify();
                                }
                            }
                        }
                    }))
                    .when_some(file_icon, |tab_el, icon| {
                        tab_el.child(crate::components::FileIcon::new(icon).size(px(12.0)))
                    })
                    .child(label)
                    .child(
                        div()
                            .id(("close-panel-tab", panel_key(position) * 100 + index as u32))
                            .size(px(11.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(theme::colors().text_secondary)
                            .cursor(CursorStyle::PointingHand)
                            .hover(|style| style.text_color(theme::colors().text_primary))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_, _, _, cx| {
                                    cx.stop_propagation();
                                }),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.close_panel_tab(position, close_tab.clone(), window, cx);
                            }))
                            .child(Icon::new(IconName::X).size(px(9.0))),
                    )
                    .into_any_element()
            })
            .collect();

        if has_tabs && !has_overflow {
            tab_items.push(add_button("inline").into_any_element());
        }

        let mut tab_container = div()
            .relative()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .flex()
            .items_center()
            .child(
                scroll_viewport(
                    ("panel-tabs", panel_key(position)),
                    ScrollAxis::Horizontal,
                    scroll_state,
                    div()
                        .flex()
                        .items_center()
                        .gap(tokens::spacing::XS)
                        .children(tab_items),
                )
                .flex_1()
                .min_w(px(0.0)),
            );

        if show_left_gradient {
            tab_container =
                tab_container.child(div().absolute().top_0().bottom_0().left_0().w(px(20.0)).bg(
                    gpui::linear_gradient(
                        270.0,
                        gpui::linear_color_stop(gpui::rgba(0x00000000), 0.0),
                        gpui::linear_color_stop(theme::colors().background_secondary, 1.0),
                    ),
                ));
        }

        if show_right_gradient {
            tab_container = tab_container.child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .right_0()
                    .w(px(20.0))
                    .bg(gpui::linear_gradient(
                        90.0,
                        gpui::linear_color_stop(gpui::rgba(0x00000000), 0.0),
                        gpui::linear_color_stop(theme::colors().background_secondary, 1.0),
                    )),
            );
        }

        div()
            .h(px(34.0))
            .px(tokens::spacing::XS)
            .py(px(3.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(tokens::spacing::XS)
            .child(tab_container)
            .when(
                panel
                    .active_tool()
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
            .when(has_tabs && has_overflow, |bar| {
                bar.child(add_button("fixed"))
            })
    }
}
