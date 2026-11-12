use std::path::PathBuf;

use super::settings_coordinator::{Change, SettingsCoordinator};
use crate::{
    components::{Icon, IconName},
    localization::Localizer,
    theme,
};
use gpui::{Context, Render, Window, div, prelude::*, px};
use settings::{
    Label, OptionSource, ResolvedSettings, SettingControl, SettingDefinition, SettingKey,
    SettingOption, SettingValue, SettingsScope,
};

/// 左ナビ3層（分類 → 中間層 → ページ）と、定義データから組み立てる設定ページ。
pub(crate) struct SettingsFeature {
    coordinator: gpui::Entity<SettingsCoordinator>,
    _subscription: gpui::Subscription,
    _catalogs: Vec<gpui::Subscription>,
    values: ResolvedSettings,
    root: Option<PathBuf>,
    locale: Localizer,
    message: String,
    page: &'static str,
    /// ローカル Web の接続情報を都度集める供給元。無い構成では接続ページを出さない。
    web_access: Option<crate::app::WebAccessSource>,
    /// 接続中の Workspace ID。Web の初回接続で入力する値として表示する。
    workspace_id: Option<String>,
    /// 接続ページで選んでいる候補ホスト。
    selected_web_host: Option<String>,
    /// コピーやブラウザー起動の結果を一時的に示す。
    web_message: String,
}

impl gpui::EventEmitter<ResolvedSettings> for SettingsFeature {}

