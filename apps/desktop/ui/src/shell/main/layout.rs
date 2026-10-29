use super::*;
use crate::shell::panel_layout::Axis;

impl Shell {
    /// 全Zoneを同じPanel配置Rendererで描画する。
    pub(crate) fn render_main(&self, window: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let now = std::time::Instant::now();
        if self.shell.layout.has_closing_panels() || self.shell.bottom_span_is_animating(now) {
            window.request_animation_frame();
        }
        let viewport = window.viewport_size();
        let geometry = crate::shell::dock_geometry::DockGeometry::calculate(
            &self.shell.layout,
            (f32::from(viewport.width), f32::from(viewport.height)),
            self.detached,
            (
                self.shell.bottom_spans_left_layout(now),
                self.shell.bottom_spans_right_layout(now),
            ),
            now,
        );
        let mut frame = div()
            .relative()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .overflow_hidden();
        for (&position, rect) in &geometry.zones {
            frame = frame.child(
                self.render_zone(position, &geometry, now, cx)
                    .absolute()
                    .left(px(rect.x - geometry.frame.x))
                    .top(px(rect.y - geometry.frame.y))
                    .w(px(rect.width))
                    .h(px(rect.height)),
            );
            self.zone_bounds.borrow_mut().insert(
                position,
                gpui::Bounds::new(
                    point(px(rect.x), px(rect.y)),
                    gpui::size(px(rect.width), px(rect.height)),
                ),
            );
        }
        for (id, rect) in &geometry.panels {
            let bounds = gpui::Bounds::new(
                point(px(rect.x), px(rect.y)),
                gpui::size(px(rect.width), px(rect.height)),
            );
            self.panel_bounds.borrow_mut().insert(id.clone(), bounds);
            if let Some(position) = self.shell.layout.zone_for_panel(id) {
                self.zone_panel_bounds
                    .borrow_mut()
                    .entry(position)
                    .or_insert(bounds);
            }
        }
        let gap = geometry.gap;
        if let Some(left) = geometry.zones.get(&ZonePosition::Left) {
            frame = frame.child(
                self.render_resize_handle(ResizeTarget::Left, false, cx)
                    .absolute()
                    .left(px(left.x + left.width))
                    .top(px(left.y - geometry.frame.y))
                    .w(px(gap))
                    .h(px(left.height)),
            );
        }
        if let Some(right) = geometry.zones.get(&ZonePosition::Right) {
            frame = frame.child(
                self.render_resize_handle(ResizeTarget::Right, false, cx)
                    .absolute()
                    .left(px((right.x - gap).max(0.0)))
                    .top(px(right.y - geometry.frame.y))
                    .w(px(gap))
                    .h(px(right.height)),
            );
        }
        if let Some(bottom) = geometry.zones.get(&ZonePosition::Bottom) {
            frame = frame.child(
                self.render_resize_handle(ResizeTarget::Bottom, true, cx)
                    .absolute()
                    .left(px(bottom.x))
                    .top(px((bottom.y - geometry.frame.y - gap).max(0.0)))
                    .w(px(bottom.width))
                    .h(px(gap)),
            );
        }
        *self.dock_geometry.borrow_mut() = Some(geometry);
        frame
    }

