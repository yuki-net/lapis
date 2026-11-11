use super::*;
use crate::components::{
    Menu, MenuEntry, MenuItemSpec, Popup, PopupPlacement, PopupScope, header_button,
};

impl Shell {
    pub(super) fn render_header_center(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let trigger = header_button(
            "header-workspace",
            desktop_client::short_workspace_label(
                Some(self.session.workspace_name()),
                self.session.workspace_id.as_deref(),
            ),
            cx,
        )
        .debug_selector(|| "workspace-menu-trigger".into())
        .when(self.shell.workspace_menu_open, |button| {
            button.bg(theme::colors(cx).button_background_selected)
        })
        .on_click(cx.listener(|this, _, window, cx| {
            this.toggle_header_popup(super::HeaderPopup::Workspace, cx);
            if this.shell.workspace_menu_open {
                crate::gpui_compat::focus(window, &this.focus_handle, cx);
            }
        }));
        let popup = Popup::new(trigger, PopupPlacement::BelowStart, PopupScope::default())
            .on_dismiss(cx.listener(|this, _: &(), _, cx| this.close_workspace_menu(cx)))
            .when(self.shell.workspace_menu_open, |popup| {
                popup.content(self.render_workspace_menu(window, cx))
            });
        div()
            .w(px(0.0))
            .min_w(px(0.0))
            .px(tokens::spacing::GAP)
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .gap(tokens::spacing::XS)
            .text_size(tokens::typography::FONT_SM)
            .child(popup)
            .child(Icon::new(IconName::ChevronRight).size(px(12.)))
            .when_some(self.branch_picker.clone(), |header, picker| {
                header.child(picker)
            })
    }

    fn close_workspace_menu(&mut self, cx: &mut Context<Self>) {
        if self.shell.workspace_menu_open {
            self.shell.workspace_menu_open = false;
            cx.notify();
        }
    }

    pub(crate) fn workspace_menu_key_down(
        &mut self,
        event: &KeyDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.shell.workspace_menu_open && event.keystroke.key == "escape" {
            self.close_workspace_menu(cx);
        }
    }

    fn render_workspace_menu(&self, window: &Window, cx: &mut Context<Self>) -> Menu {
        let spec = |id: String, label: String, icon, enabled, separator_before| MenuItemSpec {
            id: id.into(),
            label: label.into(),
            shortcut: None,
            icon,
            enabled,
            selected: false,
            indent: 0,
            tree: None,
            submenu: false,
            separator_before,
        };
        let open = spec(
            "workspace-open".into(),
            "開く…".into(),
            Some(IconName::Files),
            true,
            false,
        );
        let mut entries = Vec::new();
        if self.session.connection.is_some() || self.session.workspace_id.is_some() {
            let connection = self.session.connection.as_ref();
            let label = desktop_client::connection_label(
                connection.and_then(|connection| connection.host_name.as_deref()),
                connection.and_then(|connection| connection.host_id.as_deref()),
                Some(self.session.workspace_name()),
                self.session.workspace_id.as_deref(),
            );
            let identity = spec(
                "workspace-connection-info".into(),
                label,
                Some(IconName::Plug),
                false,
                false,
            );
            entries.push(MenuEntry::new(identity, cx, |row| {
                row.debug_selector(|| "workspace-menu-connection".into())
                    .into_any_element()
            }));
        }
        if let Some(connection) = self
            .session
            .connection
            .as_ref()
            .filter(|connection| connection.can_switch)
        {
            let heading = spec(
                "workspace-switch-title".into(),
                "Workspace を切り替え".into(),
                Some(IconName::Plug),
                false,
                true,
            );
            entries.push(MenuEntry::new(heading, cx, |row| row.into_any_element()));
            for (index, workspace) in connection.workspaces.iter().enumerate() {
                let current = self.session.workspace_id.as_deref() == Some(workspace.id.as_str());
                let mut item = spec(
                    format!("workspace-switch-{index}"),
                    workspace.name.clone(),
                    Some(IconName::Plug),
                    !current,
                    false,
                );
                item.selected = current;
                let id = workspace.id.clone();
                entries.push(MenuEntry::new(item, cx, |row| {
                    let id = id.clone();
                    row.debug_selector(move || format!("workspace-menu-switch-{index}"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.close_workspace_menu(cx);
                            cx.emit(UiCommand::SwitchWorkspace(id.clone()));
                        }))
                        .into_any_element()
                }));
            }
        }
        entries.push(MenuEntry::new(open, cx, |row| {
            row.debug_selector(|| "workspace-menu-open".into())
                .on_click(cx.listener(|this, _, window, cx| {
                    this.close_workspace_menu(cx);
                    this.open_project(window, cx);
                }))
                .into_any_element()
        }));
        let clone = spec(
            "workspace-clone".into(),
            "Git からクローン…".into(),
            Some(IconName::Git),
            false,
            false,
        );
        entries.push(MenuEntry::new(clone, cx, |row| row.into_any_element()));
        let host = spec(
            "workspace-host".into(),
            "Host に接続…".into(),
            Some(IconName::Plug),
            true,
            false,
        );
        entries.push(MenuEntry::new(host, cx, |row| {
            row.debug_selector(|| "workspace-menu-host".into())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.close_workspace_menu(cx);
                    cx.emit(UiCommand::ConnectHost);
                }))
                .into_any_element()
        }));
        let rename = spec(
            "workspace-rename".into(),
            "ワークスペース名を変更…".into(),
            Some(IconName::Pencil),
            false,
            true,
        );
        entries.push(MenuEntry::new(rename, cx, |row| row.into_any_element()));
        let heading = spec(
            "workspace-recent-title".into(),
            "最近のワークスペース".into(),
            Some(IconName::History),
            false,
            true,
        );
        entries.push(MenuEntry::new(heading, cx, |row| row.into_any_element()));
        let mut recent = Vec::new();
        if let Some(root) = &self.session.root {
            recent.push(root.clone());
        }
        for root in &self.session.recent_workspaces {
            if !recent.contains(root) {
                recent.push(root.clone());
            }
        }
        recent.truncate(8);
        if recent.is_empty() {
            let empty = spec(
                "workspace-recent-empty".into(),
                "履歴はありません".into(),
                None,
                false,
                false,
            );
            entries.push(MenuEntry::new(empty, cx, |row| row.into_any_element()));
        }
        for (index, root) in recent.into_iter().enumerate() {
            let name = root
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| root.display().to_string());
            let label = format!("{}  {}", name, root.display());
            let mut item = spec(
                format!("workspace-recent-{index}"),
                label,
                Some(IconName::Files),
                true,
                false,
            );
            item.selected = self.session.root.as_ref() == Some(&root);
            entries.push(MenuEntry::new(item, cx, |row| {
                row.debug_selector(move || format!("workspace-menu-recent-{index}"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.close_workspace_menu(cx);
                        this.open_recent_workspace(root.clone(), window, cx);
                    }))
                    .into_any_element()
            }));
        }
        Menu::new("workspace-menu", entries)
            .debug_selector("workspace-menu-panel")
            .max_height((window.viewport_size().height - px(16.)).max(px(0.)))
    }
}
