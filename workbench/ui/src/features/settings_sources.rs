//! 機能が登録した設定ソースを解決する Client 側の置き場。
//!
//! 定義データは「供給元から取る」（`OptionSource::Provided`）とだけ宣言し、
//! 実際の候補と既定値は、その状態を所有する機能が
//! [`settings::SettingsSourceProvider`] を実装して登録する。設定 UI は供給元の
//! 型を知らず、この置き場をキーで引くだけにする。
//!
//! 置き場自体を GPUI の Global にして、登録のたびに購読者へ通知する。設定画面は
//! 供給元の Feature 型を購読せず、置き場の更新だけを購読する。

use std::collections::BTreeMap;
use std::rc::Rc;

use settings::{SettingKey, SettingValue, SettingsSourceProvider};

/// 供給元の実体。1キーの所有者は1つに限る。
type Provider = Rc<dyn SettingsSourceProvider>;

/// 設定キーごとの供給元。
#[derive(Clone, Default)]
pub(crate) struct SettingsSources {
    providers: BTreeMap<SettingKey, Provider>,
}

impl gpui::Global for SettingsSources {}

impl SettingsSources {
    /// 置き場を用意する。設定画面より先に1回だけ呼ぶ。
    pub(crate) fn init(cx: &mut gpui::App) {
        if cx.try_global::<Self>().is_none() {
            cx.set_global(Self::default());
        }
    }

    /// 供給元を登録し、購読者へ通知する。同じキーは後から登録した側で置き換える。
    pub(crate) fn register(cx: &mut gpui::App, provider: Provider) {
        let mut next = cx.try_global::<Self>().cloned().unwrap_or_default();
        for key in provider.keys() {
            next.providers.insert(*key, provider.clone());
        }
        cx.set_global(next);
    }

    /// 選択候補と表示名。供給元が無いキーは空。
    pub(crate) fn options(cx: &gpui::App, key: SettingKey) -> Vec<(SettingValue, String)> {
        cx.try_global::<Self>()
            .and_then(|sources| sources.providers.get(&key))
            .map(|provider| provider.options(key))
            .unwrap_or_default()
    }

    /// 保存値が無いときに選択状態として見せる実効既定値。
    pub(crate) fn default_value(cx: &gpui::App, key: SettingKey) -> Option<SettingValue> {
        cx.try_global::<Self>()
            .and_then(|sources| sources.providers.get(&key))
            .and_then(|provider| provider.default_value(key))
    }

    /// 定義データが供給元を要求しているキーか。設定画面の分岐はここだけに閉じる。
    #[cfg(test)]
    pub(crate) fn is_provided(key: SettingKey) -> bool {
        settings::catalog()
            .definition(key)
            .is_some_and(|definition| {
                matches!(
                    definition.control,
                    settings::SettingControl::Choice(settings::OptionSource::Provided)
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use settings::{TERMINAL_DEFAULT_PROFILE, ValueKind};

    struct Fixed;

    impl SettingsSourceProvider for Fixed {
        fn keys(&self) -> &'static [SettingKey] {
            &[TERMINAL_DEFAULT_PROFILE]
        }

        fn options(&self, _: SettingKey) -> Vec<(SettingValue, String)> {
            vec![(SettingValue::text("powershell"), "PowerShell".to_owned())]
        }

        fn default_value(&self, _: SettingKey) -> Option<SettingValue> {
            Some(SettingValue::text("powershell"))
        }
    }

    #[gpui::test]
    fn registered_provider_supplies_options_and_default(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            SettingsSources::init(cx);
            assert!(SettingsSources::options(cx, TERMINAL_DEFAULT_PROFILE).is_empty());
            SettingsSources::register(cx, Rc::new(Fixed));
            assert_eq!(
                SettingsSources::options(cx, TERMINAL_DEFAULT_PROFILE),
                vec![(SettingValue::text("powershell"), "PowerShell".to_owned())]
            );
            assert_eq!(
                SettingsSources::default_value(cx, TERMINAL_DEFAULT_PROFILE)
                    .and_then(|value| value.as_text().map(ToOwned::to_owned)),
                Some("powershell".to_owned())
            );
            assert!(SettingsSources::is_provided(TERMINAL_DEFAULT_PROFILE));
            assert!(!SettingsSources::is_provided(settings::UPDATE_MODE));
            assert_eq!(
                settings::catalog()
                    .definition(TERMINAL_DEFAULT_PROFILE)
                    .map(|definition| definition.value_kind),
                Some(ValueKind::Text)
            );
        });
    }
}
