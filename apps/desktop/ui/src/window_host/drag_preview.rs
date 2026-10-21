use gpui::{App, Context, IntoElement, Render, Window, div, prelude::*, px};

/// Visual-only tab preview used in the source drag overlay and the Windows ghost window.
pub(crate) struct DragPreview {
    label: String,
    visible: bool,
}

impl DragPreview {
    pub(crate) fn new(label: String, visible: bool) -> Self {
        Self { label, visible }
    }
}

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        if !self.visible {
            return div().size(px(0.0));
        }
        div()
            .w(px(260.0))
            .h(px(28.0))
            .flex()
            .items_center()
            .px(crate::tokens::spacing::XS)
            .rounded(crate::tokens::radius::CONTROL)
            .bg(crate::theme::colors().button_background_focused)
            .border_1()
            .border_color(crate::theme::colors().button_border_focused)
            .text_color(crate::theme::colors().text_primary)
            .text_size(crate::tokens::typography::FONT_XS)
            .truncate()
            .child(self.label.clone())
    }
}

/// Opens a short-lived, non-activating native preview on Windows.
#[cfg(target_os = "windows")]
pub(crate) fn open(
    label: &str,
    position: gpui::Point<gpui::Pixels>,
    source: &Window,
    cx: &mut App,
) -> Option<gpui::AnyWindowHandle> {
    let point = crate::window_host::drag::screen_position(position, source)?;
    let bounds = gpui::Bounds::centered(None, gpui::size(px(260.0), px(32.0)), cx);
    let handle = cx
        .open_window(
            gpui::WindowOptions {
                window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
                titlebar: None,
                focus: false,
                show: false,
                kind: gpui::WindowKind::PopUp,
                is_movable: false,
                is_resizable: false,
                is_minimizable: false,
                window_background: gpui::WindowBackgroundAppearance::Transparent,
                ..Default::default()
            },
            |_, cx| cx.new(|_| DragPreview::new(label.to_owned(), true)),
        )
        .ok()?;
    let handle: gpui::AnyWindowHandle = handle.into();
    handle
        .update(cx, |_, window, _| platform::position(window, point))
        .ok()?;
    Some(handle)
}

/// Leaves the in-window GPUI drag preview available on platforms without the native ghost window.
#[cfg(not(target_os = "windows"))]
pub(crate) fn open(
    _: &str,
    _: gpui::Point<gpui::Pixels>,
    _: &Window,
    _: &mut App,
) -> Option<gpui::AnyWindowHandle> {
    None
}

/// Moves the native preview without activating it or taking mouse capture from the source.
pub(crate) fn position(window: &Window, point: crate::window_host::drag::ScreenPoint) {
    platform::position(window, point);
}

/// Hides the preview before resolving the drop target, then schedules its GPUI window for removal.
pub(crate) fn close(window: &mut Window) {
    platform::hide(window);
    window.remove_window();
}

#[cfg(target_os = "windows")]
mod platform {
    use super::*;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{
            SW_HIDE, SWP_NOACTIVATE, SWP_NOZORDER, SWP_SHOWWINDOW, SetWindowPos, ShowWindow,
        },
    };

    fn hwnd(window: &Window) -> Option<HWND> {
        match HasWindowHandle::window_handle(window).ok()?.as_raw() {
            RawWindowHandle::Win32(handle) => Some(HWND(handle.hwnd.get() as *mut _)),
            _ => None,
        }
    }

    pub(super) fn position(window: &Window, point: crate::window_host::drag::ScreenPoint) {
        let Some(hwnd) = hwnd(window) else { return };
        // The cursor stays outside the preview so native hit testing still sees the drop target.
        let _ = unsafe {
            SetWindowPos(
                hwnd,
                None,
                point.x + 14,
                point.y + 18,
                (260.0 * window.scale_factor()).round() as i32,
                (28.0 * window.scale_factor()).round() as i32,
                SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )
        };
    }

    pub(super) fn hide(window: &Window) {
        if let Some(hwnd) = hwnd(window) {
            let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod platform {
    use super::*;

    pub(super) fn position(_: &Window, _: crate::window_host::drag::ScreenPoint) {}
    pub(super) fn hide(_: &Window) {}
}
