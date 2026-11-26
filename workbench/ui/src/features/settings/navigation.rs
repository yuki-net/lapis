use super::*;
use crate::components::{ScrollAxis, scroll_items_viewport, scroll_viewport};
use crate::theme;
use gpui::{Context, div, px};
use settings::{OptionSource, SettingControl, SettingDefinition};

impl SettingsFeature {
    /// そのページをこの起動構成で表示できるか。
    ///
    /// 接続ページは供給元の有無ではなく取得結果で決める。供給元が `None` を返す
    /// 起動構成では、空のページをナビへ出さない。
    pub(super) fn page_available(&self, page: &settings::SettingPage) -> bool {
        if self.extension_terminal && matches!(page.id, "terminal.font" | "terminal.startup") {
            return false;
        }
        match page.content {
            settings::SettingPageContent::Feature => {
                page.id != settings::catalog::CONNECTION_WEB_PAGE || self.web_access().is_some()
            }
            settings::SettingPageContent::Settings => true,
        }
    }

    /// 組み込み設定ページを隠すのは、そのページを所有する拡張が公開中の場合だけ。
    pub(crate) fn set_legacy_terminal_settings_hidden(&mut self, hidden: bool) {
        self.extension_terminal = hidden;
    }

    pub(super) fn select_page(&mut self, page: &'static str, cx: &mut Context<Self>) {
        if let Some(index) = self
            .visible_pages()
            .iter()
            .position(|candidate| *candidate == page)
        {
            self.content_scroll.handle().scroll_to_top_of_item(index);
            self.pending_scroll = true;
        }
        if self.page != page {
            self.page = page;
            self.web_message.clear();
            if page == "host.web" {
                self.web_request(WebOperation::Refresh, cx);
            }
        }
        cx.notify();
    }

