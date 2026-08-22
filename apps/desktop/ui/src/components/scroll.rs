use gpui::{ElementId, div, prelude::*, px};

use crate::theme;

/// Scroll direction supported by the shared scrollable surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScrollAxis {
    Vertical,
    Horizontal,
    Both,
}

/// Applies the shared scrollbar size and overflow behavior to a GPUI element.
pub(crate) trait ScrollableElement: Sized {
    fn scrollable(self, axis: ScrollAxis) -> Self;
}

impl ScrollableElement for gpui::Stateful<gpui::Div> {
    fn scrollable(mut self, axis: ScrollAxis) -> Self {
        self = self.scrollbar_width(px(theme::SCROLLBAR_WIDTH));
        match axis {
            ScrollAxis::Vertical => self.overflow_y_scroll(),
            ScrollAxis::Horizontal => self.overflow_x_scroll(),
            ScrollAxis::Both => self.overflow_scroll(),
        }
    }
}

/// Creates a scrollable surface whose axis and scrollbar width are shared by the UI.
pub(crate) fn scroll_area(id: impl Into<ElementId>, axis: ScrollAxis) -> gpui::Stateful<gpui::Div> {
    div().id(id).scrollable(axis)
}

/// Creates the standard scroll area used by a panel body.
///
/// The panel scrollbar is intentionally wider than feature-local scrollbars so the
/// scrollable boundary remains visible even when the body is visually dense.
pub(crate) fn panel_scroll_area(id: impl Into<ElementId>) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .flex_col()
        .scrollable(ScrollAxis::Both)
        .scrollbar_width(px(theme::PANEL_SCROLLBAR_WIDTH))
}
