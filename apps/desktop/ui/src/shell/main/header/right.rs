use super::*;
use crate::components::icon_button;

impl Editor {
    pub(super) fn render_header_right(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .h_full()
            .flex_shrink_0()
            .px(px(theme::CANVAS_GAP))
            .flex()
            .items_center()
            .gap_1()
            .child(
                icon_button("open-settings-menu", IconName::Settings).on_click(cx.listener(
                    |this, event: &ClickEvent, _, cx| {
                        this.toggle_settings_menu(event.position(), cx);
                    },
                )),
            )
    }
}
