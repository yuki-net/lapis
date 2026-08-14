use super::*;

impl Editor {
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

    pub(super) fn render_panel_window_frame(
        &self,
        panel: &PanelHost,
        animated_size: Option<f32>,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let position = panel.position;
        let is_bottom = panel.position == PanelPosition::Bottom;
        let panel_size = animated_size.unwrap_or(panel.size);
        let size = if panel.position == PanelPosition::Main {
            div().w_full().flex_1().min_h(px(0.0))
        } else if is_bottom {
            div().h(px(panel_size)).w_full()
        } else {
            div().w(px(panel_size)).h_full()
        };
        size.flex_shrink_0()
            .overflow_hidden()
            .rounded(px(theme::ISLAND_RADIUS))
            .bg(theme::island())
            .flex()
            .flex_col()
            .on_drop(cx.listener(move |this, drag: &DraggedPanelTab, _, cx| {
                this.move_panel_tab(drag.source_panel, position, drag.tab.clone(), cx);
            }))
            .child(self.render_tool_panel_header(panel, cx))
            .child(match panel.active.as_ref() {
                Some(PanelTab::Tool(view)) => self.render_tool_content(view, cx).into_any_element(),
                Some(PanelTab::Document(document_id)) => self
                    .render_document_content(document_id, cx)
                    .into_any_element(),
                None => self
                    .render_empty_panel(panel.position, cx)
                    .into_any_element(),
            })
    }
}
