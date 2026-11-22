//! プラットフォーム別の entry crate が、共通 UI を GPUI Application 上で起動するための境界。
//!
//! window の生成や platform の初期化は呼び出し側が行い、ここでは共通 UI が必要とする
//! フォント・App Global・Keymap・最初の Window を同じ順序で組み立てる。

use std::borrow::Cow;

use desktop_client::EditorSession;
use gpui::App;

use crate::app::{DesktopServices, InitialView};

/// Web の wasm entry が GPUI Application へ渡す同梱フォント。
#[cfg(target_family = "wasm")]
pub fn web_fonts() -> Vec<Cow<'static, [u8]>> {
    let mut fonts: Vec<Cow<'static, [u8]>> = vec![
        Cow::Borrowed(include_bytes!("../assets/IBMPlexSans-Regular.ttf") as &[u8]),
        Cow::Borrowed(include_bytes!("../assets/IBMPlexSans-SemiBold.ttf") as &[u8]),
    ];
    // UI 用の IBM Plex Sans だけではターミナルの罫線・ブロック要素を描けない。
    fonts.extend(crate::bundled_font::fonts());
    fonts
}

/// GPUI Application が使うアイコン等の AssetSource。
pub fn application_assets() -> impl gpui::AssetSource {
    crate::components::IconAssets
}

/// 共通 UI の初期化と最初の Window の生成を行う。
pub fn boot(
    session: EditorSession,
    services: DesktopServices,
    initial_view: InitialView,
    fonts: Vec<Cow<'static, [u8]>>,
    on_ready: fn(),
    cx: &mut App,
) {
    cx.text_system()
        .add_fonts(fonts)
        .expect("起動時にフォントを読み込めません");
    let settings = services.settings.settings();
    crate::theme::init(
        &extension_ui_contract::ThemeId::new(settings.theme().to_owned()),
        settings.syntax_theme(),
        cx,
    );
    crate::features::settings_coordinator::SettingsCoordinator::init(services.settings.clone(), cx);
    crate::keymap::gpui::bind_keys(cx);
    cx.set_global(workbench_shell::ShortcutLabels(Some(
        crate::app::shortcut_label,
    )));
    crate::app::open_desktop_window(session, services, initial_view, cx);
    on_ready();
}
