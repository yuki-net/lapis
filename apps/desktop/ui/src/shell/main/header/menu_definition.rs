use crate::shell::HeaderMenuSection as MenuId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MenuAction {
    NewFile,
    NewFolder,
    NewWindow,
    OpenProject,
    OpenFile,
    CloseProject,
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
    pub(crate) separator_before: bool,
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
            separator_before: false,
        }
    }

    const fn disabled(id: &'static str, label: &'static str) -> Self {
        Self {
            id,
            label,
            shortcut: None,
            action: Some(MenuAction::Placeholder),
            enabled: false,
            separator_before: false,
        }
    }

    const fn with_separator_before(mut self) -> Self {
        self.separator_before = true;
        self
    }
}

pub(crate) const ROOT_MENUS: &[(MenuId, &str)] = &[
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
    MenuItemDefinition::action("new-folder", "New Folder...", None, MenuAction::NewFolder),
    MenuItemDefinition::action(
        "new-window",
        "New Window",
        Some("Ctrl+Shift+N"),
        MenuAction::NewWindow,
    ),
    MenuItemDefinition::action(
        "open-project",
        "Open Project...",
        Some("Ctrl+O"),
        MenuAction::OpenProject,
    )
    .with_separator_before(),
    MenuItemDefinition::action("open-file", "Open...", None, MenuAction::OpenFile),
    MenuItemDefinition::disabled("open-recent", "Open Recent"),
    MenuItemDefinition::action(
        "close-project",
        "Close Project",
        None,
        MenuAction::CloseProject,
    )
    .with_separator_before(),
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
    MenuItemDefinition::action(
        "new-window",
        "New Window",
        Some("Ctrl+Shift+N"),
        MenuAction::NewWindow,
    ),
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
                .any(|item| { item.action == Some(MenuAction::NewFolder) && item.enabled })
        );
        assert!(
            FILE_MENU
                .iter()
                .any(|item| { item.action == Some(MenuAction::NewWindow) && item.enabled })
        );
        assert!(
            FILE_MENU
                .iter()
                .any(|item| { item.action == Some(MenuAction::OpenProject) && item.enabled })
        );
        assert!(
            FILE_MENU
                .iter()
                .any(|item| { item.action == Some(MenuAction::CloseProject) && item.enabled })
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
                .any(|item| item.id == "switch-window" && !item.enabled)
        );
    }
}
