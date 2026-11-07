//! Root・Detached・Inspectorが共有するWindow chrome。タブや終了判断は呼出側が所有する。
use crate::{theme, tokens};
use gpui::{App, ClickEvent, Div, IntoElement, Window, WindowControlArea, div, prelude::*, px};

#[derive(Clone, Copy)]
pub(crate) enum WindowControlIconName {
    Minimize,
    Maximize,
    Close,
}

impl WindowControlIconName {
    fn icon_name(self) -> crate::components::IconName {
        match self {
            Self::Minimize => crate::components::IconName::Minus,
            Self::Maximize => crate::components::IconName::Square,
            Self::Close => crate::components::IconName::X,
        }
    }
}

/// ネイティブのウィンドウ操作ボタン用 Lucide アイコン。
///
/// 色は共通の `Icon` コンポーネントが有効テーマから解決する。
#[derive(IntoElement)]
pub(crate) struct WindowControlIcon {
    name: WindowControlIconName,
}

impl WindowControlIcon {
    pub(crate) const fn new(name: WindowControlIconName) -> Self {
        Self { name }
    }
}

impl gpui::RenderOnce for WindowControlIcon {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        div()
            .size_4()
            .flex()
            .items_center()
            .justify_center()
            .text_color(theme::colors(cx).text_secondary)
            .child(crate::components::Icon::new(self.name.icon_name()))
    }
}

pub(crate) fn title_bar(
    content: impl IntoElement,
    close: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &gpui::App,
) -> Div {
    let region = div()
        .h(tokens::size::TITLE_BAR_HEIGHT)
        .w_full()
        .flex_shrink_0()
        .flex()
        .items_center()
        .bg(theme::colors(cx).background_primary)
        .when(cfg!(target_os = "macos"), |el| {
            el.child(div().w(px(80.)).flex_shrink_0())
        })
        .child(content)
        .when(!cfg!(target_os = "macos"), |el| {
            el.child(window_controls(close, cx))
        });
    #[cfg(target_os = "windows")]
    let region = region.window_control_area(WindowControlArea::Drag);
    #[cfg(target_os = "linux")]
    let region = region.on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
        window.start_window_move()
    });
    region
}

fn window_controls(
    close: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &gpui::App,
) -> Div {
    div()
        .h_full()
        .w(tokens::size::WINDOW_CONTROL_WIDTH * 3.)
        .flex_shrink_0()
        .flex()
        .items_center()
        .child(
            window_control_button(
                "window-minimize",
                WindowControlIconName::Minimize,
                WindowControlArea::Min,
                false,
                cx,
            )
            .on_click(|_, window, _| window.minimize_window()),
        )
        .child(
            window_control_button(
                "window-maximize",
                WindowControlIconName::Maximize,
                WindowControlArea::Max,
                false,
                cx,
            )
            .on_click(|_, window, _| window.zoom_window()),
        )
        .child(
            window_control_button(
                "window-close",
                WindowControlIconName::Close,
                WindowControlArea::Close,
                true,
                cx,
            )
            .on_click(close),
        )
}

fn window_control_button(
    id: &'static str,
    icon: WindowControlIconName,
    area: WindowControlArea,
    close: bool,
    cx: &gpui::App,
) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .h_full()
        .w(tokens::size::WINDOW_CONTROL_WIDTH)
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .occlude()
        .window_control_area(area)
        .text_color(theme::colors(cx).text_secondary)
        .hover(move |style| {
            if close {
                style
                    .bg(theme::colors(cx).danger_background)
                    .text_color(theme::colors(cx).text_primary)
            } else {
                style
                    .bg(theme::colors(cx).button_background_hover)
                    .text_color(theme::colors(cx).text_primary)
            }
        })
        .active(|style| style.bg(theme::colors(cx).button_background_selected))
        .child(WindowControlIcon::new(icon))
}
