use super::*;
use crate::components::{Menu, MenuEntry, MenuItemSpec};

#[derive(Clone, Copy)]
enum TabAction {
    Close,
    CloseOthers,
    CloseAll,
    Pin,
}

impl Shell {
    pub(super) fn render_tab_context(
        &self,
        (panel_id, tab, anchor): (PanelId, PanelTab, Point<Pixels>),
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let pinned = self.shell.layout.tab_is_pinned(&tab);
        let items = [
            ("tab.context.close", Some(TabAction::Close), !pinned, false),
            (
                "tab.context.close-others",
                Some(TabAction::CloseOthers),
                true,
                false,
            ),
            (
                "tab.context.close-all",
                Some(TabAction::CloseAll),
                true,
                false,
            ),
            ("tab.context.split-right", None, false, true),
            ("tab.context.split-down", None, false, false),
            (
                if pinned {
                    "tab.context.unpin"
                } else {
                    "tab.context.pin"
                },
                Some(TabAction::Pin),
                true,
                true,
            ),
        ];
        let entries = items
            .into_iter()
            .map(|(key, action, enabled, separator)| {
                let spec = MenuItemSpec {
                    id: key.into(),
                    label: self.t(key).into(),
                    shortcut: None,
                    icon: None,
                    enabled,
                    selected: false,
                    indent: 0,
                    tree: None,
                    submenu: false,
                    separator_before: separator,
                };
                let panel_id = panel_id.clone();
                let tab = tab.clone();
                MenuEntry::new(spec, cx, |row| {
                    row.debug_selector(move || key.into())
                        .when_some(action.filter(|_| enabled), |row, action| {
                            row.on_click(cx.listener(move |this, _, _, cx| {
                                this.tab_context = None;
                                if this.tab_mutations_blocked {
                                    return;
                                }
                                match action {
                                    TabAction::Pin => {
                                        this.shell.layout.set_tab_pinned(&tab, !pinned);
                                    }
                                    _ => {
                                        let tabs = this
                                            .shell
                                            .layout
                                            .panel(&panel_id)
                                            .map(|panel| {
                                                panel
                                                    .tabs
                                                    .iter()
                                                    .filter(|candidate| {
                                                        this.shell.view_visible(candidate)
                                                            && !panel.pinned.contains(*candidate)
                                                            && match action {
                                                                TabAction::Close => {
                                                                    *candidate == &tab
                                                                }
                                                                TabAction::CloseOthers => {
                                                                    *candidate != &tab
                                                                }
                                                                TabAction::CloseAll => true,
                                                                TabAction::Pin => false,
                                                            }
                                                    })
                                                    .cloned()
                                                    .collect::<Vec<_>>()
                                            })
                                            .unwrap_or_default();
                                        if !tabs.is_empty() {
                                            cx.emit(UiCommand::CloseTabs(tabs));
                                        }
                                    }
                                }
                                cx.notify();
                            }))
                        })
                        .into_any_element()
                })
            })
            .collect();
        crate::components::floating_tree(anchor, point(px(0.), px(0.))).child(
            div()
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.tab_context = None;
                    cx.notify();
                }))
                .child(Menu::new("tab-context-menu", entries).debug_selector("tab-context-menu")),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext, Modifiers, TestAppContext, VisualTestContext};

    struct Empty;
    impl Render for Empty {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }
    fn draw(cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear();
        });
    }

    #[gpui::test]
    fn right_click_pin_protects_tab_from_close_all(cx: &mut TestAppContext) {
        let (shell, visual) = cx.add_window_view(|_, cx| {
            theme::init(&crate::extension_ui::ThemeId::new("lapis.dark"), None, cx);
            let editor = cx.new(|_| Empty);
            Shell::new(editor.into(), HashMap::new(), cx)
        });
        let pinned = PanelTab::tool(id::VIEW_FILES);
        let other = PanelTab::tool("test.other");
        let hidden = PanelTab::tool("test.hidden");
        let panel = shell.update(visual, |shell, _| {
            let panel = shell
                .shell
                .layout
                .open_tab(ZonePosition::Left, other.clone())
                .unwrap();
            shell
                .shell
                .layout
                .open_tab(ZonePosition::Left, hidden.clone())
                .unwrap();
            shell.shell.hidden_views.insert(hidden.instance_id());
            panel
        });
        assert_eq!(panel.as_str(), "panel-2");
        let closed = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let observed = closed.clone();
        let _subscription = visual.update(|_, cx| {
            cx.subscribe(&shell, move |_, event, _| {
                if let UiCommand::CloseTabs(tabs) = event {
                    *observed.borrow_mut() = tabs.clone();
                }
            })
        });
        draw(visual);
        let tab = visual.debug_bounds("panel-tab-panel-2-0").unwrap();
        visual.simulate_mouse_down(tab.center(), MouseButton::Right, Modifiers::none());
        visual.simulate_mouse_up(tab.center(), MouseButton::Right, Modifiers::none());
        draw(visual);
        let pin = visual.debug_bounds("tab.context.pin").unwrap();
        visual.simulate_click(pin.center(), Modifiers::none());
        shell.read_with(visual, |shell, _| {
            assert!(shell.shell.layout.tab_is_pinned(&pinned))
        });
        visual.update(|window, cx| {
            shell.update(cx, |shell, cx| {
                shell.close_tab_in_panel(
                    panel.clone(),
                    ZonePosition::Left,
                    pinned.clone(),
                    window,
                    cx,
                )
            })
        });
        shell.read_with(visual, |shell, _| {
            assert!(shell.shell.layout.tab_is_pinned(&pinned))
        });
        draw(visual);
        let tab = visual.debug_bounds("panel-tab-panel-2-0").unwrap();
        visual.simulate_mouse_down(tab.center(), MouseButton::Right, Modifiers::none());
        visual.simulate_mouse_up(tab.center(), MouseButton::Right, Modifiers::none());
        draw(visual);
        let close_all = visual.debug_bounds("tab.context.close-all").unwrap();
        visual.simulate_click(close_all.center(), Modifiers::none());
        assert_eq!(*closed.borrow(), vec![other]);
        draw(visual);
        let tab = visual.debug_bounds("panel-tab-panel-2-0").unwrap();
        visual.simulate_mouse_down(tab.center(), MouseButton::Right, Modifiers::none());
        visual.simulate_mouse_up(tab.center(), MouseButton::Right, Modifiers::none());
        draw(visual);
        let unpin = visual.debug_bounds("tab.context.unpin").unwrap();
        visual.simulate_click(unpin.center(), Modifiers::none());
        shell.read_with(visual, |shell, _| {
            assert!(!shell.shell.layout.tab_is_pinned(&pinned))
        });
        shell.update(visual, |shell, cx| {
            shell.set_tab_mutations_blocked(true, cx)
        });
        draw(visual);
        let tab = visual.debug_bounds("panel-tab-panel-2-0").unwrap();
        visual.simulate_mouse_down(tab.center(), MouseButton::Right, Modifiers::none());
        shell.read_with(visual, |shell, _| assert!(shell.tab_context.is_none()));
    }
}
