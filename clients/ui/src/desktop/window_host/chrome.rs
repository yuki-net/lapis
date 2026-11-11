//! Root・Detached・Inspectorが共有するWindow chrome。タブや終了判断は呼出側が所有する。
use crate::{theme, tokens};
use gpui::{App, ClickEvent, Div, IntoElement, Window, div, prelude::*};

#[cfg(any(target_os = "windows", target_os = "linux"))]
use gpui::WindowControlArea;
#[cfg(target_os = "macos")]
use gpui::px;

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
        .bg(theme::colors(cx).background_primary);
    // 左・content・右の組み立ては共通に保ち、両端への追加と属性付けだけをプラットフォーム別関数へ分ける。
    platform_title_bar(region, content, close, cx)
}

/// macOS のネイティブ traffic lights がタイトルバー左端に占有する幅。
/// 現行の見た目を維持するため値を変えない（見直しは Mac 実機での確認が必要）。
#[cfg(target_os = "macos")]
const MACOS_TRAFFIC_LIGHTS_INSET: gpui::Pixels = px(80.);

/// macOS 用。ネイティブ traffic lights の左 spacer だけを足す。Windows / Linux / Web では使わない。
#[cfg(target_os = "macos")]
fn platform_title_bar(
    region: Div,
    content: impl IntoElement,
    _close: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    _cx: &gpui::App,
) -> Div {
    region
        .child(div().w(MACOS_TRAFFIC_LIGHTS_INSET).flex_shrink_0())
        .child(content)
}

/// Windows 用。自前のウィンドウボタンと Drag 領域を足す。macOS / Linux / Web には出さない。
#[cfg(target_os = "windows")]
fn platform_title_bar(
    region: Div,
    content: impl IntoElement,
    close: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &gpui::App,
) -> Div {
    region
        .child(content)
        .child(controls::window_controls(close, cx))
        .window_control_area(WindowControlArea::Drag)
}

/// Linux 用。自前のウィンドウボタンと start_window_move を足す。macOS / Windows / Web には出さない。
#[cfg(target_os = "linux")]
fn platform_title_bar(
    region: Div,
    content: impl IntoElement,
    close: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &gpui::App,
) -> Div {
    region
        .child(content)
        .child(controls::window_controls(close, cx))
        .on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
            window.start_window_move()
        })
}

/// Web（wasm）とその他の OS 用。ブラウザー側の chrome がウィンドウを提供するため、意図的に何もしない。
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn platform_title_bar(
    region: Div,
    content: impl IntoElement,
    _close: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    _cx: &gpui::App,
) -> Div {
    region.child(content)
}

/// Windows / Linux 用の自前ウィンドウボタン。macOS はネイティブ traffic lights、Web はブラウザーの
/// chrome がウィンドウを提供するため、このモジュールは Windows / Linux のビルドでだけコンパイルする。
#[cfg(any(target_os = "windows", target_os = "linux"))]
mod controls {
    use super::*;

    #[derive(Clone, Copy)]
    enum WindowControlIconName {
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
    struct WindowControlIcon {
        name: WindowControlIconName,
    }

    impl WindowControlIcon {
        const fn new(name: WindowControlIconName) -> Self {
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

    pub(super) fn window_controls(
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
}
