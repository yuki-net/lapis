use super::*;

impl Shell {
    pub(super) fn tab_title(&self, position: ZonePosition, tab: &PanelTab) -> String {
        match tab {
            PanelTab::Tool { view, .. } => self
                .feature_registry
                .zone_contributions(position)
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
        }
    }
    /// Renders scrollable tabs with selection, close, context menu, and drag handlers.
    /// Detached windows omit the tool-add button.
    pub(super) fn render_tool_panel_header(
        &self,
        position: ZonePosition,
        panel: &PanelHost,
        panel_id: &PanelId,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let is_panel_focused = self.shell.focused_panel.as_ref() == Some(panel_id);
        let scroll_state = self.scroll_states.panel_tabs(panel_id);
        let max_offset = crate::gpui_compat::max_offset(&scroll_state.handle());
        let offset = scroll_state.handle().offset();
        let has_overflow = max_offset.width > px(0.0);
        let show_right_gradient = has_overflow && -offset.x < max_offset.width - px(1.0);
        let show_left_gradient = has_overflow && -offset.x > px(1.0);

        let add_button = |id_suffix: &'static str| {
            let target = panel_id.clone();
            div()
                .id(ElementId::Name(
                    format!("open-tool-{}-{id_suffix}", panel_id.as_str()).into(),
                ))
                .flex_shrink_0()
                .size(px(24.0))
                .rounded(tokens::radius::CONTROL)
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme::colors(cx).text_secondary)
                .cursor(CursorStyle::PointingHand)
                .hover(|style| {
                    style
                        .bg(theme::colors(cx).button_background_hover)
                        .text_color(theme::colors(cx).text_primary)
                })
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    this.open_tool_picker(target.clone(), event.position(), window, cx);
                }))
                .child("+")
        };

        let mut tab_items: Vec<gpui::AnyElement> = panel
            .tabs
            .iter()
            .filter(|tab| self.shell.view_visible(tab))
            .enumerate()
            .map(|(index, tab)| {
                let label = self.tab_title(position, tab);
                let active = self.shell.visible_active(panel) == Some(tab);
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
                                &self.languages,
                            )
                            .icon
                        }),
                    PanelTab::Tool { .. } => None,
                };
                let tab = tab.clone();
                let close_tab = tab.clone();
                let context_tab = tab.clone();
                let context_panel = panel_id.clone();
                let pinned = panel.pinned.contains(&tab);
                let debug_id = format!("panel-tab-{}-{index}", panel_id.as_str());
                let select_panel = panel_id.clone();
                let close_panel_id = panel_id.clone();
                let drag_shell = cx.entity().downgrade();
                let drag_label = label.clone();
                let measured = std::rc::Rc::new(std::cell::Cell::new(None::<gpui::Bounds<Pixels>>));
                let measure_bounds = measured.clone();
                let tab_bounds = self.tab_bounds.clone();
                let measured_tab = tab.clone();
                let drag = DraggedPanelTab {
                    group: self.group,
                    source_host: self.host,
                    source_panel: panel_id.clone(),
                    tab: tab.clone(),
                };

                let (bg_color, border_color, text_color) = if active && is_panel_focused {
                    (
                        theme::colors(cx).button_background_focused,
                        theme::colors(cx).button_border_focused,
                        theme::colors(cx).text_primary,
                    )
                } else if active {
                    (
                        theme::colors(cx).button_background_selected,
                        theme::colors(cx).button_border_selected,
                        theme::colors(cx).text_primary,
                    )
                } else {
                    (
                        theme::colors(cx).button_background,
                        gpui::rgba(0x00000000),
                        theme::colors(cx).text_secondary,
                    )
                };

                div()
                    .id(ElementId::Name(
                        format!("panel-tab-{}-{index}", panel_id.as_str()).into(),
                    ))
                    .debug_selector(move || debug_id.clone())
                    .h(px(26.0))
                    .relative()
                    .child(
                        gpui::canvas(
                            move |bounds, _, _| {
                                measure_bounds.set(Some(bounds));
                                tab_bounds.borrow_mut().insert(measured_tab.clone(), bounds);
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    )
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
                                .bg(theme::colors(cx).button_background_hover)
                                .text_color(theme::colors(cx).text_primary)
                        } else {
                            style
                        }
                    })
                    .cursor(CursorStyle::PointingHand)
                    .on_drag(drag, move |drag: &DraggedPanelTab, offset, window, cx| {
                        let size = measured
                            .get()
                            .map(|bounds| bounds.size)
                            .unwrap_or(gpui::size(px(160.), px(26.)));
                        let preview = crate::window_host::drag_preview::open(
                            &drag_label,
                            file_icon,
                            pinned,
                            size,
                            window.mouse_position(),
                            window,
                            cx,
                        );
                        let _ = drag_shell.update(cx, |shell, _| {
                            shell.drop_target = None;
                            shell.tab_drag.begin(drag.clone());
                            shell.drag_preview = preview;
                        });
                        cx.new(|_| {
                            crate::window_host::drag_preview::DragPreview::new(
                                drag_label.clone(),
                                file_icon,
                                pinned,
                                size,
                                point(offset.x - size.width / 2., offset.y - size.height / 2.),
                                !cfg!(target_os = "windows"),
                            )
                        })
                    })
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            if this.tab_mutations_blocked {
                                return;
                            }
                            this.tab_context =
                                Some((context_panel.clone(), context_tab.clone(), event.position));
                            crate::gpui_compat::focus(window, &this.focus_handle, cx);
                            cx.notify();
                        }),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| match tab.clone() {
                        PanelTab::Tool { view, .. } => {
                            this.select_tab_in_panel(
                                select_panel.clone(),
                                position,
                                tab.clone(),
                                cx,
                            );
                            // 外部 Resource を参照するタブは、その Resource を操作対象にする。
                            if let Some(resource) = tab.resource() {
                                cx.emit(UiCommand::ActivateViewResource {
                                    view,
                                    resource: resource.to_owned(),
                                });
                            }
                        }
                        PanelTab::Document(document_id) => {
                            this.select_tab_in_panel(
                                select_panel.clone(),
                                position,
                                PanelTab::Document(document_id.clone()),
                                cx,
                            );
                            cx.emit(UiCommand::ActivateDocument(document_id));
                            let _ = window;
                        }
                    }))
                    .when_some(file_icon, |tab_el, icon| {
                        tab_el.child(crate::components::FileIcon::new(icon).size(px(12.0)))
                    })
                    .child(label)
                    .when(!pinned, |element| {
                        element.child(
                            div()
                                .id(ElementId::Name(
                                    format!("close-tab-{}-{index}", panel_id.as_str()).into(),
                                ))
                                .size(px(11.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_color(theme::colors(cx).text_secondary)
                                .cursor(CursorStyle::PointingHand)
                                .hover(|style| style.text_color(theme::colors(cx).text_primary))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|_, _, _, cx| {
                                        cx.stop_propagation();
                                    }),
                                )
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.close_tab_in_panel(
                                        close_panel_id.clone(),
                                        position,
                                        close_tab.clone(),
                                        window,
                                        cx,
                                    );
                                }))
                                .child(Icon::new(IconName::X).size(px(9.0))),
                        )
                    })
                    .into_any_element()
            })
            .collect();

        if !has_overflow && !self.detached {
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
                    ElementId::Name(format!("panel-tabs-{}", panel_id.as_str()).into()),
                    ScrollAxis::Horizontal,
                    &scroll_state,
                    div()
                        .flex()
                        .items_center()
                        .gap(tokens::spacing::XS)
                        .children(tab_items),
                    cx,
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
                        gpui::linear_color_stop(theme::colors(cx).background_secondary, 1.0),
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
                        gpui::linear_color_stop(theme::colors(cx).background_secondary, 1.0),
                    )),
            );
        }

        crate::components::panel_header()
            .child(tab_container)
            .when(has_overflow && !self.detached, |bar| {
                bar.child(add_button("fixed"))
            })
    }
}
