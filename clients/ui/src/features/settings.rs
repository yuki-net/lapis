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
    values: ResolvedSettings,
    root: Option<PathBuf>,
    locale: Localizer,
    message: String,
    page: &'static str,
}

impl gpui::EventEmitter<ResolvedSettings> for SettingsFeature {}

impl SettingsFeature {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
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
        Self {
            coordinator,
            _subscription: subscription,
            values,
            root: None,
            locale,
            message: String::new(),
            page: default_page(),
        }
    }

    pub(crate) fn set_workspace(&mut self, root: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.root != root {
            self.root = root;
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

    fn select_page(&mut self, page: &'static str, cx: &mut Context<Self>) {
        if self.page != page {
            self.page = page;
            cx.notify();
        }
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
            .p_8()
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
                            .children(group.pages.iter().filter_map(|id| catalog.page(id)).map(
                                |page| {
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
                                },
                            ))
                    }))
            }))
    }

    fn render_page(&self, cx: &mut Context<Self>) -> gpui::Div {
        let catalog = settings::catalog();
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

    /// 1項目は定義データだけで決まる。値の保存キーも定義から取る。
    fn render_field(&self, definition: &SettingDefinition, cx: &mut Context<Self>) -> gpui::Div {
        let key = definition.key;
        let current = self.effective_value(definition);
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
    fn effective_value(&self, definition: &SettingDefinition) -> Option<SettingValue> {
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
}
