use crate::{
    client_adapter::ClientWorker,
    components::{Icon, IconName},
    extension_ui::ThemeId,
    localization::Localizer,
    theme,
};
use gpui::{Context, Render, Window, div, prelude::*, px};
use lapis_desktop_client::SettingsSession;
pub(crate) struct SettingsFeature {
    worker: ClientWorker<SettingsSession>,
    values: lapis_settings::GlobalSettings,
    confirmed: lapis_settings::GlobalSettings,
    root: Option<std::path::PathBuf>,
    locale: Localizer,
    message: String,
    generation: u64,
}
impl gpui::EventEmitter<lapis_settings::GlobalSettings> for SettingsFeature {}

impl SettingsFeature {
    pub(crate) fn new(settings: SettingsSession) -> Self {
        let values = settings.settings();
        let mut locale = Localizer::bundled();
        let _ = locale.set_active(&values.locale);
        let _ = theme::set_active(&ThemeId::new(values.theme.clone()));
        theme::syntax::set_active(values.syntax_theme.clone());
        Self {
            worker: ClientWorker::new(settings),
            confirmed: values.clone(),
            values,
            root: None,
            locale,
            message: String::new(),
            generation: 0,
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
    fn apply_values(&mut self, values: lapis_settings::GlobalSettings, cx: &mut Context<Self>) {
        let _ = theme::set_active(&ThemeId::new(values.theme.clone()));
        theme::syntax::set_active(values.syntax_theme.clone());
        let _ = self.locale.set_active(&values.locale);
        self.values = values;
        cx.emit(self.values.clone());
        cx.notify();
    }

    fn change(
        &mut self,
        candidate: lapis_settings::GlobalSettings,
        operation: impl FnOnce(&mut SettingsSession) -> Result<(), lapis_settings::SettingsError>
        + Send
        + 'static,
        cx: &mut Context<Self>,
    ) {
        let generation = self.generation + 1;
        let admitted = self.worker.submit(
            cx,
            move |settings| {
                let result = operation(settings).map_err(|error| error.to_string());
                Ok((result, settings.settings()))
            },
            move |settings, result, cx| {
                if generation != settings.generation {
                    return;
                }
                match result {
                    Ok((outcome, values)) => {
                        settings.confirmed = values.clone();
                        settings.message = outcome.err().unwrap_or_default();
                        settings.apply_values(values, cx);
                    }
                    Err(error) => {
                        settings.message = error;
                        settings.apply_values(settings.confirmed.clone(), cx);
                    }
                }
            },
        );
        match admitted {
            Ok(()) => {
                self.generation = generation;
                self.message.clear();
                self.apply_values(candidate, cx);
            }
            Err(error) => {
                self.message = error;
                cx.notify();
            }
        }
    }

    pub(crate) fn select_theme(&mut self, theme_id: ThemeId, cx: &mut Context<Self>) {
        if !theme::available().iter().any(|(id, _)| id == &theme_id) {
            return;
        }
        let mut candidate = self.values.clone();
        candidate.theme = theme_id.as_str().to_owned();
        self.change(
            candidate,
            move |settings| settings.set_theme(theme_id.as_str().to_owned()),
            cx,
        );
    }

    pub(crate) fn select_locale(
        &mut self,
        locale: lapis_localization::LocaleId,
        cx: &mut Context<Self>,
    ) {
        let mut candidate = self.values.clone();
        candidate.locale = locale.clone();
        self.change(candidate, move |settings| settings.set_locale(locale), cx);
    }

    fn select_syntax_theme(&mut self, id: &'static str, cx: &mut Context<Self>) {
        let mut candidate = self.values.clone();
        candidate.syntax_theme = Some(id.to_owned());
        self.change(
            candidate,
            move |settings| settings.set_syntax_theme(id.to_owned()),
            cx,
        );
    }

    pub(super) fn render_settings_content(&self, cx: &mut Context<Self>) -> gpui::Div {
        let active_theme = theme::active_id();
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
                                .text_color(theme::colors().text_primary)
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
                                .border_color(theme::colors().border_default)
                                .bg(theme::colors().background_tertiary)
                                .flex()
                                .flex_col()
                                .gap_3()
                                .child(self.t("settings.syntax-theme"))
                                .child(div().flex().flex_wrap().gap_2().children(
                                    theme::syntax::PRESETS.iter().map(|preset| {
                                        let selected = theme::syntax::active().id == preset.id;
                                        let id = preset.id;
                                        crate::components::action_button(preset.name, selected)
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
                                .border_color(theme::colors().border_default)
                                .bg(theme::colors().background_tertiary)
                                .flex()
                                .flex_col()
                                .gap_3()
                                .child(
                                    div()
                                        .text_size(px(14.0))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(theme::colors().text_primary)
                                        .child(self.t("settings.appearance")),
                                )
                                .child(div().flex().flex_wrap().gap_2().children(
                                    theme::available().into_iter().map(|(id, name)| {
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
                                                theme::colors().text_accent
                                            } else {
                                                theme::colors().border_default
                                            })
                                            .bg(if is_selected {
                                                theme::colors().button_background_selected
                                            } else {
                                                theme::colors().background_tertiary
                                            })
                                            .hover(|style| {
                                                style.bg(theme::colors().button_background_hover)
                                            })
                                            .text_size(px(12.0))
                                            .text_color(theme::colors().text_primary)
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
                                .border_color(theme::colors().border_default)
                                .bg(theme::colors().background_tertiary)
                                .flex()
                                .flex_col()
                                .gap_3()
                                .child(
                                    div()
                                        .text_size(px(14.0))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(theme::colors().text_primary)
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
                                .border_color(theme::colors().border_default)
                                .bg(theme::colors().background_tertiary)
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(
                                    div()
                                        .text_size(px(14.0))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(theme::colors().text_primary)
                                        .child(self.t("settings.workspace-info")),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(theme::colors().text_tertiary)
                                        .child(format!("Workspace Root: {workspace_info}")),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(theme::colors().text_tertiary)
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
                theme::colors().text_accent
            } else {
                theme::colors().border_default
            })
            .bg(if is_selected {
                theme::colors().button_background_selected
            } else {
                theme::colors().background_tertiary
            })
            .hover(|style| style.bg(theme::colors().button_background_hover))
            .text_size(px(12.0))
            .text_color(theme::colors().text_primary)
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
