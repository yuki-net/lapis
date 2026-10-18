//! Desktopの0.2.2とWeb previewのAPI差分だけを吸収する。
use gpui::{App, FocusHandle, Pixels, Point, ScrollHandle, ShapedLine, Size, Window};

pub(crate) fn focus(window: &mut Window, handle: &FocusHandle, _cx: &mut App) {
    #[cfg(not(target_family = "wasm"))]
    window.focus(handle);
    #[cfg(target_family = "wasm")]
    window.focus(handle, _cx);
}

pub(crate) fn max_offset(handle: &ScrollHandle) -> Size<Pixels> {
    let offset = handle.max_offset();
    #[cfg(not(target_family = "wasm"))]
    {
        offset
    }
    #[cfg(target_family = "wasm")]
    {
        gpui::size(offset.x, offset.y)
    }
}

pub(crate) fn paint_line(
    line: &ShapedLine,
    origin: Point<Pixels>,
    height: Pixels,
    window: &mut Window,
    cx: &mut App,
) {
    #[cfg(not(target_family = "wasm"))]
    let _ = line.paint(origin, height, window, cx);
    #[cfg(target_family = "wasm")]
    let _ = line.paint(origin, height, gpui::TextAlign::Left, None, window, cx);
}
