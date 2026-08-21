use gpui::{div, prelude::*, px};

use crate::theme;

pub(crate) fn separator() -> gpui::Div {
    div()
        .h(px(1.0))
        .my(theme::spacing(theme::Spacing::Xs))
        .bg(theme::border())
}
