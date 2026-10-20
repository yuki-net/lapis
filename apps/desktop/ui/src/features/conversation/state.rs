use crate::components::action_button;
use gpui::{Context, Render, Window, div, prelude::*};
use lapis_desktop_client::ConversationRecord;
use lapis_editor_core::ConversationId;

#[derive(Clone)]
pub(crate) enum ConversationEvent {
    Create,
    Switch(ConversationId),
}
pub(crate) struct ConversationFeature {
    pub(crate) records: Vec<ConversationRecord>,
    pub(crate) active: Option<ConversationId>,
}
impl gpui::EventEmitter<ConversationEvent> for ConversationFeature {}
impl ConversationFeature {
    pub(crate) fn new() -> Self {
        Self {
            records: Vec::new(),
            active: None,
        }
    }
    pub(crate) fn update(
        &mut self,
        records: Vec<ConversationRecord>,
        active: ConversationId,
        cx: &mut Context<Self>,
    ) {
        self.records = records;
        self.active = Some(active);
        cx.notify();
    }
}
impl Render for ConversationFeature {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .child(
                action_button("New conversation", true)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(ConversationEvent::Create))),
            )
            .children(self.records.iter().enumerate().map(|(index, record)| {
                let id = record.id.clone();
                div()
                    .id(("conversation", index))
                    .p_2()
                    .child(record.title.clone())
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(ConversationEvent::Switch(id.clone()))
                    }))
            }))
    }
}
