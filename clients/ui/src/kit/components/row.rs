//! Feature の一覧行で共有する選択・hover 表現。

use gpui::{App, Div, ElementId, Stateful, div, prelude::*};

use crate::{theme, tokens};

/// 選択状態を持たない一覧行。hover で背景を変える。
pub(crate) fn hover_row(id: impl Into<ElementId>, cx: &App) -> Stateful<Div> {
    let colors = theme::colors(cx);
    div()
        .id(id)
        .rounded(tokens::radius::CONTROL)
        .hover(move |style| style.bg(colors.button_background_hover))
}

/// 選択状態を持つ一覧行。選択中は hover で背景を変えない。
pub(crate) fn selectable_row(id: impl Into<ElementId>, selected: bool, cx: &App) -> Stateful<Div> {
    let colors = theme::colors(cx);
    if selected {
        div()
            .id(id)
            .rounded(tokens::radius::CONTROL)
            .bg(colors.button_background_selected)
    } else {
        div()
            .id(id)
            .rounded(tokens::radius::CONTROL)
            .hover(move |style| style.bg(colors.button_background_hover))
    }
}
