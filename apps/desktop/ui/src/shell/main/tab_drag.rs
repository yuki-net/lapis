use super::*;

impl Shell {
    fn move_tab_drag(
        &mut self,
        position: Point<Pixels>,
        screen: Option<crate::window_host::drag::ScreenPoint>,
        cx: &mut App,
    ) -> bool {
        let changed = self.update_drop_target(position);
        if let (Some(preview), Some(point)) = (self.drag_preview, screen) {
            let _ = preview.update(cx, |_, window, _| {
                crate::window_host::drag_preview::position(window, point);
            });
        }
        changed
    }

    pub(super) fn drop_target_size(&self, group: &TabGroupId) -> (f32, f32) {
        let measured = self
            .group_bounds
            .borrow()
            .get(group)
            .map(|bounds| (f32::from(bounds.size.width), f32::from(bounds.size.height)))
            .unwrap_or((0.0, 0.0));
        if self.shell.layout.has_variable_splits() || self.detached {
            return measured;
        }
        self.layout_bounds
            .borrow()
            .and_then(|bounds| {
                self.shell.layout.allocated_group_size(
                    group,
                    (f32::from(bounds.size.width), f32::from(bounds.size.height)),
                    std::time::Instant::now(),
                )
            })
            .map_or(measured, |projected| {
                (measured.0.min(projected.0), measured.1.min(projected.1))
            })
    }

    /// Installs capture handlers that release tab drags outside the source window or cancel them on mouse down.
    /// Releases inside the source window are left to panel drop handlers.
    pub(super) fn tab_drag_events(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let shell = cx.entity().downgrade();
        gpui::canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let released = shell.clone();
                let moving = shell.clone();
                // SetCapture 中は、Window 外の MouseUp も元 Window で受け取る。
                window.on_mouse_event(move |event: &gpui::MouseUpEvent, phase, window, cx| {
                    if phase != gpui::DispatchPhase::Capture || event.button != MouseButton::Left {
                        return;
                    }
                    let active = cx.has_active_drag();
                    let _ = released.update(cx, |shell, cx| {
                        shell.close_drag_preview(cx);
                        let drag = shell.release_tab_drag_at(event.position, active);
                        cx.notify();
                        let Some(drag) = drag else {
                            return;
                        };
                        let point =
                            crate::window_host::drag::screen_position(event.position, window);
                        let inside = gpui::Bounds::new(Point::default(), window.viewport_size())
                            .contains(&event.position);
                        if inside
                            && point.is_none_or(|point| {
                                crate::window_host::drag::is_source_window(window, point)
                            })
                        {
                            // 同じ Window 内は既存の Panel on_drop に任せる。
                            return;
                        }
                        shell.drop_target = None;
                        if let Some(point) = point {
                            cx.emit(UiCommand::ReleaseTab(drag, point));
                        }
                        cx.stop_active_drag(window);
                        cx.stop_propagation();
                    });
                });
                let moving_shell = moving.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase != gpui::DispatchPhase::Capture || !cx.has_active_drag() {
                        return;
                    }
                    let point = crate::window_host::drag::screen_position(event.position, window);
                    let _ = moving_shell.update(cx, |shell, cx| {
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

    /// Clears a pending tab drag and stops GPUI's active drag when one was recorded.
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
            crate::window_host::drag_preview::close(window);
        });
    }

