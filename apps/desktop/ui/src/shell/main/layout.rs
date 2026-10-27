use super::*;
use crate::shell::panel_layout::{Axis, PanelLayout};

impl Shell {
    /// 全Zoneを同じPanel配置Rendererで描画する。
    pub(crate) fn render_main(&self, window: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let now = std::time::Instant::now();
        if self.shell.layout.has_closing_panels() || self.shell.bottom_span_is_animating(now) {
            window.request_animation_frame();
        }
        if self.detached {
            return self
                .render_zone(PanelPosition::Main, now, cx)
                .flex_1()
                .min_h(px(0.0));
        }
        let left = self
            .shell
            .layout
            .zone(PanelPosition::Left)
            .visual_factor(now)
            > 0.0;
        let right = self
            .shell
            .layout
            .zone(PanelPosition::Right)
            .visual_factor(now)
            > 0.0;
        let bottom = self
            .shell
            .layout
            .zone(PanelPosition::Bottom)
            .visual_factor(now)
            > 0.0;
        let span_left = bottom && left && self.shell.bottom_spans_left_layout(now);
        let span_right = bottom && right && self.shell.bottom_spans_right_layout(now);
        let mut top = self
            .render_zone(PanelPosition::Main, now, cx)
            .flex_1()
            .min_w(px(0.0));
        if span_left {
            top = self.zone_row(Some(PanelPosition::Left), top, None, now, cx);
        }
        if span_right {
            top = self.zone_row(None, top, Some(PanelPosition::Right), now, cx);
        }
        let mut content = if bottom {
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .child(top.flex_1().min_h(px(0.0)))
                .child(self.render_resize_handle(ResizeTarget::Bottom, true, cx))
                .child(
                    self.render_zone(PanelPosition::Bottom, now, cx)
                        .h(px(self.shell.layout.zone(PanelPosition::Bottom).size
                            * self
                                .shell
                                .layout
                                .zone(PanelPosition::Bottom)
                                .visual_factor(now)))
                        .min_h(px(0.0)),
                )
        } else {
            top
        };
        if left && !span_left {
            content = self.zone_row(Some(PanelPosition::Left), content, None, now, cx);
        }
        if right && !span_right {
            content = self.zone_row(None, content, Some(PanelPosition::Right), now, cx);
        }
        div()
            .flex()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .overflow_hidden()
            .px(tokens::spacing::GAP)
            .child(content)
    }

    fn zone_row(
        &self,
        left: Option<PanelPosition>,
        center: gpui::Div,
        right: Option<PanelPosition>,
        now: std::time::Instant,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let mut row = div()
            .flex()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .overflow_hidden();
        if let Some(position) = left {
            row = row
                .child(
                    self.render_zone(position, now, cx)
                        .w(px(self.shell.layout.zone(position).size
                            * self.shell.layout.zone(position).visual_factor(now)))
                        .min_w(px(0.0)),
                )
                .child(self.render_resize_handle(ResizeTarget::Left, false, cx));
        }
        row = row.child(center.flex_1().min_w(px(0.0)));
        if let Some(position) = right {
            row = row
                .child(self.render_resize_handle(ResizeTarget::Right, false, cx))
                .child(
                    self.render_zone(position, now, cx)
                        .w(px(self.shell.layout.zone(position).size
                            * self.shell.layout.zone(position).visual_factor(now)))
                        .min_w(px(0.0)),
                );
        }
        row
    }

