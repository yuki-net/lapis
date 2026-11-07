//! Desktop window の初期 bounds と title bar。

use gpui::{App, Bounds, TitlebarOptions, WindowBounds, WindowOptions, size};

/// Returns centered 1440 by 900 logical-pixel bounds with a transparent Lapis title bar.
pub(crate) fn window_options(cx: &App) -> WindowOptions {
    let bounds = Bounds::centered(
        None,
        size(
            crate::tokens::size::DEFAULT_WINDOW_WIDTH,
            crate::tokens::size::DEFAULT_WINDOW_HEIGHT,
        ),
        cx,
    );
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some("Lapis".into()),
            appears_transparent: true,
            ..Default::default()
        }),
        ..Default::default()
    }
}
