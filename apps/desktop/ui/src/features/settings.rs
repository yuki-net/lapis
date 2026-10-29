use super::settings_coordinator::{Change, SettingsCoordinator};
use crate::{
    components::{Icon, IconName},
    extension_ui::ThemeId,
    localization::Localizer,
    theme,
};
use gpui::{Context, Render, Window, div, prelude::*, px};
pub(crate) struct SettingsFeature {
    coordinator: gpui::Entity<SettingsCoordinator>,
    _subscription: gpui::Subscription,
    values: lapis_settings::GlobalSettings,
    root: Option<std::path::PathBuf>,
    locale: Localizer,
    message: String,
}
impl gpui::EventEmitter<lapis_settings::GlobalSettings> for SettingsFeature {}

impl SettingsFeature {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let coordinator = SettingsCoordinator::shared(cx);
        let values = coordinator.read(cx).values.clone();
        let mut locale = Localizer::bundled();
        let _ = locale.set_active(&values.locale);
        let subscription = cx.observe(&coordinator, |this, coordinator, cx| {
            let state = coordinator.read(cx);
            this.values = state.values.clone();
            this.message = state.message.clone();
            let _ = this.locale.set_active(&this.values.locale);
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
        }
    }
    pub(crate) fn set_workspace(
        &mut self,
        root: Option<std::path::PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if self.root != root {
            self.root = root;
            cx.notify();
        }
    }
    fn t(&self, key: &str) -> String {
        self.locale
            .resolve(&lapis_localization::MessageId::new(key))
    }
    pub(crate) fn select_theme(&mut self, theme_id: ThemeId, cx: &mut Context<Self>) {
        if theme::available(cx).iter().any(|(id, _)| id == &theme_id) {
            self.coordinator.update(cx, |coordinator, cx| {
                coordinator.change(Change::Theme(theme_id.as_str().to_owned()), cx)
            });
        }
    }

    pub(crate) fn select_locale(
        &mut self,
        locale: lapis_localization::LocaleId,
        cx: &mut Context<Self>,
    ) {
        self.coordinator.update(cx, |coordinator, cx| {
            coordinator.change(Change::Locale(locale), cx)
        });
    }

    fn select_syntax_theme(&mut self, id: lapis_settings::SyntaxTheme, cx: &mut Context<Self>) {
        self.coordinator.update(cx, |coordinator, cx| {
            coordinator.change(Change::Syntax(id), cx)
        });
    }

    pub(super) fn render_settings_content(&self, cx: &mut Context<Self>) -> gpui::Div {
        let active_theme = theme::active_id(cx);
        let current_locale = self.values.locale.clone();
        let workspace_info = self
            .root
            .as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| self.t("settings.no-workspace"));

