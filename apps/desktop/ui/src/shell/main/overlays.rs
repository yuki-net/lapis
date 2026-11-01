use super::*;
use crate::components::{Menu, MenuEntry, MenuItemSpec};

impl Shell {
    pub(crate) fn render_settings_menu(&self, cx: &mut Context<Self>) -> Menu {
        let items = [
            (
                "settings-menu-settings",
                "settings.title",
                IconName::Settings,
                true,
            ),
            (
                "settings-menu-keymap",
                "settings.menu.keymap",
                IconName::Keyboard,
                false,
            ),
            (
                "settings-menu-plugins",
                "settings.menu.plugins",
                IconName::Plug,
                false,
            ),
        ];
        let entries = items
            .into_iter()
            .map(|(id, key, icon, enabled)| {
                let spec = MenuItemSpec {
                    id: id.into(),
                    label: self.t(key).into(),
                    shortcut: None,
                    icon: Some(icon),
                    enabled,
                    selected: false,
                    indent: 0,
                    tree: None,
                    submenu: false,
                    separator_before: false,
                };
                MenuEntry::new(spec, cx, |row| {
                    row.debug_selector(move || id.into())
                        .when(enabled, |row| {
                            row.on_click(cx.listener(|this, _, _, cx| this.open_settings_view(cx)))
                        })
                        .into_any_element()
                })
            })
            .collect();
        Menu::new("settings-menu", entries)
            .debug_selector("settings-menu-panel")
            .padding(tokens::spacing::SM)
    }
}
