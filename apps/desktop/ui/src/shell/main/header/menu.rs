use crate::{
    components::{Menu, MenuEntry, MenuItemSpec, Popup, PopupPlacement, PopupScope},
    shell::HeaderMenuSection as MenuId,
};
use gpui::ElementId;

use super::*;
use menu_definition::{MenuItemDefinition, ROOT_MENUS, items};

impl Shell {
    pub(crate) fn render_header_menu(
        &self,
        scope: &PopupScope,
        max_height: Pixels,
        cx: &mut Context<Self>,
    ) -> Menu {
        let active = self.shell.header_menu_section;
        let entries = ROOT_MENUS
            .iter()
            .map(|(menu, label)| {
                let menu = *menu;
                let submenu = (active == Some(menu))
                    .then(|| self.render_header_submenu(menu, max_height, cx));
                MenuEntry::new(
                    self.header_menu_root_spec(menu, label, active == Some(menu)),
                    cx,
                    |row| {
                        let row =
                            row.debug_selector(move || format!("header-menu-root-{label}"))
                                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                                    if *hovered {
                                        this.select_header_menu(menu, cx);
                                    }
                                }))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.select_header_menu(menu, cx)
                                }));
                        Popup::new(row, PopupPlacement::Right, scope.clone())
                            .when_some(submenu, |popup, submenu| popup.content(submenu))
                            .into_any_element()
                    },
                )
            })
            .collect();
        Menu::new("header-menu-column", entries).debug_selector("header-menu-panel")
    }

    fn header_menu_root_spec(
        &self,
        menu: MenuId,
        label: &'static str,
        selected: bool,
    ) -> MenuItemSpec {
        let (key, icon) = match menu {
            MenuId::File => ("menu.file", IconName::Files),
            MenuId::Edit => ("menu.edit", IconName::Pencil),
            MenuId::View => ("menu.view", IconName::PanelLeft),
            MenuId::Window => ("menu.window", IconName::Square),
            MenuId::Help => ("menu.help", IconName::HelpCircle),
        };
        MenuItemSpec {
            id: format!("header-menu-root-{label}").into(),
            label: self.t(key).into(),
            shortcut: None,
            icon: Some(icon),
            enabled: true,
            selected,
            indent: 0,
            tree: None,
            submenu: true,
            separator_before: false,
        }
    }

    fn render_header_submenu(
        &self,
        menu: MenuId,
        max_height: Pixels,
        cx: &mut Context<Self>,
    ) -> Menu {
        let menu_prefix = match menu {
            MenuId::File => "menu.file",
            MenuId::Edit => "menu.edit",
            MenuId::View => "menu.view",
            MenuId::Window => "menu.window",
            MenuId::Help => "menu.help",
        };
        let mut entries = items(menu)
            .iter()
            .map(|item| self.header_item_entry(menu_prefix, item, cx))
            .collect::<Vec<_>>();
        if menu == MenuId::View {
            entries.extend(self.panel_visibility_entries().into_iter().enumerate().map(
                |(index, (panel_id, label, _))| {
                    MenuEntry::new(
                        MenuItemSpec {
                            id: format!("panel_id-visibility-{}", panel_id.as_str()).into(),
                            label: label.into(),
                            shortcut: None,
                            icon: None,
                            enabled: true,
                            selected: false,
                            indent: 0,
                            tree: None,
                            submenu: false,
                            separator_before: index == 0,
                        },
                        cx,
                        |row| {
                            row.on_click(cx.listener(move |this, _, window, cx| {
                                this.toggle_panel_visibility(panel_id.clone(), cx);
                                this.close_header_menu(cx);
                                window.focus(&this.focus_handle);
                            }))
                            .into_any_element()
                        },
                    )
                },
            ));
        }
        Menu::new(
            ElementId::Name(format!("header-submenu-{menu:?}").into()),
            entries,
        )
        .debug_selector(format!("header-submenu-{menu:?}"))
        .max_height(max_height)
    }

    fn header_item_entry(
        &self,
        menu_prefix: &'static str,
        item: &'static MenuItemDefinition,
        cx: &mut Context<Self>,
    ) -> MenuEntry {
        let action = item.action;
        let id = item.id;
        let enabled = item.enabled;
        let key = format!("{menu_prefix}.{id}");
        let resolved = self.t(&key);
        let label = if resolved.is_empty() || resolved == key {
            item.label.to_owned()
        } else {
            resolved
        };
        MenuEntry::new(
            MenuItemSpec {
                id: format!("header-menu-item-{id}").into(),
                label: label.into(),
                shortcut: item.shortcut.map(Into::into),
                icon: item.icon,
                enabled,
                selected: false,
                indent: 0,
                tree: None,
                submenu: false,
                separator_before: item.separator_before,
            },
            cx,
            |row| {
                row.debug_selector(move || format!("header-menu-item-{id}"))
                    .when(enabled, |row| {
                        row.on_click(cx.listener(move |this, _, window, cx| {
                            if let Some(action) = action {
                                this.execute_header_menu_action(action, window, cx);
                            }
                        }))
                    })
                    .into_any_element()
            },
        )
    }
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::*;
    use gpui::{AppContext, Modifiers, TestAppContext, VisualTestContext};

    struct Empty;
    impl Render for Empty {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    fn setup(cx: &mut TestAppContext) -> (gpui::Entity<Shell>, &mut VisualTestContext) {
        cx.add_window_view(|_, cx| {
            theme::init(&crate::extension_ui::ThemeId::new("lapis.dark"), None, cx);
            let document = cx.new(|_| Empty);
            Shell::new(document.into(), HashMap::new(), cx)
        })
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear();
        });
    }

    #[gpui::test]
    fn header_popup_is_button_anchored_and_same_trigger_toggles_closed(cx: &mut TestAppContext) {
        let (shell, visual) = setup(cx);
        draw(visual);
        let trigger = visual.debug_bounds("header-menu-trigger").unwrap();
        for click in [
            trigger.origin + point(px(3.), px(3.)),
            trigger.bottom_right() - point(px(3.), px(3.)),
        ] {
            visual.simulate_click(click, Modifiers::none());
            draw(visual);
            let menu = visual.debug_bounds("header-menu-panel").unwrap();
            assert_eq!(menu.left(), trigger.left());
            assert_eq!(menu.top(), trigger.bottom() + px(4.));
            visual.simulate_click(click, Modifiers::none());
            shell.read_with(visual, |shell, _| assert!(!shell.shell.header_menu_open));
            draw(visual);
            // debug_boundsは過去のselectorを保持するため、閉じた後の操作で状態を確認する。
            visual.simulate_click(point(px(600.), px(500.)), Modifiers::none());
            shell.read_with(visual, |shell, _| assert!(!shell.shell.header_menu_open));
        }
    }

    #[gpui::test]
    fn submenu_follows_its_row_and_blank_space_outside_the_cascade_dismisses(
        cx: &mut TestAppContext,
    ) {
        let (shell, visual) = setup(cx);
        draw(visual);
        let trigger = visual.debug_bounds("header-menu-trigger").unwrap();
        visual.simulate_click(trigger.center(), Modifiers::none());
        draw(visual);
        let row = visual.debug_bounds("header-menu-root-Window").unwrap();
        visual.simulate_mouse_move(row.center(), None, Modifiers::none());
        draw(visual);
        let submenu = visual.debug_bounds("header-submenu-Window").unwrap();
        assert_eq!(submenu.top(), row.top() - px(4.));
        assert_eq!(submenu.left(), row.right() + px(4.));
        let outside = point(submenu.center().x, row.top() - px(16.));
        visual.simulate_click(outside, Modifiers::none());
        shell.read_with(visual, |shell, _| assert!(!shell.shell.header_menu_open));
    }

    #[gpui::test]
    fn submenu_click_reaches_the_action_and_file_menu_is_not_truncated(cx: &mut TestAppContext) {
        let (shell, visual) = setup(cx);
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        let observed = calls.clone();
        let _subscription = visual.update(|_, cx| {
            cx.subscribe(&shell, move |_, event, _| {
                if matches!(event, UiCommand::CloseWindow) {
                    observed.set(observed.get() + 1);
                }
            })
        });
        draw(visual);
        let trigger = visual.debug_bounds("header-menu-trigger").unwrap();
        visual.simulate_click(trigger.center(), Modifiers::none());
        draw(visual);
        let row = visual.debug_bounds("header-menu-root-File").unwrap();
        visual.simulate_mouse_move(row.center(), None, Modifiers::none());
        draw(visual);
        let submenu = visual.debug_bounds("header-submenu-File").unwrap();
        let close = visual
            .debug_bounds("header-menu-item-close-window")
            .unwrap();
        assert!(close.bottom() <= submenu.bottom());
        let save_as = visual.debug_bounds("header-menu-item-save-as").unwrap();
        let new_file = visual.debug_bounds("header-menu-item-new-file").unwrap();
        assert!(new_file.top() > save_as.bottom());
        visual.simulate_click(close.center(), Modifiers::none());
        assert_eq!(calls.get(), 1);
        shell.read_with(visual, |shell, _| assert!(!shell.shell.header_menu_open));
    }

    #[gpui::test]
    fn settings_uses_the_same_trigger_and_outside_dismissal(cx: &mut TestAppContext) {
        let (shell, visual) = setup(cx);
        draw(visual);
        let trigger = visual.debug_bounds("settings-menu-trigger").unwrap();
        visual.simulate_click(trigger.center(), Modifiers::none());
        draw(visual);
        let menu = visual.debug_bounds("settings-menu-panel").unwrap();
        let viewport = visual.update(|window, _| window.viewport_size());
        // ボタン左端を起点とし、右にはみ出す分だけWindow端の内側へ寄せる。
        assert_eq!(
            menu.left(),
            trigger
                .left()
                .min(viewport.width - menu.size.width - px(8.))
        );
        assert!(menu.right() <= viewport.width - px(8.));
        assert!(menu.left() >= px(8.));
        assert_eq!(menu.top(), trigger.bottom() + tokens::spacing::XS);
        assert_eq!(visual.debug_bounds("dock-frame").unwrap().top(), menu.top());
        visual.simulate_click(trigger.center(), Modifiers::none());
        shell.read_with(visual, |shell, _| assert!(!shell.shell.settings_menu_open));
        visual.simulate_click(trigger.center(), Modifiers::none());
        draw(visual);
        visual.simulate_click(point(px(600.), px(500.)), Modifiers::none());
        shell.read_with(visual, |shell, _| assert!(!shell.shell.settings_menu_open));
    }

    #[gpui::test]
    fn escape_dismisses_and_switching_triggers_closes_the_other_popup(cx: &mut TestAppContext) {
        let (shell, visual) = setup(cx);
        draw(visual);
        let menu = visual.debug_bounds("header-menu-trigger").unwrap();
        let settings = visual.debug_bounds("settings-menu-trigger").unwrap();
        visual.simulate_click(menu.center(), Modifiers::none());
        visual.simulate_click(settings.center(), Modifiers::none());
        shell.read_with(visual, |shell, _| {
            assert!(!shell.shell.header_menu_open);
            assert!(shell.shell.settings_menu_open);
        });
        visual.simulate_click(menu.center(), Modifiers::none());
        shell.read_with(visual, |shell, _| {
            assert!(shell.shell.header_menu_open);
            assert!(!shell.shell.settings_menu_open);
        });
        visual.simulate_keystrokes("escape");
        shell.read_with(visual, |shell, _| assert!(!shell.shell.header_menu_open));
    }
    #[gpui::test]
    fn workspace_menu_opens_recent_and_dismisses(cx: &mut TestAppContext) {
        let (shell, visual) = setup(cx);
        let recent = std::path::PathBuf::from("example-project");
        shell.update(visual, |shell, _| {
            shell.session.recent_workspaces = vec![recent.clone(), recent.clone()];
        });
        let opened = std::rc::Rc::new(std::cell::RefCell::new(None));
        let observed = opened.clone();
        let _subscription = visual.update(|_, cx| {
            cx.subscribe(&shell, move |_, command, _| {
                if let UiCommand::OpenRecent(path) = command {
                    *observed.borrow_mut() = Some(path.clone());
                }
            })
        });
        draw(visual);
        let trigger = visual.debug_bounds("workspace-menu-trigger").unwrap();
        visual.simulate_click(trigger.center(), Modifiers::none());
        draw(visual);
        let menu = visual.debug_bounds("workspace-menu-panel").unwrap();
        assert_eq!(menu.top(), trigger.bottom() + tokens::spacing::XS);
        visual.simulate_click(trigger.center(), Modifiers::none());
        shell.read_with(visual, |shell, _| assert!(!shell.shell.workspace_menu_open));
        visual.simulate_click(trigger.center(), Modifiers::none());
        draw(visual);
        let row = visual.debug_bounds("workspace-menu-recent-0").unwrap();
        visual.simulate_click(row.center(), Modifiers::none());
        assert_eq!(*opened.borrow(), Some(recent));
        shell.read_with(visual, |shell, _| assert!(!shell.shell.workspace_menu_open));
        visual.simulate_click(trigger.center(), Modifiers::none());
        draw(visual);
        visual.simulate_click(point(px(600.), px(500.)), Modifiers::none());
        shell.read_with(visual, |shell, _| assert!(!shell.shell.workspace_menu_open));
    }
    #[gpui::test]
    fn settings_menu_lists_settings_keymap_and_plugins(cx: &mut TestAppContext) {
        let (_, visual) = setup(cx);
        draw(visual);
        let trigger = visual.debug_bounds("settings-menu-trigger").unwrap();
        visual.simulate_click(trigger.center(), Modifiers::none());
        draw(visual);
        let settings = visual.debug_bounds("settings-menu-settings").unwrap();
        let keymap = visual.debug_bounds("settings-menu-keymap").unwrap();
        let plugins = visual.debug_bounds("settings-menu-plugins").unwrap();
        assert!(settings.top() < keymap.top());
        assert!(keymap.top() < plugins.top());
        assert!(visual.debug_bounds("settings-menu-theme").is_none());
    }
}
