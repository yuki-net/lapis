use gpui::{Anchored, ElementId, InteractiveElement, Pixels, Point, Stateful, anchored, px};

use super::{SurfaceVariant, surface};

/// Owns the anchored position and window-edge fitting for floating UI.
///
/// Open/close state and outside-click behavior remain with the caller.
pub(crate) fn floating_tree(anchor: Point<Pixels>, offset: Point<Pixels>) -> Anchored {
    anchored()
        .position(anchor)
        .offset(offset)
        .snap_to_window_with_margin(px(8.0))
}

/// Owns the shared floating surface and blocks hit testing behind the panel.
pub(crate) fn floating_panel(
    id: impl Into<ElementId>,
    variant: SurfaceVariant,
) -> Stateful<gpui::Div> {
    surface(variant).id(id).occlude()
}
