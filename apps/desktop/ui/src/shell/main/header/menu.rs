use crate::shell::HeaderMenuSection as MenuId;
use gpui::ElementId;

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MenuAction {
    NewFile,
    OpenProject,
    OpenFile,
    CloseWindow,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    ToggleLeftPanel,
    ToggleBottomPanel,
    ToggleRightPanel,
    Placeholder,
}

#[derive(Clone, Copy)]
pub(crate) struct MenuItemDefinition {
    pub(crate) id: &'static str,
    pub(crate) label: &'static str,
    pub(crate) shortcut: Option<&'static str>,
    pub(crate) action: Option<MenuAction>,
    pub(crate) enabled: bool,
}

impl MenuItemDefinition {
    const fn action(
        id: &'static str,
        label: &'static str,
        shortcut: Option<&'static str>,
        action: MenuAction,
    ) -> Self {
        Self {
            id,
            label,
            shortcut,
            action: Some(action),
            enabled: !matches!(action, MenuAction::Placeholder),
        }
    }

    const fn disabled(id: &'static str, label: &'static str) -> Self {
        Self {
            id,
            label,
            shortcut: None,
            action: Some(MenuAction::Placeholder),
            enabled: false,
        }
    }
}

const ROOT_MENUS: &[(MenuId, &str)] = &[
    (MenuId::File, "File"),
    (MenuId::Edit, "Edit"),
    (MenuId::View, "View"),
    (MenuId::Window, "Window"),
    (MenuId::Help, "Help"),
];

const FILE_MENU: &[MenuItemDefinition] = &[
    MenuItemDefinition::action(
        "new-file",
        "New File...",
        Some("Ctrl+N"),
        MenuAction::NewFile,
    ),
    MenuItemDefinition::action(
        "open-project",
        "Open Project...",
        Some("Ctrl+O"),
        MenuAction::OpenProject,
    ),
    MenuItemDefinition::action("open-file", "Open...", None, MenuAction::OpenFile),
    MenuItemDefinition::disabled("open-recent", "Open Recent"),
    MenuItemDefinition::action(
        "close-window",
        "Close Window",
        Some("Ctrl+Shift+F4"),
        MenuAction::CloseWindow,
    ),
];

const EDIT_MENU: &[MenuItemDefinition] = &[
    MenuItemDefinition::action("undo", "Undo", Some("Ctrl+Z"), MenuAction::Undo),
    MenuItemDefinition::action("redo", "Redo", Some("Ctrl+Y"), MenuAction::Redo),
    MenuItemDefinition::action("cut", "Cut", Some("Ctrl+X"), MenuAction::Cut),
    MenuItemDefinition::action("copy", "Copy", Some("Ctrl+C"), MenuAction::Copy),
    MenuItemDefinition::action("paste", "Paste", Some("Ctrl+V"), MenuAction::Paste),
];

const VIEW_MENU: &[MenuItemDefinition] = &[
    MenuItemDefinition::action(
        "left-panel",
        "Left Panel",
        None,
        MenuAction::ToggleLeftPanel,
    ),
    MenuItemDefinition::action(
        "bottom-panel",
        "Bottom Panel",
        Some("Ctrl+J"),
        MenuAction::ToggleBottomPanel,
    ),
    MenuItemDefinition::action(
        "right-panel",
        "Right Panel",
        None,
        MenuAction::ToggleRightPanel,
    ),
    MenuItemDefinition::disabled("command-palette", "Command Palette"),
];

const WINDOW_MENU: &[MenuItemDefinition] = &[
    MenuItemDefinition::disabled("new-window", "New Window"),
    MenuItemDefinition::action(
        "close-window",
        "Close Window",
        Some("Ctrl+Shift+F4"),
        MenuAction::CloseWindow,
    ),
    MenuItemDefinition::disabled("switch-window", "Switch Window"),
];

const HELP_MENU: &[MenuItemDefinition] = &[
    MenuItemDefinition::disabled("keyboard-shortcuts", "Keyboard Shortcuts"),
    MenuItemDefinition::disabled("documentation", "Documentation"),
    MenuItemDefinition::disabled("about", "About Lapis"),
];

pub(crate) fn items(menu: MenuId) -> &'static [MenuItemDefinition] {
    match menu {
        MenuId::File => FILE_MENU,
        MenuId::Edit => EDIT_MENU,
        MenuId::View => VIEW_MENU,
        MenuId::Window => WINDOW_MENU,
        MenuId::Help => HELP_MENU,
    }
}

