use super::*;
use crate::components::{Menu, MenuEntry, MenuItemSpec};

impl Shell {
    pub(crate) fn render_settings_menu(&self, cx: &mut Context<Self>) -> Menu {
        let specs = [
            MenuItemSpec {
                id: "settings-menu-settings".into(),
                label: "Settings".into(),
                shortcut: Some("Ctrl+,".into()),
                icon: Some(IconName::Settings),
                enabled: true,
                selected: false,
                submenu: false,
                separator_before: false,
            },
            MenuItemSpec {
                id: "settings-menu-theme".into(),
                label: "Theme".into(),
                shortcut: theme::name(&theme::active_id(cx), cx).map(Into::into),
                icon: Some(IconName::SunMoon),
                enabled: true,
                selected: false,
                submenu: false,
                separator_before: false,
            },
        ];
        let mut entries = specs
            .into_iter()
            .map(|spec| {
                let settings = spec.id.as_ref() == "settings-menu-settings";
                MenuEntry::new(spec, cx, |row| {
                    row.on_click(cx.listener(move |this, _, _, cx| {
                        if settings {
                            this.open_settings_view(cx);
                        } else {
                            this.toggle_theme_preference(cx);
                        }
                    }))
                    .into_any_element()
                })
            })
            .collect::<Vec<_>>();
        if self.shell.theme_picker_open {
            let active = theme::active_id(cx);
            entries.extend(theme::available(cx).into_iter().enumerate().map(
                |(index, (theme_id, name))| {
                    let selected = active == theme_id;
                    MenuEntry::new(
                        MenuItemSpec {
                            id: theme_id.as_str().to_owned().into(),
                            label: name.into(),
                            shortcut: None,
                            icon: None,
                            enabled: true,
                            selected,
                            submenu: false,
                            separator_before: index == 0,
                        },
                        cx,
                        |row| {
                            row.on_click(cx.listener(move |this, _, _, cx| {
                                this.select_theme(theme_id.clone(), cx)
                            }))
                            .into_any_element()
                        },
                    )
                },
            ));
        }
        Menu::new("settings-menu", entries)
            .debug_selector("settings-menu-panel")
            .padding(tokens::spacing::SM)
    }
}