impl SettingsFeature {
    #[cfg(test)]
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        Self::with_web_access(cx, None)
    }

    /// 接続情報の供給元を受け取って作る。テストは同じ入口を使う。
    pub(crate) fn with_web_access(
        cx: &mut Context<Self>,
        web_access: Option<crate::app::WebAccessSource>,
    ) -> Self {
        let coordinator = SettingsCoordinator::shared(cx);
        let values = coordinator.read(cx).values.clone();
        let mut locale = Localizer::bundled();
        let _ = locale.set_active(&values.locale());
        let subscription = cx.observe(&coordinator, |this, coordinator, cx| {
            let state = coordinator.read(cx);
            this.values = state.values.clone();
            this.message = state.message.clone();
            let _ = this.locale.set_active(&this.values.locale());
            cx.emit(this.values.clone());
            cx.notify();
        });
        // Terminal Feature が Host の profile 一覧を公開したら選択肢を作り直す。
        let catalogs =
            vec![cx.observe_global::<super::terminal::TerminalProfileCatalog>(|_, cx| cx.notify())];
        Self {
            coordinator,
            _subscription: subscription,
            _catalogs: catalogs,
            values,
            root: None,
            locale,
            message: String::new(),
            page: default_page(),
            web_access,
            workspace_id: None,
            selected_web_host: None,
            web_message: String::new(),
        }
    }

    pub(crate) fn set_workspace(
        &mut self,
        root: Option<PathBuf>,
        workspace_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.root != root || self.workspace_id != workspace_id {
            self.root = root;
            self.workspace_id = workspace_id;
            cx.notify();
        }
    }

    fn t(&self, key: &str) -> String {
        self.locale.resolve(&localization::MessageId::new(key))
    }

    /// 保存する値は定義データの候補から来るため、UI 側で型を組み立てない。
    pub(crate) fn choose(&mut self, key: SettingKey, value: SettingValue, cx: &mut Context<Self>) {
        self.coordinator.update(cx, |coordinator, cx| {
            coordinator.change(Change::set(key, value), cx)
        });
    }

    /// そのページをこの起動構成で表示できるか。
    fn page_available(&self, page: &settings::SettingPage) -> bool {
        match page.content {
            settings::SettingPageContent::Feature => {
                page.id != settings::catalog::CONNECTION_WEB_PAGE || self.web_access.is_some()
            }
            settings::SettingPageContent::Settings => true,
        }
    }

    fn select_page(&mut self, page: &'static str, cx: &mut Context<Self>) {
        if self.page != page {
            self.page = page;
            self.web_message.clear();
            cx.notify();
        }
    }

    /// 現在の接続情報。供給元が無い、または取得できない場合は `None`。
    fn web_access(&self) -> Option<desktop_client::WebAccess> {
        self.web_access.as_ref()?.as_ref()()
    }

    /// 表示中の候補アドレス。選んだホストが消えていれば先頭へ戻す。
    ///
    /// 1回の描画では同じ `access` を使い、候補と URL が食い違わないようにする。
    fn selected_web_address(
        &self,
        access: &desktop_client::WebAccess,
    ) -> Option<desktop_client::WebAddress> {
        match self.selected_web_host.as_deref() {
            Some(host)
                if access
                    .addresses()
                    .iter()
                    .any(|address| address.host == host) =>
            {
                access
                    .addresses()
                    .iter()
                    .find(|address| address.host == host)
                    .cloned()
            }
            _ => access.preferred().cloned(),
        }
    }

    fn select_web_host(&mut self, host: String, cx: &mut Context<Self>) {
        if self.selected_web_host.as_deref() != Some(host.as_str()) {
            self.selected_web_host = Some(host);
            self.web_message.clear();
            cx.notify();
        }
    }

    /// 表示中のページ URL をクリップボードへ書き、画面にも知らせる。
    fn copy_web_url(&mut self, value: String, copied_key: &'static str, cx: &mut Context<Self>) {
        if value.is_empty() {
            return;
        }
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(value));
        self.web_message = self.t(copied_key);
        cx.notify();
    }

    pub(super) fn render_settings_content(&self, cx: &mut Context<Self>) -> gpui::Div {
        let workspace_info = self
            .root
            .as_deref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| self.t("settings.no-workspace"));

        div()
            .w_full()
            .flex()
            .flex_col()
            .py_8()
            .gap_6()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(Icon::new(IconName::Settings))
                    .child(
                        div()
                            .text_size(px(22.0))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(theme::colors(cx).text_primary)
                            .child(self.t("settings.title")),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .max_w(px(920.0))
                    .flex()
                    .gap_6()
                    .child(self.render_nav(cx))
                    .child(self.render_page(cx)),
            )
            .child(self.render_workspace_info(cx, workspace_info))
    }

    /// 分類と中間層は分類専用の行として並べ、ページだけを選べるようにする。
    fn render_nav(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let catalog = settings::catalog();
        div()
            .id("main-settings-nav")
            .w(px(220.0))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_1()
            .children(catalog.categories().iter().map(|category| {
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
                    .children(category.groups.iter().map(|group| {
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
                                    .filter(|page| self.page_available(page))
                                    .map(|page| {
                                        let selected = page.id == self.page;
                                        div()
                                            .id(page.id)
                                            .px_5()
                                            .py_2()
                                            .rounded(px(6.0))
                                            .bg(if selected {
                                                theme::colors(cx).button_background_selected
                                            } else {
                                                theme::colors(cx).background_primary
                                            })
                                            .hover(|style| {
                                                style.bg(theme::colors(cx).button_background_hover)
                                            })
                                            .text_size(px(12.0))
                                            .text_color(if selected {
                                                theme::colors(cx).text_accent
                                            } else {
                                                theme::colors(cx).text_primary
                                            })
                                            .child(self.t(page.label))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.select_page(page.id, cx)
                                            }))
                                    }),
                            )
                    }))
            }))
    }
    fn render_page(&self, cx: &mut Context<Self>) -> gpui::Div {
        let catalog = settings::catalog();
        if self.page == settings::catalog::CONNECTION_WEB_PAGE && self.web_access.is_some() {
            return self.render_web_connection_page(cx);
        }
        let page = catalog.page(self.page).or_else(|| catalog.first_page());
        let mut container = div().flex_1().min_w(px(0.0)).flex().flex_col().gap_4();
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
        for definition in catalog.settings_of(page.id) {
            container = container.child(self.render_field(definition, cx));
        }
        container
    }

    /// 現在の接続情報を示すページ。保存値を持たないため、表示のたびに集め直す。
    ///
    /// コピーと「ブラウザーで開く」は同じ候補アドレスを対象にする。
    fn render_web_connection_page(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut container = div()
            .flex_1()
            .min_w(px(0.0))
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(16.0))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(theme::colors(cx).text_primary)
                            .child(self.t("settings.page.connection.web")),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(theme::colors(cx).text_tertiary)
                            .child(self.t("settings.page.connection.web.description")),
                    ),
            );
        let Some(access) = self.web_access() else {
            return container.child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme::colors(cx).text_tertiary)
                    .child(self.t("settings.web.unavailable")),
            );
        };
        let access = access.with_loopback_fallback();
        let selected = self.selected_web_address(&access);
        let page_url = selected
            .as_ref()
            .map(|address| address.page_url(access.page_port));
        let bridge_url = selected
            .as_ref()
            .map(|address| address.bridge_url(access.bridge_port));

        container =
            container.child(
                div()
                    .p_4()
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(theme::colors(cx).border_default)
                    .bg(theme::colors(cx).background_tertiary)
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(theme::colors(cx).text_tertiary)
                            .child(self.t("settings.web.address")),
                    )
                    .child(div().flex().flex_wrap().gap_2().children(
                        access.addresses().iter().map(|address| {
                            let host = address.host.clone();
                            let selected = selected
                                .as_ref()
                                .is_some_and(|current| current.same_host(address));
                            div()
                                .id(gpui::ElementId::Name(format!("web-address-{host}").into()))
                                .debug_selector({
                                    let host = host.clone();
                                    move || format!("web-address-{host}")
                                })
                                .px_3()
                                .py_2()
                                .rounded(px(6.0))
                                .border_1()
                                .border_color(if selected {
                                    theme::colors(cx).text_accent
                                } else {
                                    theme::colors(cx).border_default
                                })
                                .bg(if selected {
                                    theme::colors(cx).button_background_selected
                                } else {
                                    theme::colors(cx).background_secondary
                                })
                                .hover(|style| style.bg(theme::colors(cx).button_background_hover))
                                .text_size(px(12.0))
                                .text_color(theme::colors(cx).text_primary)
                                .child(format!(
                                    "{} {}",
                                    self.t(address.kind.label_key()),
                                    address.host
                                ))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.select_web_host(host.clone(), cx)
                                }))
                        }),
                    )),
            );

        container = container.child(self.render_web_value_row(
            "web-page-url",
            "settings.web.page-url",
            page_url.clone(),
            "settings.web.copy-page-url",
            "settings.web.copied",
            cx,
        ));
        container = container.child(self.render_web_value_row(
            "web-bridge-url",
            "settings.web.bridge-url",
            bridge_url.clone(),
            "settings.web.copy-bridge-url",
            "settings.web.copied",
            cx,
        ));

        container = container.child(
            div()
                .flex()
                .items_center()
                .flex_wrap()
                .gap_2()
                .child(
                    div()
                        .id("web-open-browser")
                        .debug_selector(|| "web-open-browser".into())
                        .px_3()
                        .py_2()
                        .rounded(px(6.0))
                        .border_1()
                        .border_color(theme::colors(cx).border_default)
                        .bg(theme::colors(cx).background_secondary)
                        .text_size(px(12.0))
                        .text_color(if page_url.is_some() {
                            theme::colors(cx).text_primary
                        } else {
                            theme::colors(cx).text_tertiary
                        })
                        .child(self.t("settings.web.open"))
                        .when_some(page_url, |button, url| {
                            button
                                .cursor_pointer()
                                .hover(|style| style.bg(theme::colors(cx).button_background_hover))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.open_url(&url);
                                    this.web_message = this.t("settings.web.opened");
                                    cx.notify();
                                }))
                        }),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(theme::colors(cx).text_tertiary)
                        .child(self.web_message.clone()),
                ),
        );

        // Desktop は接続中の Workspace をそのまま初回入力の候補として出す。
        if let Some(workspace_id) = access.workspace_id.or_else(|| self.workspace_id.clone()) {
            container = container.child(self.render_web_value_row(
                "web-workspace-id",
                "settings.web.workspace-id",
                Some(workspace_id),
                "settings.web.copy-bridge-url",
                "settings.web.copied",
                cx,
            ));
        }
        container.child(
            div()
                .text_size(px(11.0))
                .text_color(theme::colors(cx).text_tertiary)
                .child(self.t("settings.web.hint")),
        )
    }

    /// 値・コピー・動作の3点を1行に並べる。値が無いときはボタンを無効にする。
    fn render_web_value_row(
        &self,
        id: &'static str,
        label: &'static str,
        value: Option<String>,
        copy_label: &'static str,
        copied_label: &'static str,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let shown = value.clone().unwrap_or_else(|| "-".to_owned());
        div()
            .p_4()
            .rounded(px(8.0))
            .border_1()
            .border_color(theme::colors(cx).border_default)
            .bg(theme::colors(cx).background_tertiary)
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme::colors(cx).text_tertiary)
                    .child(self.t(label)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        div()
                            .id(id)
                            .debug_selector(move || id.into())
                            .flex_1()
                            .min_w(px(0.0))
                            .text_size(px(12.0))
                            .text_color(theme::colors(cx).text_primary)
                            .child(shown),
                    )
                    .child(
                        div()
                            .id(gpui::ElementId::Name(format!("{id}-copy").into()))
                            .debug_selector(move || format!("{id}-copy"))
                            .px_3()
                            .py_2()
                            .rounded(px(6.0))
                            .border_1()
                            .border_color(theme::colors(cx).border_default)
                            .bg(theme::colors(cx).background_secondary)
                            .text_size(px(12.0))
                            .text_color(if value.is_some() {
                                theme::colors(cx).text_primary
                            } else {
                                theme::colors(cx).text_tertiary
                            })
                            .child(self.t(copy_label))
                            .when_some(value, |button, value| {
                                button
                                    .cursor_pointer()
                                    .hover(|style| {
                                        style.bg(theme::colors(cx).button_background_hover)
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.copy_web_url(value.clone(), copied_label, cx)
                                    }))
                            }),
                    ),
            )
    }

    /// 1項目は定義データだけで決まる。値の保存キーも定義から取る。
    fn render_field(&self, definition: &SettingDefinition, cx: &mut Context<Self>) -> gpui::Div {
        let key = definition.key;
        let current = self.effective_value(definition, cx);
        let origin = match self.values.origin(key) {
            Some(scope) => self.t(scope_label(scope)),
            None => self.t("settings.scope.default"),
        };

        div()
            .p_4()
            .rounded(px(8.0))
            .border_1()
            .border_color(theme::colors(cx).border_default)
            .bg(theme::colors(cx).background_tertiary)
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(theme::colors(cx).text_primary)
                            .child(self.t(definition.label)),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(theme::colors(cx).text_tertiary)
                            .child(origin),
                    ),
            )
            .children(definition.description.map(|description| {
                div()
                    .text_size(px(12.0))
                    .text_color(theme::colors(cx).text_tertiary)
                    .child(self.t(description))
            }))
            .child(div().flex().flex_wrap().gap_2().children(
                self.options(definition, cx).into_iter().enumerate().map(
                    |(index, (value, label))| {
                        let selected = current.as_ref() == Some(&value);
                        div()
                            .id((key.as_str(), index))
                            .px_3()
                            .py_2()
                            .rounded(px(6.0))
                            .border_1()
                            .border_color(if selected {
                                theme::colors(cx).text_accent
                            } else {
                                theme::colors(cx).border_default
                            })
                            .bg(if selected {
                                theme::colors(cx).button_background_selected
                            } else {
                                theme::colors(cx).background_secondary
                            })
                            .hover(|style| style.bg(theme::colors(cx).button_background_hover))
                            .text_size(px(12.0))
                            .text_color(theme::colors(cx).text_primary)
                            .child(format!("{} {label}", if selected { "✓" } else { "" }))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.choose(key, value.clone(), cx)
                            }))
                    },
                ),
            ))
    }

    /// 未保存の項目は、UI が持つ実行時の既定値を選択状態として見せる。
    fn effective_value(
        &self,
        definition: &SettingDefinition,
        cx: &gpui::App,
    ) -> Option<SettingValue> {
        if let Some(value) = self.values.value(definition.key) {
            return Some(value.clone());
        }
        if definition.key == settings::TERMINAL_FONT_FAMILY {
            return Some(SettingValue::text(super::terminal::default_font_family()));
        }
        if definition.key == settings::TERMINAL_FONT_SIZE {
            return Some(SettingValue::integer(i64::from(
                super::terminal::default_font_size(),
            )));
        }
        if definition.key == settings::TERMINAL_LINE_HEIGHT {
            return Some(SettingValue::integer(i64::from(
                super::terminal::default_line_height_percent(),
            )));
        }
        if definition.key == settings::TERMINAL_CARET_SHAPE {
            let default = settings::TerminalCaretShape::default();
            return Some(SettingValue::text(default.as_id()));
        }
        if definition.key == settings::TERMINAL_DEFAULT_PROFILE {
            // 未保存なら Host の既定 profile を選択状態として見せる。
            return host_terminal_default_profile(cx).map(SettingValue::text);
        }
        None
    }

    /// 候補は定義データが指す供給元から取る。項目固有の分岐はここだけに閉じる。
    fn options(
        &self,
        definition: &SettingDefinition,
        cx: &mut Context<Self>,
    ) -> Vec<(SettingValue, String)> {
        let SettingControl::Choice(source) = &definition.control;
        match source {
            OptionSource::Values(options) => options
                .iter()
                .map(|option| (option.value.clone(), self.option_label(option)))
                .collect(),
            OptionSource::Themes => theme::available(cx)
                .into_iter()
                .map(|(id, name)| (SettingValue::text(id.as_str()), name))
                .collect(),
            OptionSource::Locales => vec![
                (SettingValue::text("ja-JP"), "日本語 (Japanese)".to_owned()),
                (SettingValue::text("en-US"), "English (英語)".to_owned()),
            ],
            OptionSource::TerminalFonts => super::terminal::font_choices()
                .iter()
                .map(|family| (SettingValue::text(*family), (*family).to_owned()))
                .collect(),
            OptionSource::TerminalFontSizes => super::terminal::font_size_choices()
                .iter()
                .map(|size| (SettingValue::integer(i64::from(*size)), size.to_string()))
                .collect(),
            OptionSource::TerminalLineHeights => super::terminal::line_height_choices()
                .iter()
                .map(|percent| {
                    (
                        SettingValue::integer(i64::from(*percent)),
                        format!("{percent}%"),
                    )
                })
                .collect(),
            OptionSource::TerminalProfiles => {
                let catalog = super::terminal::profile_catalog(cx);
                catalog
                    .profiles
                    .into_iter()
                    .map(|(id, name)| (SettingValue::text(id), name))
                    .collect()
            }
        }
    }

    fn option_label(&self, option: &SettingOption) -> String {
        match option.label {
            Some(Label::Key(key)) => self.t(key),
            Some(Label::Literal(text)) => text.to_owned(),
            None => value_text(&option.value),
        }
    }

    fn render_workspace_info(&self, cx: &mut Context<Self>, workspace_info: String) -> gpui::Div {
        div()
            .w(px(600.0))
            .max_w_full()
            .p_4()
            .rounded(px(8.0))
            .border_1()
            .border_color(theme::colors(cx).border_default)
            .bg(theme::colors(cx).background_tertiary)
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme::colors(cx).text_tertiary)
                    .child(self.t("settings.workspace-info")),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme::colors(cx).text_primary)
                    .child(workspace_info),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme::colors(cx).text_tertiary)
                    .child("Version: 0.1.0"),
            )
    }
}

