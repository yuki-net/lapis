use crate::features::id;
use extension_ui_contract::{
    ActivationCondition, FeatureDescriptor, UiContribution, UiSlot, ViewId,
};

mod canvas;
mod input;
mod state;
mod view;

pub(crate) use state::{TerminalFeature, TerminalFeatureEvent};

/// 設定 UI が既定プロファイルを選ぶための、Host が公開した profile 一覧。
///
/// Terminal Feature が Host から取得した内容をここへ写し、Settings は
/// Feature の entity に依存せずこの一覧だけを読む。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct TerminalProfileCatalog {
    pub(crate) profiles: Vec<(String, String)>,
    pub(crate) default: Option<String>,
}

impl gpui::Global for TerminalProfileCatalog {}

/// 公開済みの profile 一覧。未取得なら空。
pub(crate) fn profile_catalog(cx: &gpui::App) -> TerminalProfileCatalog {
    cx.try_global::<TerminalProfileCatalog>()
        .cloned()
        .unwrap_or_default()
}

/// Host から取得した profile 一覧を Settings へ公開する。
pub(super) fn publish_profile_catalog(profiles: &terminal::TerminalProfiles, cx: &mut gpui::App) {
    let catalog = TerminalProfileCatalog {
        profiles: profiles
            .profiles
            .iter()
            .map(|profile| (profile.id.as_str().to_owned(), profile.name.clone()))
            .collect(),
        default: profiles.default.as_ref().map(|id| id.as_str().to_owned()),
    };
    if cx.try_global::<TerminalProfileCatalog>() != Some(&catalog) {
        cx.set_global(catalog);
    }
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
        .feature_owned_scroll(),
    )
    .requires("workspace.process")
}
