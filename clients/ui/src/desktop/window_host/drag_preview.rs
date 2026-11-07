use gpui::{App, Context, IntoElement, Render, Window, div, prelude::*, px};

/// Visual-only tab preview used in the source drag overlay and the Windows ghost window.
pub(crate) struct DragPreview {
    label: String,
    visible: bool,
    icon: Option<crate::components::FileIconId>,
    pinned: bool,
    size: gpui::Size<gpui::Pixels>,
    offset: gpui::Point<gpui::Pixels>,
}

impl DragPreview {
    pub(crate) fn new(
        label: String,
        icon: Option<crate::components::FileIconId>,
        pinned: bool,
        size: gpui::Size<gpui::Pixels>,
        offset: gpui::Point<gpui::Pixels>,
        visible: bool,
    ) -> Self {
        Self {
            label,
            icon,
            pinned,
            size,
            offset,
            visible,
        }
    }
}

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.visible {
            return div().size(px(0.0));
        }
        div()
            .relative()
            .left(self.offset.x)
            .top(self.offset.y)
            .w(self.size.width)
            .h(self.size.height)
            .flex()
            .items_center()
            .px(px(4.0))
            .gap(px(3.0))
            .rounded(crate::tokens::radius::CONTROL)
            .bg(crate::theme::colors(cx).button_background_focused)
            .border_1()
            .border_color(crate::theme::colors(cx).button_border_focused)
            .text_color(crate::theme::colors(cx).text_primary)
            .text_size(crate::tokens::typography::FONT_SM)
            .when_some(self.icon, |tab, icon| {
                tab.child(crate::components::FileIcon::new(icon).size(px(12.)))
            })
            .child(
                div()
                    .min_w(px(0.))
                    .flex_1()
                    .truncate()
                    .child(self.label.clone()),
            )
            .when(!self.pinned, |tab| {
                tab.child(crate::components::Icon::new(crate::components::IconName::X).size(px(9.)))
            })
    }
}

/// Opens a short-lived, non-activating native preview on Windows.
#[cfg(target_os = "windows")]
pub(crate) fn open(
    label: &str,
    icon: Option<crate::components::FileIconId>,
    pinned: bool,
    size: gpui::Size<gpui::Pixels>,
    position: gpui::Point<gpui::Pixels>,
    source: &Window,
    cx: &mut App,
) -> Option<gpui::AnyWindowHandle> {
    let point = crate::window_host::drag::screen_position(position, source)?;
    let bounds = gpui::Bounds::centered(None, size, cx);
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
            |_, cx| {
                cx.new(|_| {
                    DragPreview::new(
                        label.to_owned(),
                        icon,
                        pinned,
                        size,
                        gpui::point(px(0.), px(0.)),
                        true,
                    )
                })
            },
        )
        .ok()?;
    let handle: gpui::AnyWindowHandle = handle.into();
    handle
        .update(cx, |_, window, _| {
            platform::disable_input(window);
            platform::show(window, point, size)
        })
        .ok()?;
    Some(handle)
}

/// Leaves the in-window GPUI drag preview available on platforms without the native ghost window.
#[cfg(not(target_os = "windows"))]
pub(crate) fn open(
    _: &str,
    _: Option<crate::components::FileIconId>,
    _: bool,
    _: gpui::Size<gpui::Pixels>,
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
        UI::Input::KeyboardAndMouse::EnableWindow,
        UI::WindowsAndMessaging::{
            SW_HIDE, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SetWindowPos,
            ShowWindow,
        },
    };

    fn hwnd(window: &Window) -> Option<HWND> {
        match HasWindowHandle::window_handle(window).ok()?.as_raw() {
            RawWindowHandle::Win32(handle) => Some(HWND(handle.hwnd.get() as *mut _)),
            _ => None,
        }
    }

    pub(super) fn disable_input(window: &Window) {
        if let Some(hwnd) = hwnd(window) {
            let _ = unsafe { EnableWindow(hwnd, false) };
        }
    }

    pub(super) fn position(window: &Window, point: crate::window_host::drag::ScreenPoint) {
        let Some(hwnd) = hwnd(window) else { return };
        let size = window.viewport_size();
        let width = (f32::from(size.width) * window.scale_factor()).round() as i32;
        let height = (f32::from(size.height) * window.scale_factor()).round() as i32;
        // 入力を無効化し、移動先をカーソル下で判定する。
        let _ = unsafe {
            SetWindowPos(
                hwnd,
                None,
                point.x - width / 2,
                point.y - height / 2,
                0,
                0,
                SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOSIZE | SWP_SHOWWINDOW,
            )
        };
    }

    pub(super) fn show(
        window: &Window,
        point: crate::window_host::drag::ScreenPoint,
        size: gpui::Size<gpui::Pixels>,
    ) {
        let Some(hwnd) = hwnd(window) else { return };
        let width = (f32::from(size.width) * window.scale_factor()).round() as i32;
        let height = (f32::from(size.height) * window.scale_factor()).round() as i32;
        // 非表示で作成したウィンドウは、最初の表示時にサイズも指定する。
        let _ = unsafe {
            SetWindowPos(
                hwnd,
                None,
                point.x - width / 2,
                point.y - height / 2,
                width,
                height,
                initial_show_flags(),
            )
        };
    }

    fn initial_show_flags() -> windows::Win32::UI::WindowsAndMessaging::SET_WINDOW_POS_FLAGS {
        SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW
    }

    pub(super) fn hide(window: &Window) {
        if let Some(hwnd) = hwnd(window) {
            let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn first_show_applies_the_tab_size() {
            let flags = initial_show_flags();
            assert_eq!(flags.0 & SWP_NOSIZE.0, 0);
            assert_ne!(flags.0 & SWP_NOACTIVATE.0, 0);
            assert_ne!(flags.0 & SWP_SHOWWINDOW.0, 0);
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod platform {
    use super::*;

    pub(super) fn position(_: &Window, _: crate::window_host::drag::ScreenPoint) {}
    pub(super) fn hide(_: &Window) {}
}