fn default_page() -> &'static str {
    settings::catalog()
        .first_page()
        .map_or("appearance.theme", |page| page.id)
}

/// Host が既定として公開している Terminal profile ID。未取得なら `None`。
fn host_terminal_default_profile(cx: &gpui::App) -> Option<String> {
    super::terminal::profile_catalog(cx).default
}

fn scope_label(scope: &SettingsScope) -> &'static str {
    match scope {
        SettingsScope::User => "settings.scope.user",
        SettingsScope::Host { .. } => "settings.scope.host",
        SettingsScope::ProjectShared { .. } => "settings.scope.project-shared",
        SettingsScope::ProjectLocal { .. } => "settings.scope.project-local",
    }
}

fn value_text(value: &SettingValue) -> String {
    match value {
        SettingValue::Text(text) => text.clone(),
        SettingValue::Integer(number) => number.to_string(),
        SettingValue::Flag(flag) => flag.to_string(),
    }
}

/// 設定ページの描画に使う定義データ。テストからも同じものを参照する。
#[cfg(test)]
pub(super) fn catalog_for_tests() -> &'static settings::DefinitionCatalog {
    settings::catalog()
}

impl Render for SettingsFeature {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w_full()
            .flex()
            .flex_col()
            .child(self.message.clone())
            .child(self.render_settings_content(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension_ui::ThemeId;
    use gpui::AppContext;
    use settings::{SettingsDocument, SettingsError, SettingsRepository, SyntaxTheme};
    use std::sync::Arc;

    struct MemorySettings;
    impl SettingsRepository for MemorySettings {
        fn load(&self) -> Result<SettingsDocument, SettingsError> {
            Ok(SettingsDocument::empty(SettingsScope::User))
        }
        fn save(&self, _: &SettingsDocument) -> Result<(), SettingsError> {
            Ok(())
        }
    }

    fn wait_for_settings(cx: &mut gpui::TestAppContext) {
        let coordinator = cx.update(|cx| SettingsCoordinator::shared(cx));
        let deadline = crate::time::Instant::now() + std::time::Duration::from_secs(3);
        while cx.update(|cx| !coordinator.read(cx).test_is_idle()) {
            assert!(
                crate::time::Instant::now() < deadline,
                "settings queue did not drain"
            );
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(20));
            cx.run_until_parked();
            std::thread::yield_now();
        }
    }

    #[gpui::test]
    fn changes_from_different_windows_keep_the_other_theme_selection(
        cx: &mut gpui::TestAppContext,
    ) {
        let (first, second) = cx.update(|cx| {
            theme::init(&ThemeId::new("lapis.dark"), None, cx);
            let settings = desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
            SettingsCoordinator::init(settings, cx);
            (cx.new(SettingsFeature::new), cx.new(SettingsFeature::new))
        });
        cx.run_until_parked();
        cx.update(|cx| {
            first.update(cx, |feature, cx| {
                feature.choose(settings::UI_THEME, SettingValue::text("lapis.white"), cx)
            });
            second.update(cx, |feature, cx| {
                feature.choose(
                    settings::UI_SYNTAX_THEME,
                    SettingValue::text(SyntaxTheme::Zed.as_id()),
                    cx,
                )
            });
        });
        wait_for_settings(cx);
        cx.update(|cx| {
            assert_eq!(theme::active_id(cx).as_str(), "lapis.white");
            assert_eq!(theme::syntax::active_family(cx), SyntaxTheme::Zed);
        });
    }

    #[gpui::test]
    fn two_features_observe_shared_theme_syntax_and_locale(cx: &mut gpui::TestAppContext) {
        let (first, second) = cx.update(|cx| {
            theme::init(&ThemeId::new("lapis.dark"), None, cx);
            let settings = desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
            SettingsCoordinator::init(settings, cx);
            (cx.new(SettingsFeature::new), cx.new(SettingsFeature::new))
        });
        cx.run_until_parked();
        cx.update(|cx| {
            first.update(cx, |feature, cx| {
                feature.choose(settings::UI_THEME, SettingValue::text("lapis.white"), cx)
            });
            second.update(cx, |feature, cx| {
                feature.choose(
                    settings::UI_SYNTAX_THEME,
                    SettingValue::text(SyntaxTheme::Zed.as_id()),
                    cx,
                )
            });
            first.update(cx, |feature, cx| {
                feature.choose(settings::UI_LOCALE, SettingValue::text("ja-JP"), cx)
            });
        });
        wait_for_settings(cx);
        cx.update(|cx| {
            for feature in [&first, &second] {
                let values = &feature.read(cx).values;
                assert_eq!(values.theme(), "lapis.white");
                assert_eq!(values.syntax_theme(), Some(SyntaxTheme::Zed));
                assert_eq!(values.locale().as_str(), "ja-JP");
            }
        });
    }

    #[gpui::test]
    fn settings_navigation_renders_only_leaf_pages(cx: &mut gpui::TestAppContext) {
        let catalog = catalog_for_tests();
        let leaf = catalog
            .categories()
            .iter()
            .flat_map(|category| category.groups.iter())
            .flat_map(|group| group.pages.iter())
            .count();
        assert_eq!(leaf, catalog.pages().len());
        assert!(catalog.first_page().is_some());
        let _ = cx;
    }

    /// Host が公開した profile が Settings の候補になることを、描画と同じ経路で確かめる。
    #[gpui::test]
    fn default_terminal_profile_options_come_from_the_published_catalog(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            theme::init(&ThemeId::new("lapis.dark"), None, cx);
            let settings = desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
            SettingsCoordinator::init(settings, cx);
            crate::features::terminal::publish_profile_catalog(
                &terminal::TerminalProfiles::new(
                    vec![
                        terminal::TerminalProfile::new(
                            "powershell",
                            "PowerShell",
                            "powershell.exe",
                        ),
                        terminal::TerminalProfile::new("cmd", "Command Prompt", "cmd.exe"),
                    ],
                    Some(terminal::TerminalProfileId::new("powershell")),
                ),
                cx,
            );
        });
        let feature = cx.update(|cx| cx.new(SettingsFeature::new));
        let definition = settings::catalog()
            .definition(settings::TERMINAL_DEFAULT_PROFILE)
            .expect("定義がある");
        feature.update(cx, |feature, cx| {
            let options = feature.options(definition, cx);
            let values = options
                .iter()
                .map(|(value, label)| {
                    (
                        value.as_text().unwrap_or_default().to_owned(),
                        label.clone(),
                    )
                })
                .collect::<Vec<_>>();
            assert_eq!(
                values,
                vec![
                    ("powershell".to_owned(), "PowerShell".to_owned()),
                    ("cmd".to_owned(), "Command Prompt".to_owned()),
                ]
            );
            // 未保存の既定値は Host の既定 profile を選択状態にする。
            assert_eq!(
                feature
                    .effective_value(definition, cx)
                    .and_then(|value| value.as_text().map(ToOwned::to_owned)),
                Some("powershell".to_owned())
            );
        });
    }

    fn web_addresses() -> Vec<desktop_client::WebAddress> {
        vec![
            desktop_client::WebAddress::new(desktop_client::WebAddressKind::Local, "127.0.0.1"),
            desktop_client::WebAddress::new(
                desktop_client::WebAddressKind::Network,
                "192.168.1.20",
            ),
            desktop_client::WebAddress::new(
                desktop_client::WebAddressKind::Tailscale,
                "100.64.1.23",
            ),
        ]
    }

    fn selected_web_address(feature: &SettingsFeature) -> Option<desktop_client::WebAddress> {
        let access = feature.web_access()?.with_loopback_fallback();
        feature.selected_web_address(&access)
    }

    fn web_feature(
        cx: &mut gpui::TestAppContext,
        addresses: Vec<desktop_client::WebAddress>,
        workspace_id: Option<String>,
    ) -> gpui::Entity<SettingsFeature> {
        cx.update(|cx| {
            theme::init(&ThemeId::new("lapis.dark"), None, cx);
            let settings = desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
            SettingsCoordinator::init(settings, cx);
            let source: crate::app::WebAccessSource = Arc::new(move || {
                Some(desktop_client::WebAccess::new(
                    4747,
                    4757,
                    addresses.clone(),
                    workspace_id.clone(),
                ))
            });
            cx.new(|cx| SettingsFeature::with_web_access(cx, Some(source)))
        })
    }

    /// 接続ページは保存値ではなく、いま集めた候補を表示する。
    #[gpui::test]
    fn connection_page_shows_the_current_web_addresses(cx: &mut gpui::TestAppContext) {
        let feature = web_feature(cx, web_addresses(), Some("workspace-1".to_owned()));
        cx.update(|cx| {
            feature.update(cx, |feature, cx| {
                feature.select_page(settings::catalog::CONNECTION_WEB_PAGE, cx);
            });
            let access = feature.read(cx).web_access().expect("接続情報がある");
            assert_eq!(access.addresses().len(), 3);
            let selected = selected_web_address(feature.read(cx)).expect("候補がある");
            assert_eq!(selected.host, "127.0.0.1");
            assert_eq!(selected.page_url(4747), "http://127.0.0.1:4747/");
            assert_eq!(selected.bridge_url(4757), "ws://127.0.0.1:4757/remote");
        });
    }

    /// 候補を選ぶと、コピーと「ブラウザーで開く」の対象がその候補へ変わる。
    #[gpui::test]
    fn choosing_a_web_address_changes_the_urls_to_open_and_copy(cx: &mut gpui::TestAppContext) {
        let feature = web_feature(cx, web_addresses(), None);
        cx.update(|cx| {
            feature.update(cx, |feature, cx| {
                feature.select_page(settings::catalog::CONNECTION_WEB_PAGE, cx);
                feature.select_web_host("100.64.1.23".to_owned(), cx);
            });
            let selected = selected_web_address(feature.read(cx)).expect("候補がある");
            assert_eq!(selected.host, "100.64.1.23");
            assert_eq!(selected.page_url(4747), "http://100.64.1.23:4747/");
            assert_eq!(selected.bridge_url(4757), "ws://100.64.1.23:4757/remote");
        });
    }

    /// Tailscale の IP が変わっても、表示は供給元の現在値へ追随する。
    #[gpui::test]
    fn web_addresses_follow_the_current_network(cx: &mut gpui::TestAppContext) {
        let current = std::sync::Arc::new(std::sync::Mutex::new("100.64.1.23".to_owned()));
        let observed = current.clone();
        let feature = cx.update(|cx| {
            theme::init(&ThemeId::new("lapis.dark"), None, cx);
            let settings = desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
            SettingsCoordinator::init(settings, cx);
            let source: crate::app::WebAccessSource = Arc::new(move || {
                let host = observed.lock().unwrap().clone();
                Some(desktop_client::WebAccess::new(
                    4747,
                    4757,
                    vec![
                        desktop_client::WebAddress::new(
                            desktop_client::WebAddressKind::Local,
                            "127.0.0.1",
                        ),
                        desktop_client::WebAddress::new(
                            desktop_client::WebAddressKind::Tailscale,
                            host,
                        ),
                    ],
                    None,
                ))
            });
            cx.new(|cx| SettingsFeature::with_web_access(cx, Some(source)))
        });
        cx.update(|cx| {
            feature.update(cx, |feature, cx| {
                feature.select_page(settings::catalog::CONNECTION_WEB_PAGE, cx);
                feature.select_web_host("100.64.1.23".to_owned(), cx);
            });
            *current.lock().unwrap() = "100.64.9.9".to_owned();
            // 保存値ではないため、選んだホストが消えていれば現在の候補へ戻る。
            let selected = selected_web_address(feature.read(cx)).expect("候補がある");
            assert_eq!(selected.host, "127.0.0.1");
            let access = feature.read(cx).web_access().expect("接続情報がある");
            assert!(
                access
                    .addresses()
                    .iter()
                    .any(|address| address.host == "100.64.9.9")
            );
        });
    }

    /// Desktop は接続中の Workspace ID を初回入力の候補として表示する。
    #[gpui::test]
    fn connection_page_shows_the_connected_workspace_id(cx: &mut gpui::TestAppContext) {
        let feature = web_feature(cx, web_addresses(), None);
        cx.update(|cx| {
            feature.update(cx, |feature, cx| {
                feature.set_workspace(None, Some("workspace-9".to_owned()), cx);
                feature.select_page(settings::catalog::CONNECTION_WEB_PAGE, cx);
            });
            assert_eq!(
                feature
                    .read(cx)
                    .web_access()
                    .and_then(|access| access.workspace_id)
                    .or_else(|| feature.read(cx).workspace_id.clone()),
                Some("workspace-9".to_owned())
            );
        });
    }

    /// 接続情報を出せない構成では、空のページをナビへ出さない。
    #[gpui::test]
    fn connection_page_is_hidden_without_a_web_source(cx: &mut gpui::TestAppContext) {
        let feature = cx.update(|cx| {
            theme::init(&ThemeId::new("lapis.dark"), None, cx);
            let settings = desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
            SettingsCoordinator::init(settings, cx);
            cx.new(SettingsFeature::new)
        });
        let page = settings::catalog()
            .page(settings::catalog::CONNECTION_WEB_PAGE)
            .expect("接続ページの定義がある");
        cx.update(|cx| {
            assert!(!feature.read(cx).page_available(page));
        });
        let available = web_feature(cx, web_addresses(), None);
        cx.update(|cx| {
            assert!(available.read(cx).page_available(page));
        });
    }

    /// 描画した接続ページのボタンが、実際にコピーとブラウザー起動を行う。
    #[gpui::test]
    fn connection_page_buttons_copy_and_open_the_selected_address(cx: &mut gpui::TestAppContext) {
        use gpui::Modifiers;

        let (feature, visual) = cx.add_window_view(|_, cx| {
            theme::init(&ThemeId::new("lapis.dark"), None, cx);
            let settings = desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
            SettingsCoordinator::init(settings, cx);
            let source: crate::app::WebAccessSource = std::sync::Arc::new(|| {
                Some(desktop_client::WebAccess::new(
                    4747,
                    4757,
                    vec![
                        desktop_client::WebAddress::new(
                            desktop_client::WebAddressKind::Local,
                            "127.0.0.1",
                        ),
                        desktop_client::WebAddress::new(
                            desktop_client::WebAddressKind::Tailscale,
                            "100.64.1.23",
                        ),
                    ],
                    None,
                ))
            });
            SettingsFeature::with_web_access(cx, Some(source))
        });
        feature.update(visual, |feature, cx| {
            feature.select_page(settings::catalog::CONNECTION_WEB_PAGE, cx)
        });
        visual.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear();
        });

        // Tailscale の候補を選ぶと、コピーと起動の対象がその候補へ変わる。
        let tailscale = visual
            .debug_bounds("web-address-100.64.1.23")
            .expect("候補が出ている");
        visual.simulate_click(tailscale.center(), Modifiers::none());
        visual.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear();
        });
        let copy = visual
            .debug_bounds("web-page-url-copy")
            .expect("ページ URL のコピー操作が出ている");
        visual.simulate_click(copy.center(), Modifiers::none());
        assert_eq!(
            visual.read_from_clipboard().and_then(|item| item.text()),
            Some("http://100.64.1.23:4747/".to_owned())
        );

        let open = visual
            .debug_bounds("web-open-browser")
            .expect("ブラウザーで開く操作が出ている");
        visual.simulate_click(open.center(), Modifiers::none());
        assert_eq!(
            visual.opened_url().as_deref(),
            Some("http://100.64.1.23:4747/")
        );
    }
}
