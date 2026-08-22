use super::*;

struct PanelTabDragPreview {
    label: String,
}

impl Render for PanelTabDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded(theme::radius(theme::Radius::MenuItem))
            .bg(theme::surface())
            .border_1()
            .border_color(theme::border())
            .text_color(theme::text())
            .text_size(px(11.0))
            .child(self.label.clone())
    }
}

impl Editor {
    pub(super) fn render_tool_panel_header(
        &self,
        panel: &PanelHost,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let tabs = panel.tabs.iter().enumerate().map(|(index, tab)| {
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
            let position = panel.position;
            let drag = DraggedPanelTab {
                source_panel: position,
                tab: tab.clone(),
            };
            crate::components::surface(crate::components::SurfaceVariant::Tab)
                .id(("panel-tab", panel_key(position) * 100 + index as u32))
                .h(px(30.0))
                .px_2()
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
                .cursor(CursorStyle::OpenHand)
                .on_drag(drag, |drag: &DraggedPanelTab, _, _, cx| {
                    cx.new(|_| PanelTabDragPreview {
                        label: format!("{:?}", drag.tab),
                    })
                })
                .on_click(cx.listener(move |this, _, window, cx| match tab.clone() {
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
                }))
                .when_some(file_icon, |tab, icon| {
                    tab.child(crate::components::FileIcon::new(icon))
                })
                .child(label)
                .child(div().flex_1())
                .child(
                    div()
                        .id(("close-panel-tab", panel_key(position) * 100 + index as u32))
                        .size(px(20.0))
                        .rounded(px(4.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor(CursorStyle::PointingHand)
                        .hover(|style| style.bg(theme::surface_hover()).text_color(theme::text()))
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
                        .child(Icon::new(IconName::X)),
                )
        });
        let position = panel.position;
        let has_tabs = !panel.tabs.is_empty();
        div()
            .h(px(39.0))
            .flex_shrink_0()
            .px_2()
            .flex()
            .items_center()
            .gap_1()
            .child(
                div()
                    .id(("panel-tabs", panel_key(position)))
                    .flex_1()
                    .min_w(px(0.0))
                    .scrollable(ScrollAxis::Horizontal)
                    .flex()
                    .items_center()
                    .gap_1()
                    .children(tabs),
            )
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
            .when(has_tabs, |bar| {
                bar.child(
                    div()
                        .id(("open-tool-tab", panel_key(position)))
                        .flex_shrink_0()
                        .size(px(25.0))
                        .rounded(px(5.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(theme::muted())
                        .hover(|style| style.bg(theme::surface_hover()).text_color(theme::text()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_tool_picker(position, cx);
                        }))
                        .child("+"),
                )
            })
    }
}