    fn render_zone(
        &self,
        position: ZonePosition,
        geometry: &crate::shell::dock_geometry::DockGeometry,
        now: std::time::Instant,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let zone = self.shell.layout.zone(position);
        let zone_rect = geometry.zones[&position];
        let mut root = div()
            .relative()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .overflow_hidden();
        for (id, rect) in &geometry.panels {
            if let Some(panel) = self
                .shell
                .layout
                .panel(id)
                .filter(|_| self.shell.layout.zone_for_panel(id) == Some(position))
            {
                root = root.child(
                    self.render_panel_window(position, panel, id, cx)
                        .absolute()
                        .left(px(rect.x - zone_rect.x))
                        .top(px(rect.y - zone_rect.y))
                        .w(px(rect.width))
                        .h(px(rect.height))
                        .opacity(zone.panel_visual_factor(id, now)),
                );
            }
        }
        for boundary in geometry
            .panel_boundaries
            .iter()
            .filter(|boundary| boundary.zone == position)
        {
            let rect = boundary.rect;
            root = root.child(
                self.render_panel_boundary(boundary.clone(), cx)
                    .absolute()
                    .left(px(rect.x - zone_rect.x))
                    .top(px(rect.y - zone_rect.y))
                    .w(px(rect.width))
                    .h(px(rect.height)),
            );
        }
        if geometry.panels.keys().all(|id| {
            self.shell
                .layout
                .zone_for_panel(id)
                .is_none_or(|zone| zone != position)
        }) {
            root = root.child(
                div()
                    .id(ElementId::Name(format!("empty-zone-{position:?}").into()))
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(theme::colors(cx).background_secondary)
                    .child(
                        div()
                            .id(ElementId::Name(
                                format!("empty-zone-add-{position:?}").into(),
                            ))
                            .px_2()
                            .py_1()
                            .child(self.t("panel.open-tool"))
                            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                                this.open_empty_zone_picker(position, event.position(), window, cx);
                            })),
                    ),
            );
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
                    .bg(theme::colors(cx).button_background_selected)
                    .opacity(0.55),
            );
        } else if self.drop_target.as_ref().is_some_and(
            |preview| matches!(preview.target, PanelDrop::IntoZone(target) if target == position),
        ) {
            root = root.child(
                div()
                    .absolute()
                    .inset_0()
                    .bg(theme::colors(cx).button_background_selected)
                    .opacity(0.55),
            );
        }
        root
    }

    fn render_panel_boundary(
        &self,
        boundary: crate::shell::dock_geometry::PanelBoundary,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let axis = boundary.axis;
        let handle = div()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .hover(|style| style.bg(theme::colors(cx).button_background_selected))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.start_panel_resize(boundary.clone(), event.position, cx);
                }),
            );
        match axis {
            Axis::Horizontal => handle.cursor(CursorStyle::ResizeLeftRight),
            Axis::Vertical => handle.cursor(CursorStyle::ResizeUpDown),
        }
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
                    {
                        let geometry = crate::shell::dock_geometry::DockGeometry::single_zone(
                            &shell.shell.layout,
                            ZonePosition::Left,
                            self.size,
                        );
                        shell.render_zone(
                            ZonePosition::Left,
                            &geometry,
                            std::time::Instant::now(),
                            cx,
                        )
                    }
                    .flex_1()
                }))
        }
    }

    #[gpui::test]
    fn gpui_panels_fit_after_window_shrink_and_restore(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            crate::theme::init(&crate::extension_ui::ThemeId::new("lapis.dark"), None, cx)
        });
        let window = cx.add_window(|_, cx| {
            let document = cx.new(|_| Empty);
            let shell = cx.new(|cx| Shell::new(document.into(), HashMap::new(), cx));
            shell.update(cx, |shell, _| {
                let a = PanelTab::tool("a");
                let b = PanelTab::tool("b");
                let c = PanelTab::tool("c");
                let first = shell.shell.layout.open_tab(ZonePosition::Left, a).unwrap();
                shell
                    .shell
                    .layout
                    .open_tab(ZonePosition::Left, b.clone())
                    .unwrap();
                shell
                    .shell
                    .layout
                    .open_tab(ZonePosition::Left, c.clone())
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
                            edge: PanelEdge::Right,
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
                        .update(cx, |shell, _| shell.panel_bounds.borrow_mut().clear());
                    window.refresh();
                })
                .unwrap();
            cx.update_window(window.into(), |_, window, cx| {
                window.draw(cx).clear();
            })
            .unwrap();
            let bounds = shell.read_with(cx, |shell, _| {
                assert_eq!(shell.shell.layout.capture(), before);
                shell.panel_bounds.borrow().clone()
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
        cx.update(|cx| {
            crate::theme::init(&crate::extension_ui::ThemeId::new("lapis.dark"), None, cx)
        });
        let window = cx.add_window(|_, cx| {
            let document = cx.new(|_| Empty);
            let shell = cx.new(|cx| Shell::new(document.into(), HashMap::new(), cx));
            shell.update(cx, |shell, _| {
                shell
                    .shell
                    .layout
                    .open_tab(ZonePosition::Left, PanelTab::tool("a"))
                    .unwrap();
                shell
                    .shell
                    .layout
                    .open_tab(ZonePosition::Left, PanelTab::tool("b"))
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
            let geometry = crate::shell::dock_geometry::DockGeometry::single_zone(
                &preview,
                ZonePosition::Left,
                (900.0, 600.0),
            );
            let rect = geometry.panels[&id];
            let rect = (rect.x, rect.y, rect.width, rect.height);
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
        let actual = shell.read_with(cx, |shell, _| shell.panel_bounds.borrow()[&target]);
        assert!(
            (f32::from(actual.origin.x) - expected.0).abs() < 1.0,
            "actual={actual:?} expected={expected:?}"
        );
        assert!((f32::from(actual.origin.y) - expected.1).abs() < 1.0);
        assert!((f32::from(actual.size.width) - expected.2).abs() < 1.0);
        assert!((f32::from(actual.size.height) - expected.3).abs() < 1.0);
    }
}
