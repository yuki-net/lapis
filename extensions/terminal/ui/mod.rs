use crate::features::id;
use extension_ui_contract::{
    ActivationCondition, FeatureDescriptor, UiContribution, UiSlot, ViewId,
};
use settings::{SettingKey, SettingValue, SettingsSourceProvider};
use std::cell::RefCell;
use std::rc::Rc;

mod canvas;
mod input;
mod lifecycle;
mod state;
mod view;

pub(crate) use lifecycle::{
    TerminalContext, TerminalLifecycle, TerminalTabSource, terminal_lifecycle,
};
pub(crate) use state::{TerminalFeature, TerminalFeatureEvent};

/// 設定 UI が既定プロファイルを選ぶための、Host が公開した profile 一覧。
///
/// Terminal Feature が Host から取得した内容をここへ写し、設定の供給元として
/// [`settings::SettingsSourceProvider`] 経由で公開する。設定画面は Feature の
/// entity に依存せず、登録された供給元だけを読む。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct TerminalProfileCatalog {
    pub(crate) profiles: Vec<(String, String)>,
    pub(crate) default: Option<String>,
}

/// Terminal が供給する設定の供給元。
///
/// 保存値ではなく実行時の状態（Host の profile 一覧・既定の描画設定）から
/// 候補と既定値を返す。状態は Feature が所有し、この型は読み取りだけを行う。
pub(crate) struct TerminalSettingsSource {
    profiles: Rc<RefCell<TerminalProfileCatalog>>,
}

impl SettingsSourceProvider for TerminalSettingsSource {
    fn keys(&self) -> &'static [SettingKey] {
        &[
            settings::TERMINAL_FONT_FAMILY,
            settings::TERMINAL_FONT_SIZE,
            settings::TERMINAL_LINE_HEIGHT,
            settings::TERMINAL_DEFAULT_PROFILE,
        ]
    }

    fn options(&self, key: SettingKey) -> Vec<(SettingValue, String)> {
        match key {
            settings::TERMINAL_FONT_FAMILY => font_choices()
                .iter()
                .map(|family| (SettingValue::text(*family), (*family).to_owned()))
                .collect(),
            settings::TERMINAL_FONT_SIZE => font_size_choices()
                .iter()
                .map(|size| (SettingValue::integer(i64::from(*size)), size.to_string()))
                .collect(),
            settings::TERMINAL_LINE_HEIGHT => line_height_choices()
                .iter()
                .map(|percent| {
                    (
                        SettingValue::integer(i64::from(*percent)),
                        format!("{percent}%"),
                    )
                })
                .collect(),
            settings::TERMINAL_DEFAULT_PROFILE => self
                .profiles
                .borrow()
                .profiles
                .iter()
                .map(|(id, name)| (SettingValue::text(id), name.clone()))
                .collect(),
            _ => Vec::new(),
        }
    }

    fn default_value(&self, key: SettingKey) -> Option<SettingValue> {
        match key {
            settings::TERMINAL_FONT_FAMILY => Some(SettingValue::text(default_font_family())),
            settings::TERMINAL_FONT_SIZE => {
                Some(SettingValue::integer(i64::from(default_font_size())))
            }
            settings::TERMINAL_LINE_HEIGHT => Some(SettingValue::integer(i64::from(
                default_line_height_percent(),
            ))),
            settings::TERMINAL_DEFAULT_PROFILE => self
                .profiles
                .borrow()
                .default
                .clone()
                .map(SettingValue::text),
            _ => None,
        }
    }
}

/// Terminal を設定の供給元として登録する。設定画面より先に1回呼ぶ。
///
/// 返すハンドルは Feature が profile 一覧を書き込むために保持する。
pub(crate) fn register_settings_source(cx: &mut gpui::App) -> Rc<RefCell<TerminalProfileCatalog>> {
    let profiles = Rc::new(RefCell::new(TerminalProfileCatalog::default()));
    crate::features::settings_sources::SettingsSources::register(
        cx,
        Rc::new(TerminalSettingsSource {
            profiles: profiles.clone(),
        }),
    );
    profiles
}

