use super::*;

impl Editor {
    pub(super) fn render_header_right(
        &self,
        cx: &mut Context<Self>,
        compact_layout: bool,
    ) -> impl IntoElement {
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
                    .bg(if self
                        .shell
                        .side_panel
                        .as_ref()
                        .is_some_and(|view| view.as_str() == id::VIEW_ASSISTANT)
                    {
                        theme::accent_soft()
                    } else {
                        theme::title_bar()
                    })
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
                    })),
            )
            .child(controls::top_icon("▷", false))
    }
}
