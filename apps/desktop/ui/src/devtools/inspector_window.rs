use gpui::{AnyElement, Context, Div, Inspector, IntoElement, Window, div, prelude::*, px};

use crate::{
    components::{
        ScrollAxis, ScrollState, header_button, panel_header, panel_surface, scroll_viewport,
    },
    theme, tokens,
};

use super::elements::tree_view::{render_active_summary, render_tree};
use super::inspector_controller::InspectorController;
use super::tool::DevToolTab;

pub(super) fn render_inspector(
    inspector: &mut Inspector,
    window: &mut Window,
    cx: &mut Context<Inspector>,
) -> AnyElement {
    let entity = cx.entity_id();
    let active_tab = InspectorController::active_tab(entity, cx);
    let content_scroll = InspectorController::content_scroll(entity, cx);

    // DevToolタブバー
    let tab_bar = div()
        .id("devtool-tab-bar")
        .flex()
        .items_center()
        .px(tokens::spacing::SM)
        .py(px(4.))
        .gap(px(2.))
        .bg(theme::colors().background_secondary)
        .border_b_1()
        .border_color(theme::colors().border_default)
        .children(DevToolTab::all().iter().enumerate().map(|(index, tab)| {
            let is_active = *tab == active_tab;
            let tab_label = tab.title();
            div()
                .id(("devtool-tab", index))
                .px(tokens::spacing::MD)
                .py(px(4.))
                .rounded(px(4.))
                .cursor_pointer()
                .text_size(px(12.))
                .font_weight(if is_active {
                    gpui::FontWeight::SEMIBOLD
                } else {
                    gpui::FontWeight::NORMAL
                })
                .when(is_active, |el| {
                    el.bg(theme::colors().button_background_selected)
                        .text_color(theme::colors().text_primary)
                })
                .when(!is_active, |el| {
                    el.text_color(theme::colors().text_secondary)
                })
                .when(!is_active, |el| {
                    el.bg(theme::colors().button_background_hover)
                })
                .child(tab_label)
                .on_click(cx.listener(move |_, _, _, cx| {
                    InspectorController::set_active_tab(*tab, entity, cx);
                    cx.notify();
                }))
        }));

    // タブコンテンツ
    let tab_content = match active_tab {
        DevToolTab::Elements => render_elements_tab(inspector, window, cx, &content_scroll),
        _ => render_placeholder_tab(active_tab),
    };

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
                .child("Lapis DevTool"),
            |_, window, _| window.remove_window(),
        ))
        .child(tab_bar)
        .child(
            div()
                .id("devtool-content")
                .min_h(px(0.))
                .flex_1()
                .child(tab_content),
        )
        .into_any_element()
}

fn render_elements_tab(
    inspector: &mut Inspector,
    window: &mut Window,
    cx: &mut Context<Inspector>,
    content_scroll: &ScrollState,
) -> Div {
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
                        content_scroll,
                        div()
                            .p_3()
                            .when_some(render_active_summary(inspector), |element, summary| {
                                element.child(summary)
                            })
                            .children(inspector.render_inspector_states(window, cx)),
                    )
                    .min_w(px(0.))
                    .min_h(px(0.))
                    .flex_1(),
                ),
        )
}

fn render_placeholder_tab(tab: DevToolTab) -> Div {
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_4()
        .p_8()
        .child(
            div()
                .text_size(px(18.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme::colors().text_primary)
                .child(tab.title()),
        )
        .child(
            div()
                .text_size(px(13.))
                .text_color(theme::colors().text_secondary)
                .child("この診断Toolは後続実装"),
        )
        .child(
            div()
                .text_size(px(11.))
                .text_color(theme::colors().text_secondary)
                .child("この機能は今後のPRで追加されます"),
        )
}
