use super::*;

impl Shell {
    pub(super) fn render_panel_window(
        &self,
        panel: &PanelHost,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let group = self.shell.layout.primary_group_id(panel.position).clone();
        self.render_panel_window_frame_for_group(panel, &group, None, false, cx)
    }

    pub(super) fn render_resize_handle(
        &self,
        target: ResizeTarget,
        horizontal: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let mut handle = div()
            .flex_shrink_0()
            .hover(|style| style.bg(theme::colors().button_background_selected))
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

    /// Renders a panel's tabs and active content and records its bounds for drop targeting.
    /// `animated_size` overrides the side width or bottom height in logical pixels.
    /// Drops from this group move tabs locally or emit a command for a transfer between windows.
    pub(super) fn render_panel_window_frame(
        &self,
        panel: &PanelHost,
        animated_size: Option<f32>,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let group = self.shell.layout.primary_group_id(panel.position).clone();
        self.render_panel_window_frame_for_group(panel, &group, animated_size, false, cx)
    }

    pub(super) fn render_panel_window_for_group(
        &self,
        panel: &PanelHost,
        group: &TabGroupId,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        self.render_panel_window_frame_for_group(panel, group, None, true, cx)
    }

    fn render_panel_window_frame_for_group(
        &self,
        panel: &PanelHost,
        group: &TabGroupId,
        animated_size: Option<f32>,
        variable_layout: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let position = panel.position;
        let group = group.clone();
        let shell = cx.entity().downgrade();
        let is_bottom = panel.position == PanelPosition::Bottom;
        let panel_size = animated_size.unwrap_or(panel.size);
        let size = if variable_layout {
            div().flex_1().min_h(px(0.0)).min_w(px(0.0))
        } else if panel.position == PanelPosition::Main {
            div().w_full().flex_1().min_h(px(0.0))
        } else if is_bottom {
            div().h(px(panel_size)).w_full()
        } else {
            div().w(px(panel_size)).h_full()
        };

        let active = self.shell.visible_active(panel);
        let (content, scroll_policy) = match active {
            Some(PanelTab::Tool { view, instance }) => {
                let policy = self
                    .feature_registry
                    .panel_contributions(position)
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
                self.render_empty_group(group.clone(), cx)
                    .into_any_element(),
                PanelScrollPolicy::Panel(ScrollAxis::Both),
            ),
        };

        let body = match scroll_policy {
            PanelScrollPolicy::Panel(axis) => {
                let state = active.map_or_else(
                    || self.scroll_states.panel_empty(&group),
                    |tab| self.scroll_states.panel_content(tab),
                );
                scroll_viewport(
                    ElementId::Name(format!("panel-body-scroll-{}", group.as_str()).into()),
                    axis,
                    &state,
                    content,
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

        let preview_zone = self
            .drop_target
            .as_ref()
            .filter(|target| target.group == group)
            .cloned();
        let group_for_drop = group.clone();
        let bounds_group = group.clone();
        size.when(!variable_layout, |size| size.flex_shrink_0())
            .overflow_hidden()
            .child(
                crate::components::panel_surface()
                    .on_drop(cx.listener(move |this, drag: &DraggedPanelTab, _, cx| {
                        if !drag.belongs_to(this.group) {
                            return;
                        }
                        let zone = this
                            .drop_target
                            .as_ref()
                            .filter(|target| target.group == group_for_drop)
                            .map_or(DropZone::Center, |target| target.zone);
                        this.drop_target = None;
                        if drag.source_host == this.host {
                            this.move_panel_tab_to_group(
                                drag.source_group.clone(),
                                group_for_drop.clone(),
                                drag.tab.clone(),
                                zone,
                                cx,
                            );
                        } else {
                            cx.emit(UiCommand::MoveTab(drag.clone(), group_for_drop.clone()));
                        }
                    }))
                    .child(
                        gpui::canvas(
                            move |bounds, _, cx| {
                                let _ = shell.update(cx, |shell, _| {
                                    shell.panel_bounds.borrow_mut().insert(position, bounds);
                                    shell
                                        .group_bounds
                                        .borrow_mut()
                                        .insert(bounds_group.clone(), bounds);
                                });
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .child(self.render_tool_panel_header_for_group(panel, &group, cx))
                    .child(body),
            )
            .when_some(preview_zone, |surface, target| {
                surface.child(self.render_drop_overlay(target.zone, target.error))
            })
    }

    fn render_drop_overlay(&self, zone: DropZone, error: Option<LayoutError>) -> gpui::Div {
        let overlay = div()
            .size_full()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .overflow_hidden()
            .rounded(tokens::radius::CONTROL)
            .bg(theme::colors().button_background_selected)
            .opacity(0.55)
            .border_1()
            .border_color(theme::colors().button_border_focused);
        let overlay = overlay.when_some(error, |overlay, error| {
            overlay
                .text_color(theme::colors().text_primary)
                .text_size(tokens::typography::FONT_SM)
                .child(match error {
                    LayoutError::InsufficientSpace => "分割できる最小領域がありません",
                    LayoutError::SideColumnLimitReached => "左右の列は通常2列までです",
                    _ => "この位置には配置できません",
                })
        });
        if zone == DropZone::Center {
            return div().absolute().inset_0().child(overlay);
        }
        let horizontal = matches!(zone, DropZone::Left | DropZone::Right);
        let allocation = crate::shell::layout::SplitAllocation::new(
            if horizontal {
                SplitDirection::Horizontal
            } else {
                SplitDirection::Vertical
            },
            0.5,
            [true, true],
        );
        let selected = div().flex_1().min_w(px(0.0)).min_h(px(0.0)).child(overlay);
        let empty = div().flex_1().min_w(px(0.0)).min_h(px(0.0));
        let (first, second) = if matches!(zone, DropZone::Left | DropZone::Top) {
            (selected, empty)
        } else {
            (empty, selected)
        };
        div()
            .absolute()
            .inset_0()
            .flex()
            .when(!horizontal, |container| container.flex_col())
            .child(first)
            .child(
                div()
                    .flex_basis(px(allocation.gap))
                    .min_w(px(0.0))
                    .min_h(px(0.0)),
            )
            .child(second)
    }
}
