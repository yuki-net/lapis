use super::*;
use crate::shell::panel_layout::{EdgeCandidate, insertion_edges, nearest_edge};

impl Shell {
    #[cfg(all(target_os = "windows", debug_assertions))]
    pub(crate) fn smoke_drop_preview(&self) -> String {
        format!("{:?}", self.drop_target)
    }
    fn move_tab_drag(
        &mut self,
        position: Point<Pixels>,
        screen: Option<crate::window_host::drag::ScreenPoint>,
        cx: &mut App,
    ) -> bool {
        let changed = self.update_drop_target(position);
        if let (Some(preview), Some(point)) = (self.drag_preview, screen) {
            let _ = preview.update(cx, |_, window, _| {
                crate::window_host::drag_preview::position(window, point)
            });
        }
        changed
    }

    pub(super) fn target_zone_size(&self, target: &PanelDrop) -> (f32, f32) {
        let position = match target {
            PanelDrop::Into(id) => self.shell.layout.group(id).map(|panel| panel.position),
            PanelDrop::Edge { panel, .. } => {
                self.shell.layout.group(panel).map(|panel| panel.position)
            }
            PanelDrop::IntoZone(position) => Some(*position),
        };
        position
            .and_then(|position| self.zone_bounds.borrow().get(&position).copied())
            .map(|bounds| (f32::from(bounds.size.width), f32::from(bounds.size.height)))
            .unwrap_or((0.0, 0.0))
    }

