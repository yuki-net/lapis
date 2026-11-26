use super::*;
use crate::components::{Icon, IconName, ScrollAxis, scroll_viewport};
use crate::features::settings_coordinator::Change;
use crate::theme;
use gpui::{Context, Focusable, Window, div, px};
use settings::{
    Label, OptionSource, SettingControl, SettingDefinition, SettingKey, SettingOption, SettingValue,
};

impl SettingsFeature {
    /// 候補を持たない値を編集する。確定するまで保存しない。
    fn begin_text_edit(&mut self, key: SettingKey, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.as_ref().is_some_and(|edit| edit.key == key) {
            return;
        }
        let current = self.values.text(key).unwrap_or_default().to_owned();
        let placeholder = self.t("settings.value.placeholder");
        let input = cx.new(|cx| crate::components::SearchInput::new(&placeholder, cx));
        input.update(cx, |input, cx| input.set_text(current.clone(), cx));
        crate::gpui_compat::focus(window, &input.read(cx).focus_handle(cx), cx);
        let subscription = cx.subscribe(&input, |this, _, event, cx| {
            let crate::components::SearchInputEvent::Changed(text) = event;
            if let Some(edit) = this.editing.as_mut() {
                edit.draft = text.clone();
            }
            cx.notify();
        });
        self.editing = Some(TextEdit {
            key,
            input,
            draft: current,
            _subscription: subscription,
        });
        cx.notify();
    }

    pub(super) fn commit_text_edit(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = self.editing.take() else {
            return;
        };
        if edit.draft.trim().is_empty() {
            self.coordinator
                .update(cx, |coordinator, cx| coordinator.clear(edit.key, cx));
        } else {
            self.choose(edit.key, SettingValue::text(edit.draft), cx);
        }
    }

    pub(super) fn cancel_text_edit(&mut self, cx: &mut Context<Self>) {
        if self.editing.take().is_some() {
            cx.notify();
        }
    }

    /// 保存する値は定義データの候補から来るため、UI 側で型を組み立てない。
    ///
    /// 配布元が実際に変わったときだけ、以前の場所を同じ操作で消す。場所の意味は取得元
    /// ごとに変わるため、別の取得元へ古い path/URL を流用しない。同じ取得元の再選択では
    /// 保存済みの場所を残す。
    pub(crate) fn choose(&mut self, key: SettingKey, value: SettingValue, cx: &mut Context<Self>) {
        let source_changed = key == settings::UPDATE_SOURCE
            && self.values.text(settings::UPDATE_SOURCE) != value.as_text();
        let changes = if source_changed {
            Change::update(vec![
                (key, Some(value)),
                (settings::UPDATE_SOURCE_LOCATION, None),
            ])
        } else {
            Change::set(key, value)
        };
        self.coordinator
            .update(cx, |coordinator, cx| coordinator.change(changes, cx));
    }

