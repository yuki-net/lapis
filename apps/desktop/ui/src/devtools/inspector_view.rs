use gpui::{AnyElement, Context, Inspector, IntoElement, Window, div, prelude::*, px};

use super::style_view::detail_row;
use crate::theme;

// 選択・検査状態・対象Windowの管理はGPUIに任せ、ここでは表示だけを担当する。
pub(super) fn render_inspector(
    inspector: &mut Inspector,
    window: &mut Window,
    cx: &mut Context<Inspector>,
) -> AnyElement {
    let selected = inspector.active_element_id().cloned();
    let states = inspector.render_inspector_states(window, cx);
    div()
        .id("lapis-inspector")
        .debug_selector(|| "lapis-inspector".into())
        .size_full()
        .flex()
        .flex_col()
        .border_l_1()
        .border_color(theme::colors().border_default)
        .bg(theme::colors().background_secondary)
        .text_color(theme::colors().text_primary)
        .text_size(px(12.0))
        .child(
            div()
                .flex_none()
                .p_3()
                .border_b_1()
                .border_color(theme::colors().border_default)
                .flex()
                .flex_col()
                .gap_2()
                .child("GPUI Inspector")
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            div()
                                .id("inspector-pick")
                                .debug_selector(|| "inspector-pick".into())
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .cursor_pointer()
                                .bg(if inspector.is_picking() {
                                    theme::colors().button_background_selected
                                } else {
                                    theme::colors().background_tertiary
                                })
                                .hover(|s| s.bg(theme::colors().button_background_hover))
                                .child("Pick element")
                                .on_click(cx.listener(|inspector, _, window, cx| {
                                    inspector.start_picking();
                                    window.refresh();
                                    cx.notify();
                                })),
                        )
                        .child(
                            div()
                                .id("inspector-close")
                                .debug_selector(|| "inspector-close".into())
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .cursor_pointer()
                                .hover(|s| s.bg(theme::colors().button_background_hover))
                                .child("Close")
                                .on_click(|_, window, cx| window.toggle_inspector(cx)),
                        ),
                )
                .child(div().text_color(theme::colors().text_secondary).child(
                    if inspector.is_picking() {
                        "対象をクリックして選択 / ホイールで深さ変更"
                    } else {
                        "Pick elementで再選択"
                    },
                )),
        )
        .child(
            div()
                .id("inspector-details")
                .min_h(px(0.0))
                .flex_1()
                .overflow_scroll()
                .child(
                    div()
                        .p_3()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .when_some(selected, |element, id| {
                            let source = id.path.source_location;
                            element
                                .child(detail_row("GlobalElementId", id.path.global_id.to_string()))
                                .child(detail_row("instance ID", id.instance_id.to_string()))
                                .child(detail_row(
                                    "生成元",
                                    format!(
                                        "{}:{}:{}",
                                        source.file(),
                                        source.line(),
                                        source.column()
                                    ),
                                ))
                        })
                        .when(states.is_empty(), |element| {
                            element.child("Pickで検査可能な要素を選択してください")
                        })
                        .children(states),
                ),
        )
        .into_any_element()
}
