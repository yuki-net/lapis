//! 更新の同意画面と控えめな通知。
//!
//! 軸A（通常更新）は install の可否を尋ねず、「次回起動で適用される」ことだけを知らせる。
//! 軸B（`breaking: true`）は data 世代が変わるため、失う範囲を示して同意を取る。
//!
//! 状態は [`desktop_client::UpdateWatch`] を購読して受け取り、操作は
//! [`desktop_client::UpdateController`] だけを呼ぶ。通信・file 操作はアプリ側が持つ。

use desktop_client::{UpdateController, UpdateStatus, UpdateWatch};
use gpui::{Context, FocusHandle, Focusable, Render, Window, div, prelude::*, px};

use crate::app::UpdaterHandle;
use crate::{localization::Localizer, theme, tokens};

/// 更新の表示状態。Root が 1 つだけ持つ。
pub(crate) struct UpdateFeature {
    controller: std::sync::Arc<dyn UpdateController>,
    watch: UpdateWatch,
    status: UpdateStatus,
    locale: Localizer,
    /// 設定変更を更新処理へ知らせる入口。
    settings_changed: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
    /// 同意画面の focus。表示時に移し、キーボードだけで操作できるようにする。
    focus: FocusHandle,
    /// 同意画面へ focus を移した状態。表示ごとに 1 回だけ移す。
    focused: bool,
}

impl Focusable for UpdateFeature {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl UpdateFeature {
    pub(crate) fn new(updater: UpdaterHandle, cx: &mut Context<Self>) -> Self {
        let status = updater.watch.status();
        Self {
            controller: updater.controller.clone(),
            watch: updater.watch.clone(),
            status,
            locale: Localizer::bundled(),
            settings_changed: updater.settings_changed.clone(),
            focus: cx.focus_handle(),
            focused: false,
        }
        .subscribe(updater.interval, cx)
    }

    /// 同意画面の操作。Enter で同意、Escape で辞退する。
    fn consent_key(&mut self, event: &gpui::KeyDownEvent, cx: &mut Context<Self>) {
        if self.consent().is_none() {
            return;
        }
        match event.keystroke.key.as_str() {
            "enter" => {
                cx.stop_propagation();
                self.controller.consent_breaking();
            }
            "escape" => {
                cx.stop_propagation();
                self.dismiss(cx);
            }
            _ => {}
        }
    }

    fn dismiss(&mut self, cx: &mut Context<Self>) {
        self.controller.dismiss_breaking();
        self.status = UpdateStatus::Idle;
        self.focused = false;
        cx.notify();
    }

    /// 更新方針の設定が変わったときに呼ぶ。変わっていなければ何もしない。
    pub(crate) fn settings_changed(&mut self, cx: &mut Context<Self>) {
        if let Some(changed) = &self.settings_changed {
            changed();
        }
        self.status = self.watch.status();
        cx.notify();
    }

