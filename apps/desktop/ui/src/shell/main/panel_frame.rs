use super::*;

impl Shell {
    pub(super) fn render_panel_window(
        &self,
        panel: &PanelHost,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        self.render_panel_window_frame(panel, None, cx)
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
        let position = panel.position;
        let shell = cx.entity().downgrade();
        let is_bottom = panel.position == PanelPosition::Bottom;
        let panel_size = animated_size.unwrap_or(panel.size);
        let size = if panel.position == PanelPosition::Main {
            div().w_full().flex_1().min_h(px(0.0))
        } else if is_bottom {
            div().h(px(panel_size)).w_full()
        } else {
            div().w(px(panel_size)).h_full()
        };

        let (content, scroll_policy) = match panel.active.as_ref() {
            Some(PanelTab::Tool { view, .. }) => {
                let policy = self
                    .feature_registry
                    .panel_contributions(position)
                    .into_iter()
                    .find(|contribution| contribution.view.as_ref() == Some(view))
                    .map(|contribution| contribution.scroll_policy)
                    .unwrap_or(PanelScrollPolicy::Disabled);
                (
                    self.render_tool_content(view, cx).into_any_element(),
                    policy,
                )
            }
            Some(PanelTab::Document(document_id)) => (
                self.render_document_content(document_id, cx)
                    .into_any_element(),
                PanelScrollPolicy::FeatureOwned,
            ),
            None => (
                self.render_empty_panel(panel.position, cx)
                    .into_any_element(),
                PanelScrollPolicy::Panel(ScrollAxis::Both),
            ),
        };

        let body = match scroll_policy {
            PanelScrollPolicy::Panel(axis) => {
                let state = panel.active.as_ref().map_or_else(
                    || self.scroll_states.panel_empty(position).clone(),
                    |tab| self.scroll_states.panel_content(tab),
                );
                scroll_viewport(
                    ("panel-body-scroll", panel_key(position)),
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

        size.flex_shrink_0().overflow_hidden().child(
            crate::components::panel_surface()
                .on_drop(cx.listener(move |this, drag: &DraggedPanelTab, _, cx| {
                    if !drag.belongs_to(this.group) {
                        return;
                    }
                    if drag.source_host == this.host {
                        this.move_panel_tab(drag.source_panel, position, drag.tab.clone(), cx);
                    } else {
                        cx.emit(UiCommand::MoveTab(drag.clone(), position));
                    }
                }))
                .child(
                    gpui::canvas(
                        move |bounds, _, cx| {
                            let _ = shell.update(cx, |shell, _| {
                                shell.panel_bounds.borrow_mut().insert(position, bounds);
                            });
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size_full(),
                )
                .child(self.render_tool_panel_header(panel, cx))
                .child(body),
        )
    }
}
