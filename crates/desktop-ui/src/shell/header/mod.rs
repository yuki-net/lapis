mod controls;
mod drag;

pub(crate) use controls::{top_icon, window_control_button};
use drag::apply_drag_region;

use super::*;

impl Editor {
    /// タイトルバー全体を描画する。
    pub(super) fn render_header(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
        compact_layout: bool,
    ) -> impl IntoElement {
        let maximize_label = if window.is_maximized() { "❐" } else { "□" };
        let active_conversation = self.conversation.session.active_id().clone();
        let conversations = self
            .conversation
            .session
            .records()
            .iter()
            .take(if compact_layout { 2 } else { 4 })
            .enumerate()
            .map(|(index, record)| (index, record.id.clone()))
            .collect::<Vec<_>>();

        div()
            .h(px(theme::TITLE_BAR_HEIGHT))
            .w_full()
            .flex_shrink_0()
            .pl(px(12.0))
            .flex()
            .flex_row()
            .items_center()
            .bg(theme::title_bar())
            .border_b_1()
            .border_color(theme::border())
            .when(cfg!(target_os = "macos"), |this| {
                this.child(div().w(px(80.0)).flex_shrink_0())
            })
            .child(
                div()
                    .w(px(if compact_layout { 200.0 } else { 320.0 }))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .size(px(22.0))
                            .rounded(px(6.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(theme::accent())
                            .text_color(rgb(0xffffff))
                            .text_size(px(11.0))
                            .child("L"),
                    )
                    .child(top_icon("☰", false))
                    .child(top_icon("▤", true))
                    .child(
                        top_icon(
                            "▥",
                            self.shell.side_panel.as_ref().is_some_and(|view| {
                                view.as_str() == id::VIEW_PREVIEW
                            }),
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.toggle_preview(&TogglePreview, window, cx);
                        })),
                    )
                    .child(
                        top_icon("▱", self.shell.bottom_panel_open).on_click(cx.listener(
                            |this, _, window, cx| {
                                this.toggle_bottom_panel(&ToggleBottomPanel, window, cx);
                            },
                        )),
                    )
                    .children(conversations.into_iter().map(|(index, conv_id)| {
                        let active = conv_id == active_conversation;
                        div()
                            .id(("conversation", index))
                            .h(px(24.0))
                            .min_w(px(24.0))
                            .px_1()
                            .rounded(px(6.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(10.0))
                            .text_color(if active {
                                rgb(0xbfc0ff)
                            } else {
                                theme::muted()
                            })
                            .bg(if active {
                                theme::accent_soft()
                            } else {
                                theme::title_bar()
                            })
                            .hover(|style| style.bg(theme::surface_hover()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.switch_conversation(conv_id.clone(), cx);
                            }))
                            .child((index + 1).to_string())
                    }))
                    .child(
                        div()
                            .id("new-conversation")
                            .size(px(24.0))
                            .rounded(px(6.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(13.0))
                            .text_color(theme::muted())
                            .hover(|style| style.bg(theme::surface_hover()))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.create_conversation(cx);
                            }))
                            .child("+"),
                    ),
            )
            .child(apply_drag_region(
                div()
                    .w(px(0.0))
                    .h(px(
                        theme::TITLE_BAR_HEIGHT - theme::WINDOW_RESIZE_BORDER_HEIGHT,
                    ))
                    .mt(px(theme::WINDOW_RESIZE_BORDER_HEIGHT))
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .text_size(px(12.0))
                    .child(
                        div()
                            .text_color(theme::muted())
                            .child(self.session.workspace_name().to_owned()),
                    )
                    .child(div().text_color(theme::subtle()).child("›"))
                    .child(div().text_color(theme::subtle()).child("local")),
            ))
            .child(
                div()
                    .w(px(if compact_layout { 150.0 } else { 220.0 }))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap_1()
                    .child(
                        div()
                            .id("assistant-toggle")
                            .h(px(27.0))
                            .px_2()
                            .rounded(px(6.0))
                            .flex()
                            .items_center()
                            .gap_1()
                            .text_size(px(11.0))
                            .text_color(rgb(0x8da8ff))
                            .bg(
                                if self.shell.side_panel.as_ref().is_some_and(|view| {
                                    view.as_str() == id::VIEW_ASSISTANT
                                }) {
                                    theme::accent_soft()
                                } else {
                                    theme::title_bar()
                                },
                            )
                            .hover(|style| style.bg(theme::surface_hover()))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_assistant(&ToggleAssistant, window, cx);
                            }))
                            .child("A")
                            .child("Note"),
                    )
                    .child(
                        div()
                            .id("search-header")
                            .size(px(28.0))
                            .rounded(px(6.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(theme::title_bar())
                            .text_color(theme::muted())
                            .hover(|style| style.bg(theme::surface_hover()).text_color(theme::text()))
                            .child(crate::components::Icon::new(crate::components::IconName::Search))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_quick_search(window, cx);
                            }))
                    )
                    .child(top_icon("▷", false)),
            )
            .when(!cfg!(target_os = "macos"), |this| {
                this.child(
                    div()
                        .h_full()
                        .flex_shrink_0()
                        .flex()
                        .items_end()
                        .child(
                            window_control_button(
                                "window-minimize",
                                "—",
                                WindowControlArea::Min,
                                false,
                            )
                            .on_click(cx.listener(|_, _, window, _| window.minimize_window())),
                        )
                        .child(
                            window_control_button(
                                "window-maximize",
                                maximize_label,
                                WindowControlArea::Max,
                                false,
                            )
                            .on_click(cx.listener(|_, _, window, _| window.zoom_window())),
                        )
                        .child(
                            window_control_button(
                                "window-close",
                                "×",
                                WindowControlArea::Close,
                                true,
                            )
                            .on_click(cx.listener(|_, _, window, _| window.remove_window())),
                        ),
                )
            })
    }
}
