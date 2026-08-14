use super::*;

impl Editor {
    pub(crate) fn render_main(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
        _compact_layout: bool,
    ) -> impl IntoElement {
        let now = std::time::Instant::now();
        if self
            .shell
            .panels()
            .into_iter()
            .any(|panel| panel.is_animating(now))
        {
            window.request_animation_frame();
        }
        let viewport_width = f32::from(window.viewport_size().width);
        let left_size = self.shell.left_panel.effective_size(now);
        let right_size = self.shell.right_panel.effective_size(now);
        let bottom_size = self.shell.bottom_panel.effective_size(now);
        let left_width = if self.shell.left_panel.is_visible(now) {
            left_size + theme::CANVAS_GAP
        } else {
            0.0
        };
        let right_width = if self.shell.right_panel.is_visible(now) {
            right_size + theme::CANVAS_GAP
        } else {
            0.0
        };
        let center_width =
            (viewport_width - theme::CANVAS_GAP * 2.0 - left_width - right_width).max(320.0);

        div()
            .h(px(0.0))
            .w_full()
            .min_h(px(0.0))
            .flex()
            .flex_1()
            .px(px(theme::CANVAS_GAP))
            .when(self.shell.left_panel.is_visible(now), |body| {
                body.child(self.render_panel_window_frame(
                    &self.shell.left_panel,
                    Some(left_size),
                    cx,
                ))
                .when(!self.shell.left_panel.is_transitioning(), |body| {
                    body.child(self.render_resize_handle(ResizeTarget::Left, false, cx))
                })
            })
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
                    .child(self.render_panel_window(&self.shell.main_panel, cx))
                    .when(self.shell.bottom_panel.is_visible(now), |center| {
                        center
                            .when(!self.shell.bottom_panel.is_transitioning(), |center| {
                                center.child(self.render_resize_handle(
                                    ResizeTarget::Bottom,
                                    true,
                                    cx,
                                ))
                            })
                            .child(self.render_panel_window_frame(
                                &self.shell.bottom_panel,
                                Some(bottom_size),
                                cx,
                            ))
                    }),
            )
            .when(self.shell.right_panel.is_visible(now), |body| {
                body.when(!self.shell.right_panel.is_transitioning(), |body| {
                    body.child(self.render_resize_handle(ResizeTarget::Right, false, cx))
                })
                .child(self.render_panel_window_frame(
                    &self.shell.right_panel,
                    Some(right_size),
                    cx,
                ))
            })
    }
}