    pub(super) fn tab_drag_events(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let shell = cx.entity().downgrade();
        gpui::canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let released = shell.clone();
                let moving = shell.clone();
                window.on_mouse_event(move |event: &gpui::MouseUpEvent, phase, window, cx| {
                    if phase != gpui::DispatchPhase::Capture || event.button != MouseButton::Left {
                        return;
                    }
                    let active = cx.has_active_drag();
                    let _ = released.update(cx, |shell, cx| {
                        shell.close_drag_preview(cx);
                        let drag = shell.release_tab_drag_at(event.position, active);
                        let Some(drag) = drag else {
                            cx.notify();
                            return;
                        };
                        let screen =
                            crate::window_host::drag::screen_position(event.position, window);
                        let inside = gpui::Bounds::new(Point::default(), window.viewport_size())
                            .contains(&event.position);
                        if inside
                            && screen.is_none_or(|point| {
                                crate::window_host::drag::is_source_window(window, point)
                            })
                        {
                            if let Some(preview) = shell.drop_target.take()
                                && !preview.invalid
                                && drag.source_host == shell.host
                            {
                                shell.move_panel_tab_to_group(
                                    drag.source_group.clone(),
                                    drag.tab.clone(),
                                    preview.target,
                                    cx,
                                );
                            }
                        } else {
                            shell.drop_target = None;
                            if let Some(point) = screen {
                                cx.emit(UiCommand::ReleaseTab(drag, point));
                            }
                        }
                        cx.stop_active_drag(window);
                        cx.stop_propagation();
                        cx.notify();
                    });
                });
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase != gpui::DispatchPhase::Capture || !cx.has_active_drag() {
                        return;
                    }
                    let point = crate::window_host::drag::screen_position(event.position, window);
                    let _ = moving.update(cx, |shell, cx| {
                        if shell.move_tab_drag(event.position, point, cx) {
                            cx.notify();
                        }
                    });
                });
                window.on_mouse_event(move |_: &MouseDownEvent, phase, window, cx| {
                    if phase == gpui::DispatchPhase::Capture {
                        let _ = shell.update(cx, |shell, cx| shell.cancel_tab_drag(window, cx));
                    }
                });
            },
        )
        .absolute()
        .size_full()
    }

    pub(crate) fn cancel_tab_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.drop_target = None;
        if self.tab_drag.cancel() {
            self.close_drag_preview(cx);
            cx.stop_active_drag(window);
        }
    }

    pub(crate) fn close_drag_preview(&mut self, cx: &mut App) {
        let Some(preview) = self.drag_preview.take() else {
            return;
        };
        let _ = preview.update(cx, |_, window, _| {
            crate::window_host::drag_preview::close(window)
        });
    }

    fn update_drop_target(&mut self, point: Point<Pixels>) -> bool {
        let Some(drag) = self.tab_drag.current() else {
            return self.drop_target.take().is_some();
        };
        if !drag.belongs_to(self.group) {
            return self.drop_target.take().is_some();
        }
        let groups = self.group_bounds.borrow();
        let mut edges = Vec::new();
        let mut panels = Vec::new();
        for (id, bounds) in groups.iter() {
            let Some(panel) = self.shell.layout.group(id) else {
                continue;
            };
            if !panel.open
                || !self.shell.layout.zone_open(panel.position)
                || bounds.size.width <= px(0.0)
                || bounds.size.height <= px(0.0)
            {
                continue;
            }
            let x = f32::from(bounds.origin.x);
            let y = f32::from(bounds.origin.y);
            let w = f32::from(bounds.size.width);
            let h = f32::from(bounds.size.height);
            for &edge in insertion_edges(panel.position) {
                let (start, end) = match edge {
                    PanelEdge::Left => ((x, y), (x, y + h)),
                    PanelEdge::Right => ((x + w, y), (x + w, y + h)),
                    PanelEdge::Top => ((x, y), (x + w, y)),
                    PanelEdge::Bottom => ((x, y + h), (x + w, y + h)),
                };
                edges.push(EdgeCandidate {
                    panel: id.clone(),
                    edge,
                    start,
                    end,
                });
            }
            if bounds.contains(&point) {
                panels.push(id.clone());
            }
        }
        panels.sort();
        let target = nearest_edge(
            &edges,
            (f32::from(point.x), f32::from(point.y)),
            f32::from(tokens::spacing::GAP) * 2.5,
        )
        .map(|candidate| PanelDrop::Edge {
            panel: candidate.panel.clone(),
            edge: candidate.edge,
        })
        .or_else(|| panels.into_iter().next().map(PanelDrop::Into))
        .or_else(|| {
            (!self.shell.layout.zone_has_panels(PanelPosition::Main)
                && self
                    .zone_bounds
                    .borrow()
                    .get(&PanelPosition::Main)
                    .is_some_and(|bounds| bounds.contains(&point)))
            .then_some(PanelDrop::IntoZone(PanelPosition::Main))
        });
        drop(groups);
        let target = target.and_then(|target| {
            let size = self.target_zone_size(&target);
            let zone = match &target {
                PanelDrop::Into(id) => self.shell.layout.group(id)?.position,
                PanelDrop::Edge { panel, .. } => self.shell.layout.group(panel)?.position,
                PanelDrop::IntoZone(position) => *position,
            };
            let result = self.shell.layout.preview_drop(
                &drag.source_group,
                &drag.tab,
                &target,
                size,
                std::time::Instant::now(),
            );
            let rect =
                match (&target, &result) {
                    (PanelDrop::Edge { .. }, Ok((candidate, id))) => candidate
                        .zone(zone)
                        .panel_rect(id, size, f32::from(tokens::spacing::GAP)),
                    _ => None,
                };
            Some(DropPreview {
                target,
                invalid: result.is_err(),
                zone,
                rect,
            })
        });
        if self.drop_target == target {
            false
        } else {
            self.drop_target = target;
            true
        }
    }

    fn release_tab_drag_at(
        &mut self,
        point: Point<Pixels>,
        active: bool,
    ) -> Option<DraggedPanelTab> {
        if active {
            self.update_drop_target(point);
        }
        let drag = self.tab_drag.release(active);
        if drag.is_none() {
            self.drop_target = None;
        }
        drag
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct DocumentFixture;
    impl Render for DocumentFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    #[gpui::test]
    fn preview_rejects_small_panels_and_ignores_hidden_bounds(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let document = cx.new(|_| DocumentFixture);
            let shell = cx.new(|cx| Shell::new(document.into(), HashMap::new(), cx));
            shell.update(cx, |shell, cx| {
                shell.group = Some(1.into());
                let source = shell
                    .shell
                    .layout
                    .first_panel_in_zone(PanelPosition::Left)
                    .unwrap();
                let target = shell
                    .shell
                    .layout
                    .open_tab(PanelPosition::Main, PanelTab::tool("target"))
                    .unwrap();
                let tab = shell.shell.layout.group(&source).unwrap().tabs[0].clone();
                shell.tab_drag.begin(DraggedPanelTab {
                    group: shell.group,
                    source_host: shell.host,
                    source_group: source.clone(),
                    tab: tab.clone(),
                });
                shell.group_bounds.borrow_mut().insert(
                    target.clone(),
                    gpui::Bounds::new(point(px(0.0), px(0.0)), gpui::size(px(50.0), px(300.0))),
                );
                shell.zone_bounds.borrow_mut().insert(
                    PanelPosition::Main,
                    gpui::Bounds::new(point(px(0.0), px(0.0)), gpui::size(px(50.0), px(300.0))),
                );
                let before = shell.shell.layout.capture();
                assert!(shell.move_tab_drag(point(px(49.0), px(150.0)), None, cx));
                assert!(shell.drop_target.as_ref().unwrap().invalid);
                shell.move_panel_tab_to_group(
                    source.clone(),
                    tab.clone(),
                    PanelDrop::Edge {
                        panel: target.clone(),
                        edge: PanelEdge::Right,
                    },
                    cx,
                );
                assert_eq!(shell.shell.layout.capture(), before);
                shell.group_bounds.borrow_mut().insert(
                    target.clone(),
                    gpui::Bounds::new(point(px(0.0), px(0.0)), gpui::size(px(900.0), px(600.0))),
                );
                shell.zone_bounds.borrow_mut().insert(
                    PanelPosition::Main,
                    gpui::Bounds::new(point(px(0.0), px(0.0)), gpui::size(px(900.0), px(600.0))),
                );
                assert!(shell.move_tab_drag(point(px(890.0), px(300.0)), None, cx));
                assert!(!shell.drop_target.as_ref().unwrap().invalid);
                shell.shell.layout.set_panel_open(&target, false);
                shell.update_drop_target(point(px(890.0), px(300.0)));
                assert!(shell.drop_target.is_none());
            });
        });
    }

    #[gpui::test]
    fn release_in_a_gap_clears_preview_and_a_panel_face_moves_tabs(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let document = cx.new(|_| DocumentFixture);
            let shell = cx.new(|cx| Shell::new(document.into(), HashMap::new(), cx));
            shell.update(cx, |shell, _| {
                shell.group = Some(1.into());
                let source = shell
                    .shell
                    .layout
                    .first_panel_in_zone(PanelPosition::Left)
                    .unwrap();
                let target = shell
                    .shell
                    .layout
                    .open_tab(PanelPosition::Main, PanelTab::tool("target"))
                    .unwrap();
                let tab = shell.shell.layout.group(&source).unwrap().tabs[0].clone();
                let drag = DraggedPanelTab {
                    group: shell.group,
                    source_host: shell.host,
                    source_group: source,
                    tab,
                };
                let bounds =
                    gpui::Bounds::new(point(px(0.0), px(0.0)), gpui::size(px(900.0), px(600.0)));
                shell
                    .group_bounds
                    .borrow_mut()
                    .insert(target.clone(), bounds);
                shell
                    .zone_bounds
                    .borrow_mut()
                    .insert(PanelPosition::Main, bounds);
                let before = shell.shell.layout.capture();
                shell.tab_drag.begin(drag.clone());
                shell.update_drop_target(point(px(450.0), px(300.0)));
                assert_eq!(
                    shell.drop_target.as_ref().map(|preview| &preview.target),
                    Some(&PanelDrop::Into(target.clone()))
                );
                assert!(
                    shell
                        .release_tab_drag_at(point(px(950.0), px(650.0)), true)
                        .is_some()
                );
                assert!(shell.drop_target.is_none());
                assert_eq!(shell.shell.layout.capture(), before);
                shell.tab_drag.begin(drag);
                assert!(
                    shell
                        .release_tab_drag_at(point(px(450.0), px(300.0)), true)
                        .is_some()
                );
                assert_eq!(
                    shell.drop_target.as_ref().map(|preview| &preview.target),
                    Some(&PanelDrop::Into(target))
                );
                assert_eq!(shell.shell.layout.capture(), before);
            });
        });
    }
}
