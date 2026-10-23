use gpui::{App, Bounds, Inspector, Pixels, Point, Size, Window};

pub(super) fn paint(inspector: &Inspector, window: &mut Window, _cx: &App) {
    // Pick・Tree Hover・Tree選択の表示方針はLapisが所有する。
    let is_picking = inspector.is_picking();
    let is_tree_preview = inspector.hovered_element_id().is_some();
    let is_selected = inspector.selected_element_visible();
    if !is_picking && !is_tree_preview && !is_selected {
        return;
    }

    let rem_size = window.rem_size();
    let mut highlighted_box_model = None;

    let highlighted_state = if is_picking || (is_selected && !is_tree_preview) {
        inspector.active_element_state::<gpui::DivInspectorState>()
    } else if is_tree_preview {
        if inspector.hovered_element_id() == inspector.active_element_id() {
            inspector.active_element_state::<gpui::DivInspectorState>()
        } else {
            inspector.hovered_element_state::<gpui::DivInspectorState>()
        }
    } else {
        inspector.active_element_state::<gpui::DivInspectorState>()
    };
    if let Some(state) = highlighted_state {
        if state.bounds.size.width != Pixels::ZERO && state.bounds.size.height != Pixels::ZERO {
            let to_px_len = |len: &gpui::Length| -> Pixels {
                match len {
                    gpui::Length::Definite(def) => {
                        def.to_pixels(gpui::AbsoluteLength::Pixels(Pixels::ZERO), rem_size)
                    }
                    gpui::Length::Auto => Pixels::ZERO,
                }
            };
            let to_px_def = |def: &gpui::DefiniteLength| -> Pixels {
                def.to_pixels(gpui::AbsoluteLength::Pixels(Pixels::ZERO), rem_size)
            };
            let to_px_abs = |abs: &gpui::AbsoluteLength| -> Pixels { abs.to_pixels(rem_size) };

            let margin_top = state
                .base_style
                .margin
                .top
                .as_ref()
                .map(to_px_len)
                .unwrap_or(Pixels::ZERO);
            let margin_right = state
                .base_style
                .margin
                .right
                .as_ref()
                .map(to_px_len)
                .unwrap_or(Pixels::ZERO);
            let margin_bottom = state
                .base_style
                .margin
                .bottom
                .as_ref()
                .map(to_px_len)
                .unwrap_or(Pixels::ZERO);
            let margin_left = state
                .base_style
                .margin
                .left
                .as_ref()
                .map(to_px_len)
                .unwrap_or(Pixels::ZERO);

            let border_top = state
                .base_style
                .border_widths
                .top
                .as_ref()
                .map(to_px_abs)
                .unwrap_or(Pixels::ZERO);
            let border_right = state
                .base_style
                .border_widths
                .right
                .as_ref()
                .map(to_px_abs)
                .unwrap_or(Pixels::ZERO);
            let border_bottom = state
                .base_style
                .border_widths
                .bottom
                .as_ref()
                .map(to_px_abs)
                .unwrap_or(Pixels::ZERO);
            let border_left = state
                .base_style
                .border_widths
                .left
                .as_ref()
                .map(to_px_abs)
                .unwrap_or(Pixels::ZERO);

            let padding_top = state
                .base_style
                .padding
                .top
                .as_ref()
                .map(to_px_def)
                .unwrap_or(Pixels::ZERO);
            let padding_right = state
                .base_style
                .padding
                .right
                .as_ref()
                .map(to_px_def)
                .unwrap_or(Pixels::ZERO);
            let padding_bottom = state
                .base_style
                .padding
                .bottom
                .as_ref()
                .map(to_px_def)
                .unwrap_or(Pixels::ZERO);
            let padding_left = state
                .base_style
                .padding
                .left
                .as_ref()
                .map(to_px_def)
                .unwrap_or(Pixels::ZERO);

            let border_bounds = state.bounds;
            let margin_bounds = Bounds {
                origin: Point {
                    x: border_bounds.origin.x - margin_left,
                    y: border_bounds.origin.y - margin_top,
                },
                size: Size {
                    width: border_bounds.size.width + margin_left + margin_right,
                    height: border_bounds.size.height + margin_top + margin_bottom,
                },
            };
            let padding_bounds = Bounds {
                origin: Point {
                    x: border_bounds.origin.x + border_left,
                    y: border_bounds.origin.y + border_top,
                },
                size: Size {
                    width: (border_bounds.size.width - border_left - border_right)
                        .max(Pixels::ZERO),
                    height: (border_bounds.size.height - border_top - border_bottom)
                        .max(Pixels::ZERO),
                },
            };
            let content_bounds = Bounds {
                origin: Point {
                    x: padding_bounds.origin.x + padding_left,
                    y: padding_bounds.origin.y + padding_top,
                },
                size: Size {
                    width: (padding_bounds.size.width - padding_left - padding_right)
                        .max(Pixels::ZERO),
                    height: (padding_bounds.size.height - padding_top - padding_bottom)
                        .max(Pixels::ZERO),
                },
            };

            highlighted_box_model =
                Some((margin_bounds, border_bounds, padding_bounds, content_bounds));
        }
    }

    if let Some((margin, border, padding, content)) = highlighted_box_model {
        // Margin (Orange): #f6b26b
        window.paint_quad(gpui::fill(margin, gpui::rgba(0xf6b26b59)));
        // Border (Yellow): #ffe599
        window.paint_quad(gpui::fill(border, gpui::rgba(0xffe59959)));
        // Padding (Green): #93c47d
        window.paint_quad(gpui::fill(padding, gpui::rgba(0x93c47d59)));
        // Content (Blue): #6fa8dc
        window.paint_quad(gpui::fill(content, gpui::rgba(0x6fa8dc80)));
    } else if let Some(bounds) = inspector
        .hovered_element_id()
        .or_else(|| inspector.active_element_id())
        .and_then(|id| inspector.element_tree().iter().find(|node| &node.id == id))
        .map(|node| node.bounds)
    {
        window.paint_quad(gpui::fill(bounds, gpui::rgba(0x61afef4d)));
    }
}
