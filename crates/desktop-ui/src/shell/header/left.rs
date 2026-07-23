use super::*;

impl Editor {
    pub(super) fn render_header_left(
        &self,
        cx: &mut Context<Self>,
        compact_layout: bool,
    ) -> impl IntoElement {
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
            .child(controls::top_icon("☰", false))
            .child(controls::top_icon("▤", true))
            .child(
                controls::top_icon(
                    "▥",
                    self.shell
                        .side_panel
                        .as_ref()
                        .is_some_and(|view| view.as_str() == id::VIEW_PREVIEW),
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.toggle_preview(&TogglePreview, window, cx);
                })),
            )
            .child(
                controls::top_icon("▱", self.shell.bottom_panel_open).on_click(cx.listener(
                    |this, _, window, cx| {
                        this.toggle_bottom_panel(&ToggleBottomPanel, window, cx);
                    },
                )),
            )
            .children(conversations.into_iter().map(|(index, conversation_id)| {
                let active = conversation_id == active_conversation;
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
                    .text_color(if active { rgb(0xbfc0ff) } else { theme::muted() })
                    .bg(if active { theme::accent_soft() } else { theme::title_bar() })
                    .hover(|style| style.bg(theme::surface_hover()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.switch_conversation(conversation_id.clone(), cx);
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
                    .on_click(cx.listener(|this, _, _, cx| this.create_conversation(cx)))
                    .child("+"),
            )
    }
}