        div().w_full().child(
            div()
                .id("main-settings-content")
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
                        .w(px(600.0))
                        .max_w_full()
                        .flex()
                        .flex_col()
                        .gap_4()
                        .child(
                            div()
                                .p_4()
                                .rounded(px(8.0))
                                .border_1()
                                .border_color(theme::colors(cx).border_default)
                                .bg(theme::colors(cx).background_tertiary)
                                .flex()
                                .flex_col()
                                .gap_3()
                                .child(self.t("settings.syntax-theme"))
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(theme::colors(cx).text_tertiary)
                                        .child(self.t("settings.syntax-theme-follows-ui")),
                                )
                                .child(div().flex().flex_wrap().gap_2().children(
                                    theme::syntax::FAMILIES.iter().map(|&(id, name)| {
                                        let selected = theme::syntax::active_family(cx) == id;
                                        crate::components::action_button(name, selected, cx)
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.select_syntax_theme(id, cx)
                                            }))
                                    }),
                                )),
                        )
                        .child(
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
                                        .text_size(px(14.0))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(theme::colors(cx).text_primary)
                                        .child(self.t("settings.appearance")),
                                )
                                .child(div().flex().flex_wrap().gap_2().children(
                                    theme::available(cx).into_iter().map(|(id, name)| {
                                        let is_selected = active_theme == id;
                                        let theme_id = id.clone();
                                        div()
                                            .id(gpui::SharedString::from(format!(
                                                "setting-theme-{}",
                                                id.as_str()
                                            )))
                                            .px_3()
                                            .py_2()
                                            .rounded(px(6.0))
                                            .border_1()
                                            .border_color(if is_selected {
                                                theme::colors(cx).text_accent
                                            } else {
                                                theme::colors(cx).border_default
                                            })
                                            .bg(if is_selected {
                                                theme::colors(cx).button_background_selected
                                            } else {
                                                theme::colors(cx).background_tertiary
                                            })
                                            .hover(|style| {
                                                style.bg(theme::colors(cx).button_background_hover)
                                            })
                                            .text_size(px(12.0))
                                            .text_color(theme::colors(cx).text_primary)
                                            .child(format!(
                                                "{} {}",
                                                if is_selected { "✓" } else { "" },
                                                name
                                            ))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.select_theme(theme_id.clone(), cx);
                                            }))
                                    }),
                                )),
                        )
                        .child(
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
                                        .text_size(px(14.0))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(theme::colors(cx).text_primary)
                                        .child(self.t("settings.language")),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .gap_2()
                                        .child(self.render_locale_button(
                                            "日本語 (Japanese)",
                                            "ja-JP",
                                            current_locale.as_str() == "ja-JP",
                                            cx,
                                        ))
                                        .child(self.render_locale_button(
                                            "English (英語)",
                                            "en-US",
                                            current_locale.as_str() == "en-US",
                                            cx,
                                        )),
                                ),
                        )
                        .child(
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
                                        .text_size(px(14.0))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(theme::colors(cx).text_primary)
                                        .child(self.t("settings.workspace-info")),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(theme::colors(cx).text_tertiary)
                                        .child(format!("Workspace Root: {workspace_info}")),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(theme::colors(cx).text_tertiary)
                                        .child("Version: 0.1.0"),
                                ),
                        ),
                ),
        )
    }

    fn render_locale_button(
        &self,
        label: &'static str,
        locale_code: &'static str,
        is_selected: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(label)
            .px_3()
            .py_2()
            .rounded(px(6.0))
            .border_1()
            .border_color(if is_selected {
                theme::colors(cx).text_accent
            } else {
                theme::colors(cx).border_default
            })
            .bg(if is_selected {
                theme::colors(cx).button_background_selected
            } else {
                theme::colors(cx).background_tertiary
            })
            .hover(|style| style.bg(theme::colors(cx).button_background_hover))
            .text_size(px(12.0))
            .text_color(theme::colors(cx).text_primary)
            .child(format!("{} {}", if is_selected { "✓" } else { "" }, label))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.select_locale(lapis_localization::LocaleId::new(locale_code), cx);
            }))
    }
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
    use gpui::AppContext;
    use lapis_settings::{GlobalSettings, GlobalSettingsRepository, SettingsError, SyntaxTheme};
    use std::sync::Arc;

    struct MemorySettings;
    impl GlobalSettingsRepository for MemorySettings {
        fn load(&self) -> Result<GlobalSettings, SettingsError> {
            Ok(GlobalSettings::default())
        }
        fn save(&self, _: &GlobalSettings) -> Result<(), SettingsError> {
            Ok(())
        }
    }

    #[gpui::test]
    fn changes_from_different_windows_keep_the_other_theme_selection(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            theme::init(&ThemeId::new("lapis.dark"), None, cx);
            let settings =
                lapis_desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
            SettingsCoordinator::init(settings, cx);
            let first = cx.new(SettingsFeature::new);
            let second = cx.new(SettingsFeature::new);
            first.update(cx, |feature, cx| {
                feature.select_theme(ThemeId::new("lapis.white"), cx)
            });
            second.update(cx, |feature, cx| {
                feature.select_syntax_theme(SyntaxTheme::Zed, cx)
            });
            assert_eq!(theme::active_id(cx).as_str(), "lapis.white");
            assert_eq!(theme::syntax::active_family(cx), SyntaxTheme::Zed);
        });
    }

    #[gpui::test]
    fn two_features_observe_shared_theme_syntax_and_locale(cx: &mut gpui::TestAppContext) {
        let (first, second) = cx.update(|cx| {
            theme::init(&ThemeId::new("lapis.dark"), None, cx);
            let settings =
                lapis_desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
            SettingsCoordinator::init(settings, cx);
            (cx.new(SettingsFeature::new), cx.new(SettingsFeature::new))
        });
        cx.run_until_parked();
        cx.update(|cx| {
            first.update(cx, |feature, cx| {
                feature.select_theme(ThemeId::new("lapis.white"), cx)
            });
            second.update(cx, |feature, cx| {
                feature.select_syntax_theme(SyntaxTheme::Zed, cx)
            });
            first.update(cx, |feature, cx| {
                feature.select_locale(lapis_localization::LocaleId::new("ja-JP"), cx)
            });
        });
        cx.run_until_parked();
        cx.update(|cx| {
            for feature in [&first, &second] {
                let values = &feature.read(cx).values;
                assert_eq!(values.theme, "lapis.white");
                assert_eq!(values.syntax_theme, Some(SyntaxTheme::Zed));
                assert_eq!(values.locale.as_str(), "ja-JP");
            }
        });
    }
}