impl Editor {
    pub(crate) fn render_header_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.shell.header_menu_section;
        anchored()
            .position(self.shell.header_menu_anchor)
            .offset(point(px(-8.0), px(8.0)))
            .snap_to_window_with_margin(px(8.0))
            .child(
                div()
                    .id("header-menu-surface")
                    .relative()
                    .w(px(440.0))
                    .h(px(164.0))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.close_header_menu(cx);
                    }))
                    .child(
                        div()
                            .w(px(190.0))
                            .h(px(164.0))
                            .p_1()
                            .rounded(px(7.0))
                            .border_1()
                            .border_color(theme::border())
                            .bg(theme::surface())
                            .shadow_lg()
                            .text_color(theme::text())
                            .children(ROOT_MENUS.iter().map(|(menu, label)| {
                                self.render_header_menu_root(
                                    *menu,
                                    label,
                                    active == Some(*menu),
                                    cx,
                                )
                            })),
                    )
                    .when_some(active, |surface, menu| {
                        surface.child(self.render_header_submenu(menu, cx))
                    }),
            )
    }

    fn render_header_menu_root(
        &self,
        menu: MenuId,
        label: &'static str,
        active: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(ElementId::Name(format!("header-menu-root-{label}").into()))
            .h(px(30.0))
            .w_full()
            .px_2()
            .rounded(px(4.0))
            .flex()
            .items_center()
            .justify_between()
            .text_size(px(13.0))
            .when(active, |item| item.bg(theme::accent_soft()))
            .hover(|style| style.bg(theme::accent_soft()))
            .child(label)
            .child(div().text_color(theme::muted()).child("›"))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.select_header_menu(menu, cx);
                }
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.select_header_menu(menu, cx);
            }))
    }

    fn render_header_submenu(
        &self,
        menu: MenuId,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(ElementId::Name(format!("header-submenu-{menu:?}").into()))
            .absolute()
            .left(px(184.0))
            .top(px(0.0))
            .w(px(250.0))
            .p_1()
            .rounded(px(7.0))
            .border_1()
            .border_color(theme::border())
            .bg(theme::surface())
            .shadow_lg()
            .children(
                items(menu)
                    .iter()
                    .map(|item| self.render_header_item(item, cx)),
            )
    }

    fn render_header_item(
        &self,
        item: &'static MenuItemDefinition,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let action = item.action;
        let id = item.id;
        let enabled = item.enabled;
        div()
            .id(ElementId::Name(format!("header-menu-item-{id}").into()))
            .h(px(32.0))
            .w_full()
            .px_2()
            .rounded(px(4.0))
            .flex()
            .items_center()
            .justify_between()
            .text_size(px(13.0))
            .text_color(if enabled {
                theme::text()
            } else {
                theme::subtle()
            })
            .when(enabled, |item| {
                item.hover(|style| style.bg(theme::surface_hover()))
            })
            .child(item.label)
            .when_some(item.shortcut, |row, shortcut| {
                row.child(div().text_color(theme::muted()).child(shortcut))
            })
            .when(enabled, |row| {
                row.on_click(cx.listener(move |this, _, window, cx| {
                    if let Some(action) = action {
                        this.execute_header_menu_action(action, window, cx);
                    }
                }))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_menu_has_five_root_sections() {
        assert_eq!(ROOT_MENUS.len(), 5);
        assert_eq!(ROOT_MENUS[0].0, MenuId::File);
        assert_eq!(ROOT_MENUS[1].0, MenuId::Edit);
        assert_eq!(ROOT_MENUS[2].0, MenuId::View);
        assert_eq!(ROOT_MENUS[3].0, MenuId::Window);
        assert_eq!(ROOT_MENUS[4].0, MenuId::Help);
    }

    #[test]
    fn file_menu_exposes_workspace_and_window_actions() {
        assert!(
            FILE_MENU
                .iter()
                .any(|item| { item.action == Some(MenuAction::NewFile) && item.enabled })
        );
        assert!(
            FILE_MENU
                .iter()
                .any(|item| { item.action == Some(MenuAction::OpenProject) && item.enabled })
        );
        assert!(
            FILE_MENU
                .iter()
                .any(|item| { item.action == Some(MenuAction::CloseWindow) && item.enabled })
        );
    }

    #[test]
    fn placeholder_items_are_disabled() {
        assert!(HELP_MENU.iter().all(|item| !item.enabled));
        assert!(
            WINDOW_MENU
                .iter()
                .any(|item| item.id == "new-window" && !item.enabled)
        );
    }
}