    pub(super) fn visible_pages(&self) -> Vec<&'static str> {
        let mut pages = Vec::new();
        let catalog = settings::catalog();
        for category in catalog.categories() {
            for group in category.groups {
                for id in group.pages {
                    if let Some(page) = catalog.page(id).filter(|page| self.page_available(page))
                        && (self.matches_search(&[
                            self.t(category.label),
                            self.t(group.label),
                            self.t(page.label),
                            page.description.map(|key| self.t(key)).unwrap_or_default(),
                        ]) || catalog
                            .settings_of(page.id)
                            .any(|field| self.field_matches_search(field)))
                    {
                        pages.push(page.id);
                    }
                }
            }
        }
        if self.web.is_some()
            && self.matches_search(&[
                self.t("settings.web.publish-title"),
                self.t("settings.web.devices"),
                self.t("settings.web.address"),
                self.t("settings.web.invite"),
            ])
        {
            pages.push("host.web");
        }
        let catalog_matches = self
            .extension
            .as_ref()
            .and_then(|session| session.snapshot())
            .is_some_and(|snapshot| {
                snapshot.extensions().iter().any(|entry| {
                    self.matches_search(&[
                        entry.id.clone(),
                        entry.version.clone(),
                        self.t("settings.extensions.title"),
                    ])
                })
            });
        if catalog_matches
            || self
                .extension_rows()
                .iter()
                .any(|row| self.matches_search(&[row.1.clone(), row.2.clone(), row.0.clone()]))
        {
            pages.push("extensions.settings");
        }
        pages
    }

    pub(super) fn matches_search(&self, fields: &[String]) -> bool {
        let text = fields.join(" ").to_lowercase();
        self.query
            .split_whitespace()
            .all(|term| text.contains(&term.to_lowercase()))
    }

    pub(super) fn field_matches_search(&self, definition: &SettingDefinition) -> bool {
        let catalog = settings::catalog();
        let page = catalog.page(definition.page);
        let category = page.and_then(|page| {
            catalog
                .categories()
                .iter()
                .find(|category| category.id == page.category)
        });
        let group = page.and_then(|page| {
            category
                .and_then(|category| category.groups.iter().find(|group| group.id == page.group))
        });
        let mut fields = vec![
            category
                .map(|value| self.t(value.label))
                .unwrap_or_default(),
            group.map(|value| self.t(value.label)).unwrap_or_default(),
            page.map(|value| self.t(value.label)).unwrap_or_default(),
            definition.key.as_str().to_owned(),
            self.t(definition.label),
            definition
                .description
                .map(|key| self.t(key))
                .unwrap_or_default(),
        ];
        if let SettingControl::Choice(OptionSource::Values(options)) = &definition.control {
            fields.extend(options.iter().map(|option| self.option_label(option)));
        }
        self.matches_search(&fields)
    }

    fn page_heading_matches_search(&self, page: &settings::SettingPage) -> bool {
        let catalog = settings::catalog();
        let category = catalog
            .categories()
            .iter()
            .find(|category| category.id == page.category);
        let group = category
            .and_then(|category| category.groups.iter().find(|group| group.id == page.group));
        self.matches_search(&[
            category
                .map(|value| self.t(value.label))
                .unwrap_or_default(),
            group.map(|value| self.t(value.label)).unwrap_or_default(),
            self.t(page.label),
            page.description.map(|key| self.t(key)).unwrap_or_default(),
        ])
    }

    pub(super) fn render_settings_content(&self, cx: &mut Context<Self>) -> gpui::Div {
        let workspace_info = self
            .root
            .as_deref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| self.t("settings.no-workspace"));
        let nav = self.render_nav(cx).into_any_element();
        let mut items = self
            .visible_pages()
            .into_iter()
            .map(|page| self.render_page(page, cx).flex_shrink_0())
            .collect::<Vec<_>>();
        if items.is_empty() {
            items.push(
                div()
                    .text_size(px(13.0))
                    .text_color(theme::colors(cx).text_tertiary)
                    .child(self.t("settings.search.empty")),
            );
        } else if self.query.trim().is_empty() {
            items.push(
                self.render_workspace_info(cx, workspace_info)
                    .flex_shrink_0(),
            );
        }
        let on_scroll = cx.listener(|this, _, _, cx| {
            this.sync_page_on_scroll = true;
            cx.notify();
        });
        let content = scroll_items_viewport(
            "settings-content-scroll",
            &self.content_scroll,
            items,
            on_scroll,
            cx,
        );

        div()
            .size_full()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .items_center()
            .pt_8()
            .px_4()
            .gap_4()
            .child(
                div()
                    .id("settings-search")
                    .debug_selector(|| "settings-search".into())
                    .w_full()
                    .max_w(px(920.0))
                    .child(self.search.clone()),
            )
            .child(
                div()
                    .w_full()
                    .max_w(px(920.0))
                    .flex_1()
                    .min_h(px(0.0))
                    .flex()
                    .gap_6()
                    .child(
                        div()
                            .w(px(220.0))
                            .flex_shrink_0()
                            .h_full()
                            .child(scroll_viewport(
                                "settings-nav-scroll",
                                ScrollAxis::Vertical,
                                &self.nav_scroll,
                                nav,
                                cx,
                            )),
                    )
                    .child(div().flex_1().min_w(px(0.0)).h_full().child(content)),
            )
    }

    /// 分類と中間層は分類専用の行として並べ、ページだけを選べるようにする。
    fn render_nav(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let catalog = settings::catalog();
        let visible = self.visible_pages();
        div()
            .id("main-settings-nav")
            .debug_selector(|| "main-settings-nav".into())
            .w(px(220.0))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_1()
            .children(
                catalog
                    .categories()
                    .iter()
                    .filter(|category| {
                        category
                            .groups
                            .iter()
                            .any(|group| group.pages.iter().any(|id| visible.contains(id)))
                    })
                    .map(|category| {
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .px_3()
                                    .pt_3()
                                    .pb_1()
                                    .text_size(px(11.0))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(theme::colors(cx).text_tertiary)
                                    .child(self.t(category.label)),
                            )
                            .children(
                                category
                                    .groups
                                    .iter()
                                    .filter(|group| {
                                        group.pages.iter().any(|id| visible.contains(id))
                                    })
                                    .map(|group| {
                                        div()
                                            .flex()
                                            .flex_col()
                                            .child(
                                                div()
                                                    .px_5()
                                                    .py_1()
                                                    .text_size(px(11.0))
                                                    .text_color(theme::colors(cx).text_tertiary)
                                                    .child(self.t(group.label)),
                                            )
                                            .children(
                                                group
                                                    .pages
                                                    .iter()
                                                    .filter_map(|id| catalog.page(id))
                                                    .filter(|page| visible.contains(&page.id))
                                                    .map(|page| {
                                                        let selected = page.id == self.page;
                                                        div()
                                                            .id(page.id)
                                                            .px_5()
                                                            .py_2()
                                                            .rounded(px(6.0))
                                                            .when(selected, |row| {
                                                                row.bg(theme::colors(cx)
                                                                    .button_background_selected)
                                                            })
                                                            .hover(|style| {
                                                                style.bg(theme::colors(cx)
                                                                    .button_background_hover)
                                                            })
                                                            .text_size(px(12.0))
                                                            .text_color(
                                                                theme::colors(cx).text_primary,
                                                            )
                                                            .child(self.t(page.label))
                                                            .on_click(cx.listener(
                                                                move |this, _, _, cx| {
                                                                    this.select_page(page.id, cx)
                                                                },
                                                            ))
                                                    }),
                                            )
                                    }),
                            )
                    }),
            )
            .when(visible.contains(&"host.web"), |nav| {
                let selected = self.page == "host.web";
                nav.child(
                    div()
                        .id("settings-host-web")
                        .debug_selector(|| "settings-host-web".into())
                        .px_5()
                        .py_2()
                        .rounded(px(6.0))
                        .when(selected, |row| {
                            row.bg(theme::colors(cx).button_background_selected)
                        })
                        .hover(|style| style.bg(theme::colors(cx).button_background_hover))
                        .text_size(px(12.0))
                        .text_color(theme::colors(cx).text_primary)
                        .child(self.t("settings.web.publish-title"))
                        .focusable()
                        .on_click(cx.listener(|this, _, _, cx| this.select_page("host.web", cx))),
                )
            })
            .when(visible.contains(&"extensions.settings"), |nav| {
                nav.child(
                    div()
                        .id("settings-extensions")
                        .px_5()
                        .py_2()
                        .rounded(px(6.0))
                        .when(self.page == "extensions.settings", |row| {
                            row.bg(theme::colors(cx).button_background_selected)
                        })
                        .hover(|style| style.bg(theme::colors(cx).button_background_hover))
                        .text_size(px(12.0))
                        .text_color(theme::colors(cx).text_primary)
                        .child(self.t("settings.extensions.title"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.select_page("extensions.settings", cx)
                        })),
                )
            })
    }

    fn render_page(&self, page_id: &'static str, cx: &mut Context<Self>) -> gpui::Div {
        if page_id == "extensions.settings" {
            return self.render_extensions_page(cx);
        }
        if page_id == "host.web" {
            return self.render_web_page(cx);
        }
        let catalog = settings::catalog();
        if page_id == settings::catalog::CONNECTION_WEB_PAGE && self.web_access().is_some() {
            return self.render_web_connection_page(cx);
        }
        let page = catalog.page(page_id).or_else(|| catalog.first_page());
        let mut container = div().w_full().min_w(px(0.0)).flex().flex_col().gap_4();
        let Some(page) = page else {
            return container.child(self.t("settings.empty"));
        };
        container = container.child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_size(px(16.0))
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(theme::colors(cx).text_primary)
                        .child(self.t(page.label)),
                )
                .children(page.description.map(|description| {
                    div()
                        .text_size(px(12.0))
                        .text_color(theme::colors(cx).text_tertiary)
                        .child(self.t(description))
                })),
        );
        let show_page = self.page_heading_matches_search(page);
        let fields = catalog
            .settings_of(page.id)
            .filter(|definition| show_page || self.field_matches_search(definition))
            .map(|definition| self.render_field(definition, cx))
            .collect::<Vec<_>>();
        if !fields.is_empty() {
            container = container.child(
                div()
                    .w_full()
                    .p_4()
                    .rounded(px(8.0))
                    .bg(theme::colors(cx).background_tertiary)
                    .flex()
                    .flex_col()
                    .gap_3()
                    .children(fields),
            );
        }
        if let Some(edit) = &self.editing
            && catalog
                .settings_of(page.id)
                .any(|definition| definition.key == edit.key)
        {
            let input = edit.input.clone();
            let key = edit.key.as_str();
            container = container.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_4()
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(theme::colors(cx).border_default)
                    .bg(theme::colors(cx).background_tertiary)
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(theme::colors(cx).text_tertiary)
                            .child(self.t("settings.value.edit-description")),
                    )
                    .child(input)
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                div()
                                    .id(gpui::SharedString::from(format!(
                                        "settings-text-save-{key}"
                                    )))
                                    .px_3()
                                    .py_2()
                                    .rounded(px(6.0))
                                    .bg(theme::colors(cx).button_background_selected)
                                    .text_size(px(12.0))
                                    .text_color(theme::colors(cx).text_accent)
                                    .child(self.t("settings.value.save"))
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.commit_text_edit(cx)),
                                    ),
                            )
                            .child(
                                div()
                                    .id(gpui::SharedString::from(format!(
                                        "settings-text-cancel-{key}"
                                    )))
                                    .px_3()
                                    .py_2()
                                    .rounded(px(6.0))
                                    .border_1()
                                    .border_color(theme::colors(cx).border_default)
                                    .text_size(px(12.0))
                                    .text_color(theme::colors(cx).text_primary)
                                    .child(self.t("settings.value.cancel"))
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.cancel_text_edit(cx)),
                                    ),
                            ),
                    )
                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                        if event.keystroke.key != "enter" {
                            return;
                        }
                        cx.stop_propagation();
                        this.commit_text_edit(cx);
                    })),
            );
        }
        container
    }
}
