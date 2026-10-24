use gpui::{AnyElement, Context, Inspector, IntoElement, Window, div, prelude::*, px};

use crate::{
    components::{ScrollAxis, header_button, panel_header, panel_surface, scroll_viewport},
    theme, tokens,
};

use super::inspector_controller::InspectorController;
use super::tree_view::{render_active_summary, render_tree};

pub(super) fn render_inspector(
    inspector: &mut Inspector,
    window: &mut Window,
    cx: &mut Context<Inspector>,
) -> AnyElement {
    let content_scroll = InspectorController::content_scroll(cx.entity_id(), cx);
    let details_header = panel_header()
        .id("inspector-details-header")
        .px(tokens::spacing::SM)
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .child("Element details"),
        )
        .child(
            header_button("inspector-refresh", "Refresh")
                .cursor_pointer()
                .on_click(cx.listener(|_, _, _, cx| {
                    InspectorController::defer(cx, InspectorController::refresh_target);
                })),
        )
        .child(
            header_button("inspector-pick", "Pick element")
                .cursor_pointer()
                .when(inspector.is_picking(), |button| {
                    button.bg(theme::colors().button_background_selected)
                })
                .on_click(cx.listener(|inspector, _, window, cx| {
                    inspector.start_picking();
                    window.refresh();
                    cx.notify();
                    InspectorController::defer(cx, InspectorController::repaint_target);
                })),
        );
    div()
        .size_full()
        .flex()
        .flex_col()
        .bg(theme::colors().background_primary)
        .text_color(theme::colors().text_primary)
        .child(crate::window_host::chrome::title_bar(
            div()
                .id("inspector-title")
                .h_full()
                .flex_1()
                .flex()
                .items_center()
                .px_3()
                .child("Lapis Inspector"),
            |_, window, _| window.remove_window(),
        ))
        .child(
            div()
                .min_h(px(0.))
                .flex_1()
                .flex()
                .p(tokens::spacing::GAP)
                .gap(tokens::spacing::GAP)
                .child(
                    panel_surface()
                        .id("inspector-tree-island")
                        .w(tokens::size::SIDE_PANEL_WIDTH)
                        .flex_none()
                        .child(render_tree(inspector, cx)),
                )
                .child(
                    panel_surface()
                        .id("inspector-details-island")
                        .flex_1()
                        .child(details_header)
                        .child(
                            scroll_viewport(
                                "inspector-content",
                                ScrollAxis::Both,
                                &content_scroll,
                                div()
                                    .p_3()
                                    .when_some(
                                        render_active_summary(inspector),
                                        |element, summary| element.child(summary),
                                    )
                                    .children(inspector.render_inspector_states(window, cx)),
                            )
                            .min_w(px(0.))
                            .min_h(px(0.))
                            .flex_1(),
                        ),
                ),
        )
        .into_any_element()
}