    /// 購読を開始し、起動時にもう一度確認する。失敗しても起動は止めない。
    fn subscribe(self, interval: std::time::Duration, cx: &mut Context<Self>) -> Self {
        let watch = self.watch.clone();
        // 購読は 1 回だけ登録し、同じ receiver で後続の状態を待つ。
        // 反復ごとに subscribe すると初期値を即座に受け取り、更新が無くても
        // 通知と background task を繰り返してしまう。receiver は Clone できないため
        // lock で共有し、待つ処理だけを background executor に渡す。
        let receiver = std::sync::Arc::new(std::sync::Mutex::new(watch.subscribe()));
        cx.spawn(async move |this, cx| {
            loop {
                let shared = receiver.clone();
                let notified = cx.background_executor().spawn(async move {
                    shared.lock().ok().and_then(|receiver| receiver.recv().ok())
                });
                let Some(status) = notified.await else {
                    return;
                };
                if this
                    .update(cx, |feature, cx| {
                        feature.status = status;
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
        })
        .detach();
        // 起動時の確認と、セッション中の定期確認を同じ経路で回す。
        self.controller.check();
        let controller = self.controller.clone();
        cx.spawn(async move |_, cx| {
            loop {
                // Timer を直接 await する。background_executor の spawn は wasm では
                // Send を要求できず、UI thread の timer と混在できない。
                crate::timer::Timer::after(interval).await;
                let _ = cx;
                controller.check();
            }
        })
        .detach();
        self
    }

    pub(crate) fn set_locale(&mut self, locale: localization::LocaleId, cx: &mut Context<Self>) {
        if self.locale.set_active(&locale) {
            cx.notify();
        }
    }

    fn t(&self, key: &str) -> String {
        self.locale.resolve(&localization::MessageId::new(key))
    }

    /// 通知として出す状態か。同意画面は別に描画する。
    fn notice(&self) -> Option<(String, String)> {
        match &self.status {
            UpdateStatus::Staged {
                version,
                breaking: false,
            } => Some((
                self.t("update.notice.staged.title"),
                self.t("update.notice.staged.message")
                    .replace("{version}", version),
            )),
            UpdateStatus::Staged {
                version,
                breaking: true,
            } => Some((
                self.t("update.notice.breaking-staged.title"),
                self.t("update.notice.breaking-staged.message")
                    .replace("{version}", version),
            )),
            UpdateStatus::Available { version, .. } => Some((
                self.t("update.notice.available.title"),
                self.t("update.notice.available.message")
                    .replace("{version}", version),
            )),
            UpdateStatus::Idle
            | UpdateStatus::Checking
            | UpdateStatus::Breaking { .. }
            | UpdateStatus::Failed { .. } => None,
        }
    }

    /// 同意待ちの破壊的更新。失う範囲を明示する。
    fn consent(&self) -> Option<(&str, &str, u32)> {
        match &self.status {
            UpdateStatus::Breaking {
                version,
                notes_url,
                current_generation,
            } => Some((version, notes_url, *current_generation)),
            _ => None,
        }
    }
}

impl Render for UpdateFeature {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = theme::colors(cx);
        if let Some((version, notes_url, generation)) = self.consent() {
            let version = version.to_owned();
            let notes_url = notes_url.to_owned();
            // 同意画面を表示したら focus を移す。キーボードだけでも同意・辞退できる。
            if !self.focused {
                crate::gpui_compat::focus(window, &self.focus, cx);
                self.focused = true;
            }
            return div()
                .id("update-consent")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::rgba(0x00000088))
                .track_focus(&self.focus)
                .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                    this.consent_key(event, cx);
                }))
                .child(
                    crate::components::surface(crate::components::SurfaceVariant::Popover, cx)
                        .w(px(520.0))
                        .max_w(gpui::relative(0.9))
                        .p(px(20.0))
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(
                            div()
                                .text_size(px(16.0))
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_color(colors.text_primary)
                                .child(self.t("update.consent.title")),
                        )
                        .child(
                            div()
                                .text_size(px(13.0))
                                .text_color(colors.text_primary)
                                .child(
                                    self.t("update.consent.message")
                                        .replace("{version}", &version),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(colors.text_tertiary)
                                .child(
                                    self.t("update.consent.scope")
                                        .replace("{generation}", &generation.to_string()),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(colors.text_tertiary)
                                .child(self.t("update.consent.credentials")),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(colors.text_tertiary)
                                .child(notes_url),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    div()
                                        .id("update-consent-decline")
                                        .px_3()
                                        .py_2()
                                        .rounded(px(6.0))
                                        .border_1()
                                        .border_color(colors.border_default)
                                        .text_size(px(12.0))
                                        .text_color(colors.text_primary)
                                        .child(self.t("update.consent.decline"))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.dismiss(cx);
                                        })),
                                )
                                .child(
                                    div()
                                        .id("update-consent-accept")
                                        .px_3()
                                        .py_2()
                                        .rounded(px(6.0))
                                        .bg(colors.button_background_selected)
                                        .text_size(px(12.0))
                                        .text_color(colors.text_accent)
                                        .child(self.t("update.consent.accept"))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.controller.consent_breaking();
                                            cx.notify();
                                        })),
                                ),
                        ),
                )
                .into_any_element();
        }
        self.focused = false;
        match self.notice() {
            Some((title, message)) => div()
                .id("update-notice")
                .absolute()
                .bottom(px(16.0))
                .right(px(16.0))
                .max_w(px(360.0))
                .p(px(12.0))
                .rounded(tokens::radius::MENU)
                .border_1()
                .border_color(colors.floating_border)
                .bg(colors.floating_background)
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(colors.text_tertiary)
                        .child(message),
                )
                .into_any_element(),
            None => div().into_any_element(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Off;
    impl UpdateController for Off {
        fn check(&self) {}
        fn consent_breaking(&self) {}
        fn dismiss_breaking(&self) {}
    }

    fn feature(cx: &mut gpui::TestAppContext, status: UpdateStatus) -> UpdateFeature {
        let focus = cx.update(|cx| cx.focus_handle());
        UpdateFeature {
            controller: std::sync::Arc::new(Off),
            watch: UpdateWatch::new(),
            status,
            locale: Localizer::bundled(),
            settings_changed: None,
            focus,
            focused: false,
        }
    }

    #[gpui::test]
    fn only_axis_b_shows_the_consent_screen(cx: &mut gpui::TestAppContext) {
        let breaking = feature(
            cx,
            UpdateStatus::Breaking {
                version: "1.0.0".into(),
                notes_url: "https://example.invalid".into(),
                current_generation: 1,
            },
        );
        assert!(breaking.consent().is_some());
        assert!(breaking.notice().is_none());

        // 軸A の適用は確認せず、次回起動の案内だけを出す。
        let staged = feature(
            cx,
            UpdateStatus::Staged {
                version: "0.2.0".into(),
                breaking: false,
            },
        );
        assert!(staged.consent().is_none());
        assert!(staged.notice().is_some());
    }

    #[gpui::test]
    fn failures_and_idle_show_nothing(cx: &mut gpui::TestAppContext) {
        for status in [
            UpdateStatus::Idle,
            UpdateStatus::Checking,
            UpdateStatus::Failed {
                message: "offline".into(),
            },
        ] {
            let feature = feature(cx, status);
            assert!(feature.consent().is_none());
            assert!(feature.notice().is_none());
        }
    }
}
