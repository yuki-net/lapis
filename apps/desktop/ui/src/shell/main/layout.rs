use super::*;

impl Editor {
    pub(crate) fn render_main(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let now = std::time::Instant::now();
        if self
            .shell
            .panels()
            .into_iter()
            .any(|panel| panel.is_animating(now))
            || self.shell.bottom_span_is_animating(now)
        {
            window.request_animation_frame();
        }

        let gap = f32::from(tokens::spacing::GAP);
        let left_size = self.shell.left_panel.effective_size(now);
        let right_size = self.shell.right_panel.effective_size(now);
        let bottom_size = self.shell.bottom_panel.effective_size(now);
        let body_height = (f32::from(window.viewport_size().height)
            - f32::from(tokens::size::TITLE_BAR_HEIGHT)
            - f32::from(tokens::size::STATUS_BAR_HEIGHT)
            - gap * 2.0)
            .max(0.0);

        let left_visible = self.shell.left_panel.is_visible(now);
        let right_visible = self.shell.right_panel.is_visible(now);
        let bottom_visible = self.shell.bottom_panel.is_visible(now);
        let span_left = bottom_visible && left_visible && self.shell.bottom_spans_left_layout(now);
        let span_right =
            bottom_visible && right_visible && self.shell.bottom_spans_right_layout(now);

        let left_shortening = if bottom_visible && left_visible {
            self.shell.left_side_shortening(now)
        } else {
            0.0
        };
        let right_shortening = if bottom_visible && right_visible {
            self.shell.right_side_shortening(now)
        } else {
            0.0
        };
        let left_height = (body_height - (bottom_size + gap) * left_shortening).max(0.0);
        let right_height = (body_height - (bottom_size + gap) * right_shortening).max(0.0);
        let left_inset = if span_left {
            (left_size + gap) * (1.0 - self.shell.bottom_left_extent(now))
        } else {
            0.0
        };
        let right_inset = if span_right {
            (right_size + gap) * (1.0 - self.shell.bottom_right_extent(now))
        } else {
            0.0
        };

        let root = || {
            div()
                .h(px(0.0))
                .w_full()
                .min_h(px(0.0))
                .min_w(px(0.0))
                .flex_1()
                .px(tokens::spacing::GAP)
        };

        match (span_left, span_right) {
            (true, true) => {
                let top = div()
                    .w_full()
                    .flex()
                    .flex_1()
                    .min_h(px(0.0))
                    .min_w(px(0.0))
                    .child(self.render_panel_window_frame(
                        &self.shell.left_panel,
                        Some(left_size),
                        cx,
                    ))
                    .when(!self.shell.left_panel.is_transitioning(), |top| {
                        top.child(self.render_resize_handle(ResizeTarget::Left, false, cx))
                    })
                    .child(self.render_main_flex(cx))
                    .when(!self.shell.right_panel.is_transitioning(), |top| {
                        top.child(self.render_resize_handle(ResizeTarget::Right, false, cx))
                    })
                    .child(self.render_panel_window_frame(
                        &self.shell.right_panel,
                        Some(right_size),
                        cx,
                    ));

                root().flex().flex_col().child(self.render_bottom_stack(
                    top,
                    bottom_size,
                    left_inset,
                    right_inset,
                    cx,
                ))
            }
            (true, false) => {
                let top = div()
                    .w_full()
                    .flex()
                    .flex_1()
                    .min_h(px(0.0))
                    .min_w(px(0.0))
                    .child(self.render_panel_window_frame(
                        &self.shell.left_panel,
                        Some(left_size),
                        cx,
                    ))
                    .when(!self.shell.left_panel.is_transitioning(), |top| {
                        top.child(self.render_resize_handle(ResizeTarget::Left, false, cx))
                    })
                    .child(self.render_main_flex(cx));

                root()
                    .flex()
                    .child(self.render_bottom_stack(top, bottom_size, left_inset, 0.0, cx))
                    .when(right_visible, |body| {
                        body.when(!self.shell.right_panel.is_transitioning(), |body| {
                            body.child(self.render_resize_handle(ResizeTarget::Right, false, cx))
                        })
                        .child(self.render_side_panel(
                            &self.shell.right_panel,
                            right_size,
                            right_shortening,
                            right_height,
                            cx,
                        ))
                    })
            }
            (false, true) => {
                let top = div()
                    .w_full()
                    .flex()
                    .flex_1()
                    .min_h(px(0.0))
                    .min_w(px(0.0))
                    .child(self.render_main_flex(cx))
                    .when(!self.shell.right_panel.is_transitioning(), |top| {
                        top.child(self.render_resize_handle(ResizeTarget::Right, false, cx))
                    })
                    .child(self.render_panel_window_frame(
                        &self.shell.right_panel,
                        Some(right_size),
                        cx,
                    ));

                root()
                    .flex()
                    .when(left_visible, |body| {
                        body.child(self.render_side_panel(
                            &self.shell.left_panel,
                            left_size,
                            left_shortening,
                            left_height,
                            cx,
                        ))
                        .when(
                            !self.shell.left_panel.is_transitioning(),
                            |body| {
                                body.child(self.render_resize_handle(ResizeTarget::Left, false, cx))
                            },
                        )
                    })
                    .child(self.render_bottom_stack(top, bottom_size, 0.0, right_inset, cx))
            }
            (false, false) => {
                let main = self.render_main_flex(cx);
                let center = if bottom_visible {
                    self.render_bottom_stack(main, bottom_size, 0.0, 0.0, cx)
                } else {
                    div()
                        .h_full()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h(px(0.0))
                        .min_w(px(0.0))
                        .child(main)
                };

                root()
                    .flex()
                    .when(left_visible, |body| {
                        body.child(self.render_side_panel(
                            &self.shell.left_panel,
                            left_size,
                            left_shortening,
                            left_height,
                            cx,
                        ))
                        .when(
                            !self.shell.left_panel.is_transitioning(),
                            |body| {
                                body.child(self.render_resize_handle(ResizeTarget::Left, false, cx))
                            },
                        )
                    })
                    .child(center)
                    .when(right_visible, |body| {
                        body.when(!self.shell.right_panel.is_transitioning(), |body| {
                            body.child(self.render_resize_handle(ResizeTarget::Right, false, cx))
                        })
                        .child(self.render_side_panel(
                            &self.shell.right_panel,
                            right_size,
                            right_shortening,
                            right_height,
                            cx,
                        ))
                    })
            }
        }
    }

