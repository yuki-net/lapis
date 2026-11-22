use gpui::{App, FocusHandle, Pixels, Point, ScrollHandle, ShapedLine, Size, Window};

/// NativeとWeb版GPUIのfocus API差を吸収する。
pub fn focus(window: &mut Window, handle: &FocusHandle, cx: &mut App) {
    #[cfg(not(target_family = "wasm"))]
    {
        let _ = cx;
        window.focus(handle);
    }
    #[cfg(target_family = "wasm")]
    window.focus(handle, cx);
}

pub fn max_offset(handle: &ScrollHandle) -> Size<Pixels> {
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

pub fn paint_line(
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

#[cfg(not(target_family = "wasm"))]
type PopupAnchor = gpui::Corner;
#[cfg(target_family = "wasm")]
type PopupAnchor = gpui::Anchor;

pub fn popup_anchor(end: bool) -> PopupAnchor {
    if end {
        PopupAnchor::TopRight
    } else {
        PopupAnchor::TopLeft
    }
}