    /// 1項目は定義データだけで決まる。値の保存キーも定義から取る。
    pub(super) fn render_field(
        &self,
        definition: &SettingDefinition,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let key = definition.key;
        let current = self.effective_value(definition, cx);
        let origin = self.values.origin(key).map_or_else(
            || self.t("settings.scope.default"),
            |scope| self.t(scope_label(scope)),
        );
        let label = div()
            .w(px(150.0))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_1()
            .text_size(px(13.0))
            .text_color(theme::colors(cx).text_primary)
            .child(self.t(definition.label))
            .children(definition.description.map(|description| {
                div()
                    .text_size(px(11.0))
                    .text_color(theme::colors(cx).text_tertiary)
                    .child(self.t(description))
            }));
        let control = match &definition.control {
            SettingControl::Choice(_) => {
                let options = self.options(definition, cx);
                let selected = options
                    .iter()
                    .find(|(value, _)| Some(value) == current.as_ref())
                    .map(|(_, label)| label.clone())
                    .unwrap_or_else(|| self.t("settings.value.unset"));
                let open = self.open_choice == Some(key);
                let choice_height = px((options.len() as f32 * 34.0).clamp(34.0, 200.0));
                let keyboard_options = options.clone();
                let keyboard_current = current.clone();
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .id(gpui::SharedString::from(format!(
                                "settings-choice-{}",
                                key.as_str()
                            )))
                            .debug_selector(move || format!("settings-choice-{}", key.as_str()))
                            .w_full()
                            .px_3()
                            .py_2()
                            .rounded(px(4.0))
                            .border_1()
                            .border_color(theme::colors(cx).border_default)
                            .bg(theme::colors(cx).background_secondary)
                            .hover(|style| style.bg(theme::colors(cx).button_background_hover))
                            .cursor_pointer()
                            .focusable()
                            .flex()
                            .items_center()
                            .justify_between()
                            .text_size(px(12.0))
                            .text_color(theme::colors(cx).text_primary)
                            .child(selected)
                            .child(Icon::new(IconName::ChevronDown))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_choice = if this.open_choice == Some(key) {
                                    None
                                } else {
                                    Some(key)
                                };
                                this.choice_scroll
                                    .handle()
                                    .set_offset(gpui::point(px(0.0), px(0.0)));
                                cx.notify();
                            }))
                            .on_key_down(cx.listener(
                                move |this, event: &gpui::KeyDownEvent, _, cx| {
                                    if event.keystroke.key == "enter"
                                        || event.keystroke.key == "space"
                                    {
                                        cx.stop_propagation();
                                        this.open_choice = if this.open_choice == Some(key) {
                                            None
                                        } else {
                                            Some(key)
                                        };
                                        this.choice_scroll
                                            .handle()
                                            .set_offset(gpui::point(px(0.0), px(0.0)));
                                        cx.notify();
                                    } else if event.keystroke.key == "escape"
                                        && this.open_choice == Some(key)
                                    {
                                        cx.stop_propagation();
                                        this.open_choice = None;
                                        cx.notify();
                                    } else if matches!(event.keystroke.key.as_str(), "up" | "down")
                                        && !keyboard_options.is_empty()
                                    {
                                        cx.stop_propagation();
                                        let index = keyboard_options
                                            .iter()
                                            .position(|(value, _)| {
                                                Some(value) == keyboard_current.as_ref()
                                            })
                                            .unwrap_or(0);
                                        let next = if event.keystroke.key == "down" {
                                            (index + 1).min(keyboard_options.len() - 1)
                                        } else {
                                            index.saturating_sub(1)
                                        };
                                        this.choose(key, keyboard_options[next].0.clone(), cx);
                                    }
                                },
                            )),
                    )
                    .when(open, |control| {
                        let choices = div().w_full().flex().flex_col().children(
                            options
                                .into_iter()
                                .enumerate()
                                .map(|(index, (value, label))| {
                                    let selected = current.as_ref() == Some(&value);
                                    div()
                                        .id(gpui::SharedString::from(format!(
                                            "{}-{index}",
                                            key.as_str()
                                        )))
                                        .debug_selector(move || format!("{}-{index}", key.as_str()))
                                        .px_3()
                                        .py_2()
                                        .rounded(px(4.0))
                                        .bg(if selected {
                                            theme::colors(cx).button_background_selected
                                        } else {
                                            theme::colors(cx).background_secondary
                                        })
                                        .hover(|style| {
                                            style.bg(theme::colors(cx).button_background_hover)
                                        })
                                        .cursor_pointer()
                                        .text_size(px(12.0))
                                        .text_color(theme::colors(cx).text_primary)
                                        .child(label)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.open_choice = None;
                                            this.choose(key, value.clone(), cx);
                                        }))
                                }),
                        );
                        control.child(div().h(choice_height).child(scroll_viewport(
                            gpui::SharedString::from(format!("settings-choices-{}", key.as_str())),
                            ScrollAxis::Vertical,
                            &self.choice_scroll,
                            choices,
                            cx,
                        )))
                    })
                    .into_any_element()
            }
            SettingControl::Text => {
                let value_text = current
                    .as_ref()
                    .and_then(|value| value.as_text().map(ToOwned::to_owned))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| self.t("settings.value.unset"));
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id(gpui::SharedString::from(format!(
                                "settings-value-{}",
                                key.as_str()
                            )))
                            .flex_1()
                            .min_w(px(0.0))
                            .px_3()
                            .py_2()
                            .rounded(px(4.0))
                            .border_1()
                            .border_color(theme::colors(cx).border_default)
                            .bg(theme::colors(cx).background_secondary)
                            .text_size(px(12.0))
                            .child(value_text),
                    )
                    .child(
                        div()
                            .id(gpui::SharedString::from(format!(
                                "settings-edit-{}",
                                key.as_str()
                            )))
                            .px_3()
                            .py_2()
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .text_size(px(12.0))
                            .text_color(theme::colors(cx).text_accent)
                            .child(self.t("settings.value.edit"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.begin_text_edit(key, window, cx)
                            })),
                    )
                    .into_any_element()
            }
        };
        div()
            .w_full()
            .flex()
            .items_start()
            .gap_4()
            .child(label)
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(control)
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(theme::colors(cx).text_tertiary)
                            .child(origin),
                    ),
            )
    }

    /// 未保存の項目は、供給元が示す実行時の既定値を選択状態として見せる。
    pub(super) fn effective_value(
        &self,
        definition: &SettingDefinition,
        cx: &gpui::App,
    ) -> Option<SettingValue> {
        if let Some(value) = self.values.value(definition.key) {
            return Some(value.clone());
        }
        // 実行時の既定値を持つキーは、その機能が登録した供給元から取る。
        if let Some(value) =
            crate::features::settings_sources::SettingsSources::default_value(cx, definition.key)
        {
            return Some(value);
        }
        if definition.key == settings::TERMINAL_CARET_SHAPE {
            let default = settings::TerminalCaretShape::default();
            return Some(SettingValue::text(default.as_id()));
        }
        None
    }

    /// 候補は定義データが指す供給元から取る。項目固有の分岐はここだけに閉じる。
    pub(super) fn options(
        &self,
        definition: &SettingDefinition,
        cx: &mut Context<Self>,
    ) -> Vec<(SettingValue, String)> {
        let SettingControl::Choice(source) = &definition.control else {
            return Vec::new();
        };
        match source {
            OptionSource::Values(options) => options
                .iter()
                .map(|option| (option.value.clone(), self.option_label(option)))
                .collect(),
            OptionSource::Themes => theme::available(cx)
                .into_iter()
                .map(|(id, name)| (SettingValue::text(id.as_str()), name))
                .collect(),
            OptionSource::Locales => self
                .locale
                .available()
                .into_iter()
                .map(|(id, name)| (SettingValue::text(id.as_str()), name))
                .collect(),
            // 候補が実行時の状態に依存するキーは、機能が登録した供給元から取る。
            // 設定 UI は供給元の型を知らない。
            OptionSource::Provided => {
                crate::features::settings_sources::SettingsSources::options(cx, definition.key)
            }
        }
    }

    pub(super) fn option_label(&self, option: &SettingOption) -> String {
        match option.label {
            Some(Label::Key(key)) => self.t(key),
            Some(Label::Literal(text)) => text.to_owned(),
            None => value_text(&option.value),
        }
    }
}
