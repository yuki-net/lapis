//! Web の起動処理（GPUI Web backend と初期 window）。

use desktop_client::EditorSession;
use desktop_ui::{DesktopServices, InitialView};
use gpui::Application;

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
        .with_assets(desktop_ui::application_assets())
        .run_embedded(move |cx| {
            desktop_ui::boot(session, services, initial_view, fonts, on_ready, cx);
        })
}
