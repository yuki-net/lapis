use gpui::{Rgba, rgb};

pub const TITLE_BAR_HEIGHT: f32 = 40.0;
pub const TOOL_ISLAND_WIDTH: f32 = 260.0;
pub const ISLAND_RADIUS: f32 = 8.0;
pub const CANVAS_GAP: f32 = 6.0;

pub fn canvas() -> Rgba {
    rgb(0x0f1012)
}

pub fn title_bar() -> Rgba {
    rgb(0x0d0e10)
}

pub fn island() -> Rgba {
    rgb(0x18191d)
}

pub fn surface() -> Rgba {
    rgb(0x202127)
}

pub fn surface_hover() -> Rgba {
    rgb(0x272931)
}

pub fn surface_active() -> Rgba {
    rgb(0x2d3039)
}

pub fn border() -> Rgba {
    rgb(0x25262b)
}

pub fn text() -> Rgba {
    rgb(0xe6e7eb)
}

pub fn muted() -> Rgba {
    rgb(0x989ba5)
}

pub fn subtle() -> Rgba {
    rgb(0x676b75)
}

pub fn accent() -> Rgba {
    rgb(0x7a7df5)
}

pub fn accent_soft() -> Rgba {
    rgb(0x2a2b43)
}

pub fn orange() -> Rgba {
    rgb(0xe4a86c)
}
