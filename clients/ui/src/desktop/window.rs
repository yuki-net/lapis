//! Desktop（ネイティブ window）の起動と window options。

use desktop_client::EditorSession;
use gpui::{App, Application};

use crate::app::{DesktopServices, InitialView, open_desktop_window};

/// Runs the desktop event loop with an initial root window and quits after the last window closes.
pub fn run(session: EditorSession, services: DesktopServices, initial_view: InitialView) {
    // 診断ログの tracing subscriber を初期化（Debug は Console 用 store も含む）
    #[cfg(debug_assertions)]
    let _ = crate::devtools::console::init_subscriber();
    // Release でも保存失敗などのエラーを stderr に残す
    #[cfg(not(debug_assertions))]
    crate::app::init_release_tracing();

    Application::new()
        .with_assets(crate::components::IconAssets)
        .run(move |cx: &mut App| {
            let settings = services.settings.settings();
            crate::theme::init(
                &crate::extension_ui::ThemeId::new(settings.theme().to_owned()),
                settings.syntax_theme(),
                cx,
            );
            crate::features::settings_coordinator::SettingsCoordinator::init(
                services.settings.clone(),
                cx,
            );
            if let Some(web_access) = services.web_access.clone() {
                crate::features::settings::install_web_access(web_access, cx);
            }
            #[cfg(all(feature = "hot-reload", debug_assertions))]
            crate::hot_reload::init(cx);
            crate::keymap::gpui::bind_keys(cx);
            #[cfg(debug_assertions)]
            crate::devtools::init(cx);
            open_desktop_window(session, services, initial_view, cx);
            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    let coordinator =
                        crate::features::settings_coordinator::SettingsCoordinator::shared(cx);
                    coordinator.update(cx, |coordinator, cx| coordinator.quit_after_pending(cx));
                }
            })
            .detach();
        });
}
