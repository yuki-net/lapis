use super::*;

impl Shell {
    pub(super) fn render_resize_handle(
        &self,
        target: ResizeTarget,
        horizontal: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let mut handle = div()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .hover(|style| style.bg(theme::colors(cx).button_background_selected))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.start_resize(target, event.position, window.viewport_size(), cx);
                }),
            );
        if horizontal {
            handle = handle
                .h(tokens::spacing::GAP)
                .w_full()
                .cursor(CursorStyle::ResizeUpDown);
        } else {
            handle = handle
                .w(tokens::spacing::GAP)
                .h_full()
                .cursor(CursorStyle::ResizeLeftRight);
        }
        handle
    }

    pub(super) fn render_panel_window(
        &self,
        position: ZonePosition,
        panel: &PanelHost,
        panel_id: &PanelId,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let panel_id = panel_id.clone();
        let shell = cx.entity().downgrade();
        let active = self.shell.visible_active(panel);
        let (content, scroll_policy) = match active {
            Some(PanelTab::Tool { view, instance }) => {
                let policy = self
                    .feature_registry
                    .zone_contributions(position)
                    .into_iter()
                    .find(|contribution| contribution.view.as_ref() == Some(view))
                    .map(|contribution| contribution.scroll_policy)
                    .unwrap_or(PanelScrollPolicy::Disabled);
                (
                    self.render_tool_content(view, instance, cx)
                        .into_any_element(),
                    policy,
                )
            }
            Some(PanelTab::Document(document_id)) => (
                self.render_document_content(document_id, cx)
                    .into_any_element(),
                PanelScrollPolicy::FeatureOwned,
            ),
            None => (
                self.render_empty_panel(panel_id.clone(), cx)
                    .into_any_element(),
                PanelScrollPolicy::Panel(ScrollAxis::Both),
            ),
        };

        let body = match scroll_policy {
            PanelScrollPolicy::Panel(axis) => {
                let state = active.map_or_else(
                    || self.scroll_states.panel_empty(&panel_id),
                    |tab| self.scroll_states.panel_content(tab),
                );
                scroll_viewport(
                    ElementId::Name(format!("panel-body-scroll-{}", panel_id.as_str()).into()),
                    axis,
                    &state,
                    content,
                    cx,
                )
                .flex_1()
                .size_full()
            }
            PanelScrollPolicy::FeatureOwned | PanelScrollPolicy::Disabled => div()
                .flex_1()
                .size_full()
                .flex()
                .flex_col()
                .min_h(px(0.0))
                .min_w(px(0.0))
                .overflow_hidden()
                .child(content),
        };

        let preview = self
            .drop_target
            .as_ref()
            .filter(|preview| match &preview.target {
                PanelDrop::Into(id) => id == &panel_id,
                PanelDrop::Edge { panel, .. } => panel == &panel_id && preview.invalid,
                PanelDrop::IntoZone(_) => false,
            })
            .cloned();
        let bounds_panel = panel_id.clone();
        div()
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .overflow_hidden()
            .child(
                crate::components::panel_surface(cx)
                    .when(cfg!(test), |surface| {
                        surface.child(
                            gpui::canvas(
                                move |bounds, _, cx| {
                                    let _ = shell.update(cx, |shell, _| {
                                        shell
                                            .zone_panel_bounds
                                            .borrow_mut()
                                            .insert(position, bounds);
                                        shell
                                            .panel_bounds
                                            .borrow_mut()
                                            .insert(bounds_panel.clone(), bounds);
                                    });
                                },
                                |_, _, _, _| {},
                            )
                            .absolute()
                            .size_full(),
                        )
                    })
                    .child(self.render_tool_panel_header(position, panel, &panel_id, cx))
                    .child(body),
            )
            .when_some(preview, |surface, preview| {
                surface.child(self.render_drop_overlay(&preview, cx))
            })
    }

    fn render_drop_overlay(&self, preview: &DropPreview, cx: &gpui::App) -> gpui::Div {
        let mut marker = div()
            .absolute()
            .bg(theme::colors(cx).button_background_selected)
            .opacity(0.65);
        if preview.invalid {
            marker = marker.opacity(0.25);
        }
        match preview.target {
            PanelDrop::Into(_) => marker.inset_0(),
            PanelDrop::IntoZone(_) => marker.inset_0(),
            PanelDrop::Edge {
                edge: PanelEdge::Left,
                ..
            } => marker.left_0().top_0().bottom_0().w(px(4.0)),
            PanelDrop::Edge {
                edge: PanelEdge::Right,
                ..
            } => marker.right_0().top_0().bottom_0().w(px(4.0)),
            PanelDrop::Edge {
                edge: PanelEdge::Top,
                ..
            } => marker.left_0().right_0().top_0().h(px(4.0)),
        }
    }
}
