use super::*;

impl Shell {
    pub(super) fn render_empty_panel(
        &self,
        position: PanelPosition,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        if position == PanelPosition::Main {
            return self.render_main_empty_panel(cx);
        }
        let title = match position {
            PanelPosition::Left => self.t("panel.left"),
            PanelPosition::Bottom => self.t("panel.bottom"),
            PanelPosition::Right => self.t("panel.right"),
            PanelPosition::Main => self.t("panel.main"),
        };
        panel_empty_state_element(
            super::header::controls::open_panel_icon(position),
            title,
            self.t("panel.empty-desc"),
        )
        .child(
            div()
                .id(("open-tool", panel_key(position)))
                .mt_2()
                .px_2()
                .py_1()
                .rounded(px(5.0))
                .border_1()
                .border_color(theme::colors().border_default)
                .text_size(px(11.0))
                .hover(|style| style.bg(theme::colors().button_background_hover))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.open_tool_picker(position, cx);
                }))
                .child(self.t("panel.open-tool")),
        )
    }

    pub(super) fn render_document_content(
        &self,
        document: &lapis_editor_core::DocumentId,
        _: &mut Context<Self>,
    ) -> gpui::Div {
        if self.session.active_document_id() == Some(document) {
            div().size_full().child(self.document_view.clone())
        } else {
            panel_empty_state("F", "Document", "Select the document tab to edit")
        }
    }
    pub(super) fn render_tool_content(&self, view: &ViewId, _: &mut Context<Self>) -> gpui::Div {
        match self.views.get(view) {
            Some(view) => div().flex().flex_1().min_h(px(0.0)).child(view.clone()),
            None => panel_empty_state("?", "Unknown view", view.as_str().to_owned()),
        }
    }
    pub(super) fn render_main_empty_panel(&self, cx: &mut Context<Self>) -> gpui::Div {
        if self.detached {
            return div()
                .size_full()
                .child(panel_empty_state("", "Lapis", "タブはありません"));
        }
        // TODO: Workspaceを開いている状態と、作業中の状態で開始画面を分ける。
        // 現在はどちらも同じWorkspace一覧を表示する。
        let current_workspace = if self.session.workspace_root().is_some() {
            self.session.workspace_root().map(ToOwned::to_owned)
        } else {
            None
        };
        let mut recent_workspaces = Vec::<std::path::PathBuf>::new();
        if let Some(root) = current_workspace.clone() {
            recent_workspaces.push(root);
        }
        for root in &self.session.recent_workspaces {
            if !recent_workspaces.contains(root) {
                recent_workspaces.push(root.clone());
            }
        }
        recent_workspaces.truncate(8);
        let has_recent_workspaces = !recent_workspaces.is_empty();

        div().flex_1().w_full().h_full().child(
            div()
                .id("main-empty-panel")
                .flex_1()
                .w_full()
                .h_full()
                .flex()
                .flex_col()
                .p_8()
                .gap_5()
                .child(
                    div()
                        .w(px(760.0))
                        .max_w_full()
                        .text_size(px(24.0))
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(theme::colors().text_primary)
                        .child(self.t("workspace.title")),
                )
                .child(
                    div()
                        .w(px(760.0))
                        .max_w_full()
                        .flex()
                        .gap_3()
                        .child(
                            workspace_action("workspace-open", "▱", self.t("workspace.open"))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_project(window, cx);
                                })),
                        )
                        .child(
                            workspace_action("workspace-clone", "⑂", self.t("workspace.clone"))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.clone_from_git(window, cx);
                                })),
                        ),
                )
                .child(
                    div()
                        .w(px(760.0))
                        .max_w_full()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .children(recent_workspaces.into_iter().enumerate().map(
                            |(index, root)| {
                                let selected = current_workspace.as_ref() == Some(&root);
                                let open_root = root.clone();
                                let name = root
                                    .file_name()
                                    .and_then(|name| name.to_str())
                                    .unwrap_or("Workspace")
                                    .to_owned();
                                div()
                                    .id(("recent-workspace", index))
                                    .w_full()
                                    .p_3()
                                    .rounded(px(6.0))
                                    .bg(if selected {
                                        theme::colors().button_background_selected
                                    } else {
                                        theme::colors().background_tertiary
                                    })
                                    .hover(|style| {
                                        style.bg(theme::colors().button_background_hover)
                                    })
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .text_color(theme::colors().text_primary)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_recent_workspace(open_root.clone(), window, cx);
                                    }))
                                    .child(
                                        div()
                                            .text_size(px(14.0))
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .child(name),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(theme::colors().text_secondary)
                                            .child(root.display().to_string()),
                                    )
                            },
                        )),
                )
                .when(!has_recent_workspaces, |content| {
                    content.child(
                        div()
                            .text_size(px(12.0))
                            .text_color(theme::colors().text_secondary)
                            .child(self.t("workspace.no-recent")),
                    )
                }),
        )
    }
}

fn workspace_action(
    id: &'static str,
    icon: &'static str,
    label: String,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .w(px(230.0))
        .h(px(96.0))
        .p_3()
        .rounded(px(6.0))
        .bg(theme::colors().background_tertiary)
        .flex()
        .flex_col()
        .justify_between()
        .text_color(theme::colors().text_primary)
        .hover(|style| style.bg(theme::colors().button_background_hover))
        .child(
            div()
                .text_size(px(24.0))
                .text_color(theme::colors().text_secondary)
                .child(icon),
        )
        .child(div().text_size(px(14.0)).child(label))
}
