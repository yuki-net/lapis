//! GPUI の capture を維持したまま、離した位置を Window / Panel に解決する。
use super::*;
use crate::shell::DraggedPanelTab;

/// 画面全体の物理座標。異なる DPI の Window 間で論理座標を混ぜない。
#[derive(Clone, Copy, Debug)]
pub(crate) struct ScreenPoint {
    pub x: i32,
    pub y: i32,
}

impl DesktopRoot {
    /// Allows moves only for a document owned by a live host when the group is not closing
    /// and no member has pending worker operations or draft recovery.
    pub(super) fn can_move_tab(&self, host: HostId, tab: &PanelTab, cx: &gpui::App) -> bool {
        let group = self.group.read(cx);
        !group.closing
            && matches!(tab, PanelTab::Document(_))
            && group
                .layout
                .tabs
                .get(tab)
                .is_some_and(|(owner, _)| *owner == host)
            && group.members.contains_key(&host)
            && !group
                .members
                .values()
                .any(|member| member.editor.read(cx).has_pending())
    }

    /// Resolves a drag release at a physical screen point.
    /// On Windows, moves eligible tabs into a panel in this group or detaches over
    /// external windows and desktop space. Releases over the source, another group,
    /// or a non-panel Lapis region leave placement unchanged. Other platforms
    /// cannot resolve native targets and detach eligible tabs if called directly.
    pub(super) fn release_tab(
        &mut self,
        host: HostId,
        drag: DraggedPanelTab,
        point: ScreenPoint,
        source_window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if drag.source_host != host
            || !drag.belongs_to(Some(self.group.entity_id()))
            || !self.can_move_tab(host, &drag.tab, cx)
        {
            return;
        }
        // WindowFromPoint を使い、背後の Lapis や別グループへ落とさない。
        // 外部アプリ上は新 Window、別 Lapis Window の非 Panel 領域は取消。
        let mut lapis_target = false;
        let mut target = None;
        for handle in cx.windows() {
            if handle == source_window.window_handle() {
                if is_target(source_window, point) {
                    return;
                }
                continue;
            }
            let hit = handle
                .update(cx, |_, window, _| {
                    is_target(window, point).then(|| client_position(window, point))
                })
                .ok()
                .flatten();
            if let Some(position) = hit {
                lapis_target = true;
                target = position.and_then(|position| {
                    self.group
                        .read(cx)
                        .members
                        .iter()
                        .find_map(|(host, member)| {
                            if member.handle != handle {
                                return None;
                            }
                            let shell = member.shell.read(cx);
                            shell
                                .group_bounds
                                .borrow()
                                .iter()
                                .find(|(_, bounds)| bounds.contains(&position))
                                .map(|(group, _)| (*host, group.clone()))
                        })
                });
                break;
            }
        }
        if let Some((target, group)) = target {
            let handle = self.group.read(cx).members[&target].handle;
            self.transfer_tab_to_group(host, target, group, drag.tab, cx);
            let editor = self.group.read(cx).members[&target].editor.clone();
            let _ = handle.update(cx, |_, window, cx| {
                window.activate_window();
                window.focus(&editor.focus_handle(cx));
            });
        } else if !lapis_target {
            self.detach_tab_at(host, drag.tab, Some(point), cx);
        }
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::*;
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
    pub(crate) fn screen_position(
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
    pub(super) fn client_position(
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
    pub(crate) fn position_window(window: &Window, point: ScreenPoint) {
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
    pub(super) fn is_target(window: &Window, point: ScreenPoint) -> bool {
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
    use super::*;
    /// Returns none because screen-coordinate conversion is unavailable on this platform.
    pub(crate) fn screen_position(_: gpui::Point<gpui::Pixels>, _: &Window) -> Option<ScreenPoint> {
        None
    }
    /// Returns none because native client-coordinate conversion is unavailable on this platform.
    pub(super) fn client_position(_: &Window, _: ScreenPoint) -> Option<gpui::Point<gpui::Pixels>> {
        None
    }
    /// Returns false because native window hit testing is unavailable on this platform.
    pub(super) fn is_target(_: &Window, _: ScreenPoint) -> bool {
        false
    }
    /// Leaves window placement unchanged on this platform.
    pub(crate) fn position_window(_: &Window, _: ScreenPoint) {}
}

pub(crate) use platform::screen_position;
use platform::{client_position, is_target};

/// Tests whether the source window is frontmost at a physical screen point; returns false outside Windows.
pub(crate) fn is_source_window(window: &Window, point: ScreenPoint) -> bool {
    is_target(window, point)
}

pub(super) use platform::position_window;
