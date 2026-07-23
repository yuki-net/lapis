mod definition;
mod registry;
mod tokens;

// 定義（外部から参照可能なもの）
pub use definition::{
    ThemeColors, ThemeDefinition,
    BOTTOM_PANEL_HEIGHT, CANVAS_GAP, ISLAND_RADIUS, SIDE_PANEL_WIDTH, TITLE_BAR_HEIGHT,
    TOOL_ISLAND_WIDTH, WINDOW_CONTROL_WIDTH, WINDOW_RESIZE_BORDER_HEIGHT,
};

// レジストリ操作
pub use registry::{active_id, register, set_active};

// カラートークン
pub use tokens::{
    accent, accent_soft, border, canvas, close_hover, island, muted, orange, subtle, surface,
    surface_active, surface_hover, text, title_bar,
};