/// テスト用: profile 一覧を供給元へ公開する。
#[cfg(test)]
pub(crate) fn publish_test_profile_catalog(
    profiles: &terminal::TerminalProfiles,
    catalog: &Rc<RefCell<TerminalProfileCatalog>>,
    cx: &mut gpui::App,
) {
    publish_profile_catalog(profiles, catalog, cx);
}

/// Host から取得した profile 一覧を設定の供給元へ公開する。
///
/// 内容が変わったときだけ登録し直し、設定画面へ通知する。
pub(super) fn publish_profile_catalog(
    profiles: &terminal::TerminalProfiles,
    catalog: &Rc<RefCell<TerminalProfileCatalog>>,
    cx: &mut gpui::App,
) {
    let next = TerminalProfileCatalog {
        profiles: profiles
            .profiles
            .iter()
            .map(|profile| (profile.id.as_str().to_owned(), profile.name.clone()))
            .collect(),
        default: profiles.default.as_ref().map(|id| id.as_str().to_owned()),
    };
    if *catalog.borrow() == next {
        return;
    }
    *catalog.borrow_mut() = next;
    crate::features::settings_sources::SettingsSources::register(
        cx,
        Rc::new(TerminalSettingsSource {
            profiles: catalog.clone(),
        }),
    );
}

/// OS ごとの既定等幅フォント。
///
/// Web はブラウザーや OS のフォントを引けず、同梱フォントだけが使える。
/// `monospace` のような総称名は解決できないため、Web だけは同梱した family 名を返す。
pub(crate) fn default_font_family() -> &'static str {
    #[cfg(target_family = "wasm")]
    const DEFAULT: &str = crate::bundled_font::FAMILY;
    #[cfg(windows)]
    const DEFAULT: &str = "Cascadia Mono";
    #[cfg(target_os = "macos")]
    const DEFAULT: &str = "Menlo";
    #[cfg(not(any(target_family = "wasm", windows, target_os = "macos")))]
    const DEFAULT: &str = "monospace";
    DEFAULT
}

pub(crate) fn default_font_size() -> u8 {
    13
}

/// 行送りの既定値（フォントサイズに対する百分率）。
pub(crate) fn default_line_height_percent() -> u8 {
    125
}

pub(crate) fn line_height_choices() -> &'static [u8] {
    &[100, 110, 120, 125, 140, 150]
}

/// 設定 UI に出す等幅フォントの候補。
///
/// Web は同梱フォントしか使えない。読み込んでいない family を候補に出すと
/// 選んだ瞬間に別フォントへ落ちるため、Web は同梱 family だけを候補にする。
pub(crate) fn font_choices() -> &'static [&'static str] {
    #[cfg(target_family = "wasm")]
    const FONTS: &[&str] = &[crate::bundled_font::FAMILY];
    #[cfg(windows)]
    const FONTS: &[&str] = &["Cascadia Mono", "Consolas", "Courier New"];
    #[cfg(target_os = "macos")]
    const FONTS: &[&str] = &["Menlo", "Monaco", "SF Mono", "Courier New"];
    #[cfg(not(any(target_family = "wasm", windows, target_os = "macos")))]
    const FONTS: &[&str] = &[
        "monospace",
        "DejaVu Sans Mono",
        "Liberation Mono",
        "Noto Sans Mono",
    ];
    FONTS
}

pub(crate) fn font_size_choices() -> &'static [u8] {
    &[11, 12, 13, 14, 16, 18]
}

pub(super) fn descriptor() -> FeatureDescriptor {
    FeatureDescriptor::bundled(
        id::FEATURE_TERMINAL,
        [
            ActivationCondition::OnView(ViewId::new(id::VIEW_TERMINAL)),
            ActivationCondition::OnCommand(id::COMMAND_START_TERMINAL.into()),
        ],
    )
    .contributes(
        UiContribution::view(
            id::VIEW_TERMINAL,
            UiSlot::BottomDock,
            "view.terminal",
            "terminal",
            10,
        )
        // タブごとに独立したセッションを持つ。開くたびに新しい個体を作る。
        .multiple_instances()
        // タブは Host の Session（外部 Resource）を参照する。Session の開始と
        // 終了は Terminal Feature が所有し、Layout は ID を保持するだけ。
        .references_external_resource()
        .feature_owned_scroll(),
    )
    .requires("workspace.process")
}
