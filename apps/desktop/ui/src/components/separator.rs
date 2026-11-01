use gpui::{div, prelude::*, px};

use crate::{theme, tokens};

pub(crate) fn separator(cx: &gpui::App) -> gpui::Div {
    div()
        .h(px(1.0))
        .w_full()
        .flex_shrink_0()
        .my(tokens::spacing::XS)
        .bg(theme::colors(cx).floating_border)
}
