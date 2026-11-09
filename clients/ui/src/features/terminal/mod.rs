use crate::{
    extension_ui::{ActivationCondition, FeatureDescriptor, UiContribution, UiSlot, ViewId},
    features::id,
};

mod input;
mod state;
mod view;

pub(crate) use state::{TerminalFeature, TerminalFeatureEvent};

/// OS ごとの既定等幅フォント。
pub(crate) fn default_font_family() -> &'static str {
    #[cfg(windows)]
    const DEFAULT: &str = "Cascadia Mono";
    #[cfg(target_os = "macos")]
    const DEFAULT: &str = "Menlo";
    #[cfg(not(any(windows, target_os = "macos")))]
    const DEFAULT: &str = "monospace";
    DEFAULT
}

pub(crate) fn default_font_size() -> u8 {
    13
}

/// 設定 UI に出す等幅フォントの候補。
pub(crate) fn font_choices() -> &'static [&'static str] {
    #[cfg(windows)]
    const FONTS: &[&str] = &["Cascadia Mono", "Consolas", "Courier New"];
    #[cfg(target_os = "macos")]
    const FONTS: &[&str] = &["Menlo", "Monaco", "SF Mono", "Courier New"];
    #[cfg(not(any(windows, target_os = "macos")))]
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
        .feature_owned_scroll(),
    )
    .requires("workspace.process")
}
