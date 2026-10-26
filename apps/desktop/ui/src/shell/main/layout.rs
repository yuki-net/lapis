use super::*;

impl Shell {
    /// Renders the animated root panel layout, or a single main panel filling a detached window's body.
    pub(crate) fn render_main(&self, window: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        if self.detached {
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .p(tokens::spacing::GAP)
                .child(self.render_panel_window(self.shell.panel(PanelPosition::Main), cx));
        }
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

        if self.shell.layout.has_variable_splits() {
            return self.render_variable_layout(now, cx);
        }

        let gap = f32::from(tokens::spacing::GAP);
        let left_size = self.shell.panel(PanelPosition::Left).effective_size(now);
        let right_size = self.shell.panel(PanelPosition::Right).effective_size(now);
        let bottom_size = self.shell.panel(PanelPosition::Bottom).effective_size(now);
        let body_height = (f32::from(window.viewport_size().height)
            - f32::from(tokens::size::TITLE_BAR_HEIGHT)
            - f32::from(tokens::size::STATUS_BAR_HEIGHT)
            - gap * 2.0)
            .max(0.0);

        let left_visible = self.shell.panel(PanelPosition::Left).is_visible(now);
        let right_visible = self.shell.panel(PanelPosition::Right).is_visible(now);
        let bottom_visible = self.shell.panel(PanelPosition::Bottom).is_visible(now);
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
                        self.shell.panel(PanelPosition::Left),
                        Some(left_size),
                        cx,
                    ))
                    .when(
                        !self.shell.panel(PanelPosition::Left).is_transitioning(),
                        |top| top.child(self.render_resize_handle(ResizeTarget::Left, false, cx)),
                    )
                    .child(self.render_main_flex(cx))
                    .when(
                        !self.shell.panel(PanelPosition::Right).is_transitioning(),
                        |top| top.child(self.render_resize_handle(ResizeTarget::Right, false, cx)),
                    )
                    .child(self.render_panel_window_frame(
                        self.shell.panel(PanelPosition::Right),
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
                        self.shell.panel(PanelPosition::Left),
                        Some(left_size),
                        cx,
                    ))
                    .when(
                        !self.shell.panel(PanelPosition::Left).is_transitioning(),
                        |top| top.child(self.render_resize_handle(ResizeTarget::Left, false, cx)),
                    )
                    .child(self.render_main_flex(cx));

                root()
                    .flex()
                    .child(self.render_bottom_stack(top, bottom_size, left_inset, 0.0, cx))
                    .when(right_visible, |body| {
                        body.when(
                            !self.shell.panel(PanelPosition::Right).is_transitioning(),
                            |body| {
                                body.child(self.render_resize_handle(
                                    ResizeTarget::Right,
                                    false,
                                    cx,
                                ))
                            },
                        )
                        .child(self.render_side_panel(
                            self.shell.panel(PanelPosition::Right),
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
                    .when(
                        !self.shell.panel(PanelPosition::Right).is_transitioning(),
                        |top| top.child(self.render_resize_handle(ResizeTarget::Right, false, cx)),
                    )
                    .child(self.render_panel_window_frame(
                        self.shell.panel(PanelPosition::Right),
                        Some(right_size),
                        cx,
                    ));

                root()
                    .flex()
                    .when(left_visible, |body| {
                        body.child(self.render_side_panel(
                            self.shell.panel(PanelPosition::Left),
                            left_size,
                            left_shortening,
                            left_height,
                            cx,
                        ))
                        .when(
                            !self.shell.panel(PanelPosition::Left).is_transitioning(),
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
                            self.shell.panel(PanelPosition::Left),
                            left_size,
                            left_shortening,
                            left_height,
                            cx,
                        ))
                        .when(
                            !self.shell.panel(PanelPosition::Left).is_transitioning(),
                            |body| {
                                body.child(self.render_resize_handle(ResizeTarget::Left, false, cx))
                            },
                        )
                    })
                    .child(center)
                    .when(right_visible, |body| {
                        body.when(
                            !self.shell.panel(PanelPosition::Right).is_transitioning(),
                            |body| {
                                body.child(self.render_resize_handle(
                                    ResizeTarget::Right,
                                    false,
                                    cx,
                                ))
                            },
                        )
                        .child(self.render_side_panel(
                            self.shell.panel(PanelPosition::Right),
                            right_size,
                            right_shortening,
                            right_height,
                            cx,
                        ))
                    })
            }
        }
    }

    fn render_variable_layout(&self, now: std::time::Instant, cx: &mut Context<Self>) -> gpui::Div {
        self.render_layout_node(self.shell.layout.root(), now, cx)
            .h_full()
            .w_full()
            .flex()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .overflow_hidden()
    }

    fn render_layout_node(
        &self,
        node: &LayoutNode,
        now: std::time::Instant,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        if !self.layout_node_visible(node, now) {
            return div().size(px(0.0)).flex_shrink_0();
        }
        match node {
            LayoutNode::Tabs(group) => {
                let panel = self
                    .shell
                    .layout
                    .group(group)
                    .expect("layout leaf must have a tab group");
                self.render_panel_window_for_group(panel, group, cx)
            }
            LayoutNode::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                let first_visible = self.layout_node_visible(first, now);
                let second_visible = self.layout_node_visible(second, now);
                let allocation = crate::shell::layout::SplitAllocation::new(
                    *direction,
                    *ratio,
                    [first_visible, second_visible],
                );
                // basis=0からGAP控除後の空間を配分する。百分率幅とGAPを加算しない。
                let mut first = self
                    .render_layout_node(first, now, cx)
                    .flex_basis(px(0.0))
                    .min_h(px(0.0))
                    .min_w(px(0.0))
                    .overflow_hidden();
                first.style().flex_grow = Some(allocation.weights[0]);
                let mut second = self
                    .render_layout_node(second, now, cx)
                    .flex_basis(px(0.0))
                    .min_h(px(0.0))
                    .min_w(px(0.0))
                    .overflow_hidden();
                second.style().flex_grow = Some(allocation.weights[1]);
                let mut split = div()
                    .flex()
                    .flex_1()
                    .min_h(px(0.0))
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .child(first)
                    // GAP自体も極小WindowではGPUIに縮小させる。
                    .child(
                        div()
                            .flex_basis(px(allocation.gap))
                            .min_w(px(0.0))
                            .min_h(px(0.0)),
                    )
                    .child(second);
                split = match direction {
                    SplitDirection::Horizontal => split.flex_row(),
                    SplitDirection::Vertical => split.flex_col(),
                };
                split
            }
        }
    }

    fn layout_node_visible(&self, node: &LayoutNode, now: std::time::Instant) -> bool {
        match node {
            LayoutNode::Tabs(group) => self
                .shell
                .layout
                .group(group)
                .is_some_and(|panel| panel.is_visible(now)),
            LayoutNode::Split { first, second, .. } => {
                self.layout_node_visible(first, now) || self.layout_node_visible(second, now)
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
            .child(self.render_panel_window(self.shell.panel(PanelPosition::Main), cx))
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
                self.render_panel_window_frame(
                    self.shell.panel(PanelPosition::Bottom),
                    Some(bottom_size),
                    cx,
                ),
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
            .when(
                !self.shell.panel(PanelPosition::Bottom).is_transitioning(),
                |stack| stack.child(self.render_resize_handle(ResizeTarget::Bottom, true, cx)),
            )
            .child(bottom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Empty;
    impl Render for Empty {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    struct Fixture {
        shell: gpui::Entity<Shell>,
        size: (f32, f32),
    }
    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().w(px(self.size.0)).h(px(self.size.1)).flex().child(
                self.shell.update(cx, |shell, cx| {
                    shell.render_variable_layout(std::time::Instant::now(), cx)
                }),
            )
        }
    }

    #[gpui::test]
    fn gpui_nested_splits_fit_after_resize_without_changing_layout(cx: &mut gpui::TestAppContext) {
        let window = cx.add_window(|_, cx| {
            let document = cx.new(|_| Empty);
            let shell = cx.new(|cx| Shell::new(document.into(), HashMap::new(), cx));
            shell.update(cx, |shell, _| {
                let main = shell
                    .shell
                    .layout
                    .primary_group_id(PanelPosition::Main)
                    .clone();
                let extra = shell
                    .shell
                    .layout
                    .split_group(&main, SplitDirection::Horizontal, 0.3)
                    .unwrap();
                shell
                    .shell
                    .layout
                    .split_group(&extra, SplitDirection::Vertical, 0.65)
                    .unwrap();
                let left = shell
                    .shell
                    .layout
                    .primary_group_id(PanelPosition::Left)
                    .clone();
                shell
                    .shell
                    .layout
                    .split_group(&left, SplitDirection::Vertical, 0.4)
                    .unwrap();
            });
            Fixture {
                shell,
                size: (1000.0, 700.0),
            }
        });
        let shell = window
            .update(cx, |fixture, _, _| fixture.shell.clone())
            .unwrap();
        for hidden in [false, true] {
            shell.update(cx, |shell, _| {
                let left = shell
                    .shell
                    .layout
                    .primary_group_id(PanelPosition::Left)
                    .clone();
                shell
                    .shell
                    .layout
                    .group_mut(&left)
                    .unwrap()
                    .set_open_immediate(!hidden);
            });
            let before = shell.read_with(cx, |shell, _| shell.shell.layout.capture_view_state());
            let mut original = None;
            for size in [(1000.0, 700.0), (240.0, 180.0), (4.0, 4.0), (1000.0, 700.0)] {
                window
                    .update(cx, |fixture, window, cx| {
                        fixture.size = size;
                        fixture
                            .shell
                            .update(cx, |shell, _| shell.group_bounds.borrow_mut().clear());
                        window.refresh();
                    })
                    .unwrap();
                cx.update_window(window.into(), |_, window, cx| {
                    window.draw(cx).clear();
                })
                .unwrap();
                let bounds = shell.read_with(cx, |shell, _| {
                    assert_eq!(shell.shell.layout.capture_view_state(), before);
                    shell.group_bounds.borrow().clone()
                });
                assert!(!bounds.is_empty());
                for bounds in bounds.values() {
                    assert!(f32::from(bounds.origin.x) >= -0.1);
                    assert!(f32::from(bounds.origin.y) >= -0.1);
                    assert!(
                        f32::from(bounds.origin.x + bounds.size.width) <= size.0 + 0.1,
                        "{bounds:?} in {size:?}"
                    );
                    assert!(
                        f32::from(bounds.origin.y + bounds.size.height) <= size.1 + 0.1,
                        "{bounds:?} in {size:?}"
                    );
                }
                if size == (1000.0, 700.0) {
                    if let Some(original) = &original {
                        assert_eq!(&bounds, original);
                    } else {
                        original = Some(bounds);
                    }
                }
            }
        }
    }
}
