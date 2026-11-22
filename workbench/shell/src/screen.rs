//! Physical screen coordinates and native window hit testing.

use crate::ScreenPoint;
use gpui::Window;

#[cfg(target_os = "windows")]
mod platform {
    use crate::ScreenPoint;
    use gpui::Window;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::{
        Foundation::{HWND, POINT},
        Graphics::Gdi::{
            ClientToScreen, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO,
            MonitorFromPoint, ScreenToClient,
        },
        UI::{
            HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
            WindowsAndMessaging::{
                GA_ROOT, GetAncestor, SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos, WindowFromPoint,
            },
        },
    };

    /// Returns the native Win32 handle, or none if unavailable or of another platform type.
    fn hwnd(window: &Window) -> Option<HWND> {
        match HasWindowHandle::window_handle(window).ok()?.as_raw() {
            RawWindowHandle::Win32(handle) => Some(HWND(handle.hwnd.get() as *mut _)),
            _ => None,
        }
    }

    /// Converts logical client pixels to physical screen pixels, or none if handle lookup or conversion fails.
    pub fn screen_position(
        position: gpui::Point<gpui::Pixels>,
        window: &Window,
    ) -> Option<ScreenPoint> {
        let mut point = POINT {
            x: (f32::from(position.x) * window.scale_factor()).round() as i32,
            y: (f32::from(position.y) * window.scale_factor()).round() as i32,
        };
        // 有効な Window handle と、呼出中に生存する POINT のみを渡す。
        unsafe { ClientToScreen(hwnd(window)?, &mut point) }
            .as_bool()
            .then_some(ScreenPoint {
                x: point.x,
                y: point.y,
            })
    }

    /// Converts physical screen pixels to logical client pixels, or none if handle lookup or conversion fails.
    pub fn client_position(
        window: &Window,
        point: ScreenPoint,
    ) -> Option<gpui::Point<gpui::Pixels>> {
        let mut point = POINT {
            x: point.x,
            y: point.y,
        };
        unsafe { ScreenToClient(hwnd(window)?, &mut point) }
            .as_bool()
            .then(|| {
                gpui::point(
                    gpui::px(point.x as f32 / window.scale_factor()),
                    gpui::px(point.y as f32 / window.scale_factor()),
                )
            })
    }

    /// Places a nominal 1000 by 700 logical-pixel window near the physical screen point.
    /// Clamps size and position to the nearest monitor's work area, using 96 DPI if
    /// DPI lookup fails. Missing handles, monitor information, or placement errors are ignored.
    pub fn position_window(window: &Window, point: ScreenPoint) {
        let Some(handle) = hwnd(window) else {
            return;
        };
        let monitor = unsafe {
            MonitorFromPoint(
                POINT {
                    x: point.x,
                    y: point.y,
                },
                MONITOR_DEFAULTTONEAREST,
            )
        };
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
            return;
        }
        let (mut dpi_x, mut dpi_y) = (96, 96);
        let _ = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
        let scale = dpi_x as f32 / 96.;
        let work = info.rcWork;
        let width = ((1000. * scale) as i32).min(work.right - work.left);
        let height = ((700. * scale) as i32).min(work.bottom - work.top);
        let x = (point.x - (80. * scale) as i32).clamp(work.left, work.right - width);
        let y = (point.y - (50. * scale) as i32).clamp(work.top, work.bottom - height);
        let _ = unsafe {
            SetWindowPos(
                handle,
                None,
                x,
                y,
                width,
                height,
                SWP_NOZORDER | SWP_NOACTIVATE,
            )
        };
    }

    /// Tests whether this window is the frontmost native root at the physical screen point.
    /// Returns false if its native handle is unavailable.
    pub fn is_target(window: &Window, point: ScreenPoint) -> bool {
        let Some(handle) = hwnd(window) else {
            return false;
        };
        unsafe {
            GetAncestor(
                WindowFromPoint(POINT {
                    x: point.x,
                    y: point.y,
                }),
                GA_ROOT,
            ) == handle
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod platform {
    use crate::ScreenPoint;
    use gpui::Window;
    /// Returns none because screen-coordinate conversion is unavailable on this platform.
    pub fn screen_position(_: gpui::Point<gpui::Pixels>, _: &Window) -> Option<ScreenPoint> {
        None
    }
    /// Returns none because native client-coordinate conversion is unavailable on this platform.
    pub fn client_position(_: &Window, _: ScreenPoint) -> Option<gpui::Point<gpui::Pixels>> {
        None
    }
    /// Returns false because native window hit testing is unavailable on this platform.
    pub fn is_target(_: &Window, _: ScreenPoint) -> bool {
        false
    }
    /// Leaves window placement unchanged on this platform.
    pub fn position_window(_: &Window, _: ScreenPoint) {}
}

pub use platform::{client_position, is_target, position_window, screen_position};

/// Tests whether the source window is frontmost at a physical screen point; returns false outside Windows.
pub fn is_source_window(window: &Window, point: ScreenPoint) -> bool {
    is_target(window, point)
}