    fn update_drop_target(&mut self, point: Point<Pixels>) -> bool {
        let Some(drag) = self.tab_drag.current() else {
            return self.drop_target.take().is_some();
        };
        if !drag.belongs_to(self.group) {
            return self.drop_target.take().is_some();
        }
        let target = self
            .group_bounds
            .borrow()
            .iter()
            .find_map(|(group, bounds)| {
                if !self
                    .shell
                    .layout
                    .group(group)
                    .is_some_and(|panel| panel.is_visible(std::time::Instant::now()))
                    || !bounds.contains(&point)
                    || bounds.size.width <= px(0.0)
                    || bounds.size.height <= px(0.0)
                {
                    return None;
                }
                let x = f32::from(point.x - bounds.origin.x) / f32::from(bounds.size.width);
                let y = f32::from(point.y - bounds.origin.y) / f32::from(bounds.size.height);
                let zone = DropZone::from_normalized(x, y);
                let mut candidate = self.shell.layout.clone();
                let error = candidate
                    .drop_tab(
                        &drag.source_group,
                        group,
                        &drag.tab,
                        zone,
                        self.drop_target_size(group),
                    )
                    .err();
                Some(DropTarget {
                    group: group.clone(),
                    zone,
                    error,
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
        // 最後のMouseMoveと異なる座標で離しても、古いOverlayを残さない。
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

    #[gpui::test]
    fn preview_rejects_small_split_and_ignores_hidden_group_bounds(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let document = cx.new(|_| DocumentFixture);
            let shell = cx.new(|cx| Shell::new(document.into(), HashMap::new(), cx));
            shell.update(cx, |shell, cx| {
                shell.group = Some(1.into());
                let source = shell
                    .shell
                    .layout
                    .primary_group_id(PanelPosition::Left)
                    .clone();
                let target = shell
                    .shell
                    .layout
                    .primary_group_id(PanelPosition::Main)
                    .clone();
                let tab = shell.shell.layout.group(&source).unwrap().tabs[0].clone();
                shell.tab_drag.begin(DraggedPanelTab {
                    group: shell.group,
                    source_host: shell.host,
                    source_group: source.clone(),
                    tab: tab.clone(),
                });
                let before = shell.shell.layout.capture_view_state();
                shell.group_bounds.borrow_mut().insert(
                    target.clone(),
                    gpui::Bounds::new(point(px(0.0), px(0.0)), gpui::size(px(407.0), px(300.0))),
                );
                assert!(shell.move_tab_drag(point(px(400.0), px(150.0)), None, cx));
                assert_eq!(
                    shell.drop_target.as_ref().unwrap().error,
                    Some(LayoutError::InsufficientSpace)
                );
                shell.move_panel_tab_to_group(
                    source.clone(),
                    target.clone(),
                    tab.clone(),
                    DropZone::Right,
                    cx,
                );
                assert_eq!(shell.shell.layout.capture_view_state(), before);
                // 固定配置では広くても、可変木へ切替後に狭くなる初回Splitを拒否する。
                shell.group_bounds.borrow_mut().insert(
                    target.clone(),
                    gpui::Bounds::new(point(px(0.0), px(0.0)), gpui::size(px(900.0), px(600.0))),
                );
                assert!(shell.move_tab_drag(point(px(890.0), px(300.0)), None, cx));
                assert!(shell.drop_target.as_ref().unwrap().error.is_none());
                *shell.layout_bounds.borrow_mut() = Some(gpui::Bounds::new(
                    point(px(0.0), px(0.0)),
                    gpui::size(px(500.0), px(600.0)),
                ));
                shell.update_drop_target(point(px(890.0), px(300.0)));
                assert_eq!(
                    shell.drop_target.as_ref().unwrap().error,
                    Some(LayoutError::InsufficientSpace)
                );
                shell.move_panel_tab_to_group(source.clone(), target, tab, DropZone::Right, cx);
                assert_eq!(shell.shell.layout.capture_view_state(), before);
                shell.group_bounds.borrow_mut().clear();
                shell
                    .shell
                    .layout
                    .group_mut(&source)
                    .unwrap()
                    .set_open_immediate(false);
                shell.group_bounds.borrow_mut().insert(
                    source,
                    gpui::Bounds::new(point(px(0.0), px(0.0)), gpui::size(px(900.0), px(600.0))),
                );
                shell.update_drop_target(point(px(20.0), px(300.0)));
                assert!(shell.drop_target.is_none());
            });
        });
    }

    struct DocumentFixture;
    impl Render for DocumentFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    #[gpui::test]
    fn releasing_in_a_gap_clears_preview_without_committing_and_allows_next_drag(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let document = cx.new(|_| DocumentFixture);
            let shell = cx.new(|cx| Shell::new(document.into(), HashMap::new(), cx));
            shell.update(cx, |shell, _| {
                shell.group = Some(1.into());
                let source = shell
                    .shell
                    .layout
                    .primary_group_id(PanelPosition::Left)
                    .clone();
                let target = shell
                    .shell
                    .layout
                    .primary_group_id(PanelPosition::Main)
                    .clone();
                let tab = shell.shell.layout.group(&source).unwrap().tabs[0].clone();
                let before = shell.shell.layout.capture_view_state();
                let drag = DraggedPanelTab {
                    group: shell.group,
                    source_host: 0,
                    source_group: source,
                    tab,
                };
                shell.group_bounds.borrow_mut().insert(
                    target.clone(),
                    gpui::Bounds::new(point(px(0.0), px(0.0)), gpui::size(px(900.0), px(600.0))),
                );
                shell.tab_drag.begin(drag.clone());
                assert!(shell.update_drop_target(point(px(20.0), px(300.0))));
                assert!(shell.drop_target.is_some());
                assert!(
                    shell
                        .release_tab_drag_at(point(px(950.0), px(650.0)), true)
                        .is_some()
                );
                assert!(shell.drop_target.is_none());
                assert_eq!(shell.shell.layout.capture_view_state(), before);
                shell.tab_drag.begin(drag);
                assert!(
                    shell
                        .release_tab_drag_at(point(px(450.0), px(300.0)), true)
                        .is_some()
                );
                assert_eq!(
                    shell
                        .drop_target
                        .as_ref()
                        .map(|drop| (&drop.group, drop.zone)),
                    Some((&target, DropZone::Center))
                );
                assert_eq!(shell.shell.layout.capture_view_state(), before);
            });
        });
    }
}