    fn render_main_flex(&self, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .h_full()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .min_w(tokens::size::PANEL_MIN_WIDTH)
            .overflow_hidden()
            .child(self.render_panel_window(&self.shell.main_panel, cx))
    }

    fn render_side_panel(
        &self,
        panel: &PanelHost,
        size: f32,
        shortening: f32,
        animated_height: f32,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let panel = self.render_panel_window_frame(panel, Some(size), cx);
        if shortening > 0.0 {
            panel.h(px(animated_height)).flex_shrink_0()
        } else {
            panel
        }
    }

    fn render_bottom_stack(
        &self,
        top: gpui::Div,
        bottom_size: f32,
        left_inset: f32,
        right_inset: f32,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let bottom = div()
            .h(px(bottom_size))
            .w_full()
            .flex()
            .flex_shrink_0()
            .when(left_inset > 0.0, |row| {
                row.child(div().w(px(left_inset)).h_full().flex_shrink_0())
            })
            .child(div().h_full().flex().flex_1().min_w(px(0.0)).child(
                self.render_panel_window_frame(&self.shell.bottom_panel, Some(bottom_size), cx),
            ))
            .when(right_inset > 0.0, |row| {
                row.child(div().w(px(right_inset)).h_full().flex_shrink_0())
            });

        div()
            .h_full()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .child(top)
            .when(!self.shell.bottom_panel.is_transitioning(), |stack| {
                stack.child(self.render_resize_handle(ResizeTarget::Bottom, true, cx))
            })
            .child(bottom)
    }
}