    fn render_zone(
        &self,
        position: PanelPosition,
        now: std::time::Instant,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let zone = self.shell.layout.zone(position);
        let shell = cx.entity().downgrade();
        let record = gpui::canvas(
            move |bounds, _, cx| {
                let _ = shell.update(cx, |shell, _| {
                    shell.zone_bounds.borrow_mut().insert(position, bounds);
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        let mut root = div()
            .relative()
            .flex()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .overflow_hidden()
            .child(record);
        if let Some(layout) = &zone.layout {
            root = root.child(self.render_panel_layout(position, layout, Vec::new(), now, cx));
        } else if position == PanelPosition::Main {
            root = root.child(self.render_main_empty_panel(cx));
        }
        if let Some(preview) = self
            .drop_target
            .as_ref()
            .filter(|preview| preview.zone == position && preview.rect.is_some())
        {
            let (x, y, width, height) = preview.rect.unwrap();
            root = root.child(
                div()
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .w(px(width))
                    .h(px(height))
                    .bg(theme::colors().button_background_selected)
                    .opacity(0.55),
            );
        } else if self.drop_target.as_ref().is_some_and(
            |preview| matches!(preview.target, PanelDrop::IntoZone(target) if target == position),
        ) {
            root = root.child(
                div()
                    .absolute()
                    .inset_0()
                    .bg(theme::colors().button_background_selected)
                    .opacity(0.55),
            );
        }
        root
    }

    fn panel_layout_visible(&self, position: PanelPosition, node: &PanelLayout) -> bool {
        match node {
            PanelLayout::Panel(id) => self.shell.layout.group(id).is_some_and(|panel| {
                panel.open
                    || self.shell.layout.zone(position).closing_since(id).is_some()
                    || self.shell.layout.zone(position).hiding_since(id).is_some()
            }),
            PanelLayout::Arrangement { children, .. } => children
                .iter()
                .any(|child| self.panel_layout_visible(position, child)),
        }
    }

    fn panel_layout_factor(
        &self,
        position: PanelPosition,
        node: &PanelLayout,
        now: std::time::Instant,
    ) -> f32 {
        match node {
            PanelLayout::Panel(id) => self
                .shell
                .layout
                .zone(position)
                .closing_since(id)
                .or_else(|| self.shell.layout.zone(position).hiding_since(id))
                .map_or(1.0, |started| {
                    let duration =
                        crate::shell::panel_transition::PANEL_ANIMATION_DURATION.as_secs_f32();
                    (1.0 - now.saturating_duration_since(started).as_secs_f32() / duration)
                        .clamp(0.0, 1.0)
                }),
            PanelLayout::Arrangement { children, .. } => children
                .iter()
                .filter(|child| self.panel_layout_visible(position, child))
                .map(|child| self.panel_layout_factor(position, child, now))
                .fold(0.0, f32::max),
        }
    }

    fn render_panel_layout(
        &self,
        position: PanelPosition,
        node: &PanelLayout,
        path: Vec<usize>,
        now: std::time::Instant,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        match node {
            PanelLayout::Panel(id) => self
                .shell
                .layout
                .group(id)
                .filter(|panel| {
                    panel.open
                        || self.shell.layout.zone(position).closing_since(id).is_some()
                        || self.shell.layout.zone(position).hiding_since(id).is_some()
                })
                .map_or_else(
                    || div().size(px(0.0)),
                    |panel| {
                        self.render_panel_window_for_group(panel, id, cx)
                            .opacity(self.panel_layout_factor(position, node, now))
                    },
                ),
            PanelLayout::Arrangement {
                axis,
                children,
                weights,
            } => {
                let mut row = div()
                    .relative()
                    .flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .overflow_hidden();
                row = match axis {
                    Axis::Horizontal => row.flex_row(),
                    Axis::Vertical => row.flex_col(),
                };
                let shell = cx.entity().downgrade();
                let key = (position, path.clone());
                row = row.child(
                    gpui::canvas(
                        move |bounds, _, cx| {
                            let _ = shell.update(cx, |shell, _| {
                                shell
                                    .arrangement_bounds
                                    .borrow_mut()
                                    .insert(key.clone(), bounds);
                            });
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size_full(),
                );
                let visible = children
                    .iter()
                    .map(|child| self.panel_layout_visible(position, child))
                    .collect::<Vec<_>>();
                let mut previous = None;
                for (index, child) in children.iter().enumerate() {
                    if !visible[index] {
                        continue;
                    }
                    if let Some(previous_index) = previous {
                        row = row.child(self.render_panel_boundary(
                            position,
                            path.clone(),
                            previous_index,
                            index,
                            *axis,
                            cx,
                        ));
                    }
                    let mut child_path = path.clone();
                    child_path.push(index);
                    let mut rendered = self
                        .render_panel_layout(position, child, child_path, now, cx)
                        .flex_basis(px(0.0))
                        .min_w(px(0.0))
                        .min_h(px(0.0))
                        .overflow_hidden();
                    rendered.style().flex_grow =
                        Some(weights[index] * self.panel_layout_factor(position, child, now));
                    row = row.child(rendered);
                    previous = Some(index);
                }
                row
            }
        }
    }

    fn render_panel_boundary(
        &self,
        position: PanelPosition,
        path: Vec<usize>,
        boundary: usize,
        next: usize,
        axis: Axis,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let mut handle = div()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .hover(|style| style.bg(theme::colors().button_background_selected))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.start_panel_resize(
                        position,
                        path.clone(),
                        (boundary, next),
                        axis,
                        event.position,
                        cx,
                    );
                }),
            );
        handle = match axis {
            Axis::Horizontal => handle
                .w(tokens::spacing::GAP)
                .h_full()
                .cursor(CursorStyle::ResizeLeftRight),
            Axis::Vertical => handle
                .h(tokens::spacing::GAP)
                .w_full()
                .cursor(CursorStyle::ResizeUpDown),
        };
        handle
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
            div()
                .w(px(self.size.0))
                .h(px(self.size.1))
                .flex()
                .child(self.shell.update(cx, |shell, cx| {
                    shell
                        .render_zone(PanelPosition::Main, std::time::Instant::now(), cx)
                        .flex_1()
                }))
        }
    }

    #[gpui::test]
    fn gpui_panels_fit_after_window_shrink_and_restore(cx: &mut gpui::TestAppContext) {
        let window = cx.add_window(|_, cx| {
            let document = cx.new(|_| Empty);
            let shell = cx.new(|cx| Shell::new(document.into(), HashMap::new(), cx));
            shell.update(cx, |shell, _| {
                let a = PanelTab::tool("a");
                let b = PanelTab::tool("b");
                let c = PanelTab::tool("c");
                let first = shell.shell.layout.open_tab(PanelPosition::Main, a).unwrap();
                shell
                    .shell
                    .layout
                    .open_tab(PanelPosition::Main, b.clone())
                    .unwrap();
                shell
                    .shell
                    .layout
                    .open_tab(PanelPosition::Main, c.clone())
                    .unwrap();
                let second = shell
                    .shell
                    .layout
                    .drop_tab(
                        &first,
                        &b,
                        &PanelDrop::Edge {
                            panel: first.clone(),
                            edge: PanelEdge::Right,
                        },
                        (1000.0, 700.0),
                        std::time::Instant::now(),
                    )
                    .unwrap();
                shell
                    .shell
                    .layout
                    .drop_tab(
                        &first,
                        &c,
                        &PanelDrop::Edge {
                            panel: second,
                            edge: PanelEdge::Bottom,
                        },
                        (1000.0, 700.0),
                        std::time::Instant::now(),
                    )
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
        let before = shell.read_with(cx, |shell, _| shell.shell.layout.capture());
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
                assert_eq!(shell.shell.layout.capture(), before);
                shell.group_bounds.borrow().clone()
            });
            assert_eq!(bounds.len(), 3);
            if size == (1000.0, 700.0) {
                assert!(bounds.values().all(|bounds| bounds.size.width > px(32.0) && bounds.size.height > px(32.0)));
            }
            for bounds in bounds.values() {
                assert!(f32::from(bounds.origin.x) >= -0.1 && f32::from(bounds.origin.y) >= -0.1);
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

    #[gpui::test]
    fn preview_rectangle_matches_the_committed_gpui_panel(cx: &mut gpui::TestAppContext) {
        let window = cx.add_window(|_, cx| {
            let document = cx.new(|_| Empty);
            let shell = cx.new(|cx| Shell::new(document.into(), HashMap::new(), cx));
            shell.update(cx, |shell, _| {
                shell
                    .shell
                    .layout
                    .open_tab(PanelPosition::Main, PanelTab::tool("a"))
                    .unwrap();
                shell
                    .shell
                    .layout
                    .open_tab(PanelPosition::Main, PanelTab::tool("b"))
                    .unwrap();
            });
            Fixture {
                shell,
                size: (900.0, 600.0),
            }
        });
        let shell = window
            .update(cx, |fixture, _, _| fixture.shell.clone())
            .unwrap();
        let (target, expected) = shell.update(cx, |shell, _| {
            let tab = PanelTab::tool("b");
            let source = shell.shell.layout.find_tab(&tab).unwrap();
            let drop = PanelDrop::Edge {
                panel: source.clone(),
                edge: PanelEdge::Right,
            };
            let now = std::time::Instant::now();
            let (preview, id) = shell
                .shell
                .layout
                .preview_drop(&source, &tab, &drop, (900.0, 600.0), now)
                .unwrap();
            let rect = preview
                .zone(PanelPosition::Main)
                .panel_rect(&id, (900.0, 600.0), f32::from(tokens::spacing::GAP))
                .unwrap();
            assert_eq!(
                shell
                    .shell
                    .layout
                    .drop_tab(&source, &tab, &drop, (900.0, 600.0), now),
                Ok(id.clone())
            );
            (id, rect)
        });
        window.update(cx, |_, window, _| window.refresh()).unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear();
        })
        .unwrap();
        let actual = shell.read_with(cx, |shell, _| shell.group_bounds.borrow()[&target]);
        assert!(
            (f32::from(actual.origin.x) - expected.0).abs() < 1.0,
            "actual={actual:?} expected={expected:?}"
        );
        assert!((f32::from(actual.origin.y) - expected.1).abs() < 1.0);
        assert!((f32::from(actual.size.width) - expected.2).abs() < 1.0);
        assert!((f32::from(actual.size.height) - expected.3).abs() < 1.0);
    }
}
