use gpui::{AnyElement, Context, Inspector, IntoElement, Window, div, prelude::*, px};

use crate::{
    components::{ScrollAxis, ScrollableElement},
    theme,
};

use super::inspector_controller::InspectorController;
use super::tree_view::{render_active_summary, render_tree};

pub(super) fn render_inspector(
    inspector: &mut Inspector,
    window: &mut Window,
    cx: &mut Context<Inspector>,
) -> AnyElement {
    div()
        .size_full()
        .flex()
        .flex_col()
        .bg(theme::colors().background_secondary)
        .text_color(theme::colors().text_primary)
        .child(
            div()
                .h(px(44.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_between()
                .px_3()
                .border_b_1()
                .border_color(theme::colors().border_default)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child("GPUI Inspector")
                        .child(
                            div()
                                .text_size(px(10.0))
                                .text_color(theme::colors().text_secondary)
                                .child("auto update"),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .id("inspector-refresh")
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .cursor_pointer()
                                .bg(theme::colors().background_tertiary)
                                .hover(|style| style.bg(theme::colors().button_background_hover))
                                .child("Refresh")
                                .on_click(cx.listener(|_, _, _, cx| {
                                    cx.defer(InspectorController::refresh_target);
                                })),
                        )
                        .child(
                            div()
                                .id("inspector-pick")
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .cursor_pointer()
                                .bg(if inspector.is_picking() {
                                    theme::colors().button_background_selected
                                } else {
                                    theme::colors().background_tertiary
                                })
                                .hover(|style| style.bg(theme::colors().button_background_hover))
                                .child("Pick element")
                                .on_click(cx.listener(|inspector, _, window, cx| {
                                    inspector.start_picking();
                                    window.refresh();
                                    cx.notify();
                                    cx.defer(InspectorController::repaint_target);
                                })),
                        ),
                ),
        )
        .child(
            div()
                .min_h(px(0.0))
                .flex_1()
                .flex()
                .child(
                    div()
                        .w(px(310.0))
                        .flex_none()
                        .border_r_1()
                        .border_color(theme::colors().border_default)
                        .child(render_tree(inspector, cx)),
                )
                .child(
                    div()
                        .id("inspector-content")
                        .min_w(px(0.0))
                        .min_h(px(0.0))
                        .flex_1()
                        .scrollable(ScrollAxis::Horizontal)
                        .scrollable(ScrollAxis::Vertical)
                        .p_3()
                        .when_some(render_active_summary(inspector), |element, summary| {
                            element.child(summary)
                        })
                        .children(inspector.render_inspector_states(window, cx)),
                ),
        )
        .into_any_element()
}
