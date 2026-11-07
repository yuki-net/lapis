//! Web の起動処理（GPUI Web backend と初期 window）。

use desktop_client::EditorSession;
use gpui::Application;

use crate::app::{DesktopServices, InitialView, open_desktop_window};

pub fn run_web(
    session: EditorSession,
    services: DesktopServices,
    initial_view: InitialView,
    fonts: Vec<std::borrow::Cow<'static, [u8]>>,
    backend: &str,
    on_ready: fn(),
) -> gpui::ApplicationHandle {
    gpui_platform::web_init();
    let preference = match backend {
        "webgl" => gpui_web::WebBackendPreference::WebGl,
        "webgpu" => gpui_web::WebBackendPreference::WebGpu,
        _ => gpui_web::WebBackendPreference::Auto,
    };
    let platform = std::rc::Rc::new(gpui_web::WebPlatform::new_with_backend_and_font_fallback(
        false,
        preference,
        gpui_web::CanvasFontFallback::EmojiAndCjk,
    ));
    let http_client = std::sync::Arc::new(platform.fetch_http_client());
    Application::with_platform(platform)
        .with_http_client(http_client)
        .with_assets(crate::components::IconAssets)
        .run_embedded(move |cx| {
            cx.text_system()
                .add_fonts(fonts)
                .expect("Web fonts should load");
            let settings = services.settings.settings();
            crate::theme::init(
                &crate::extension_ui::ThemeId::new(settings.theme),
                settings.syntax_theme,
                cx,
            );
            crate::features::settings_coordinator::SettingsCoordinator::init(
                services.settings.clone(),
                cx,
            );
            crate::keymap::gpui::bind_keys(cx);
            open_desktop_window(session, services, initial_view, cx);
            on_ready();
        })
}
