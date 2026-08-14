mod definition;
mod registry;
mod tokens;

// 定義（外部から参照可能なもの）
pub use definition::{
    BOTTOM_PANEL_HEIGHT, CANVAS_GAP, ISLAND_RADIUS, SIDE_PANEL_WIDTH, TITLE_BAR_HEIGHT,
    TOOL_ISLAND_WIDTH, ThemeColors, ThemeDefinition, WINDOW_CONTROL_WIDTH,
    WINDOW_RESIZE_BORDER_HEIGHT,
};

// レジストリ操作
pub use registry::{active_id, available, name, register, set_active};

// カラートークン
pub use tokens::{
    accent, accent_soft, assistant_accent, border, brand_text, canvas, close_hover,
    command_input_border, command_palette_border, diff_added, diff_changed, diff_removed,
    editor_cursor, editor_search_match, editor_selection, focus_border, island, muted, note,
    on_accent_text, orange, problem_error, search_selection, status_error, status_info,
    status_success, status_warning, subtle, surface, surface_active, surface_hover,
    task_primary_border, task_primary_text, text, title_bar,
};
