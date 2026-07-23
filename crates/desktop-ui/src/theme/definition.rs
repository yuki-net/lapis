use gpui::Rgba;

use crate::extension_ui::ThemeId;

/// レイアウト定数
pub const TITLE_BAR_HEIGHT: f32 = 40.0;
pub const WINDOW_CONTROL_WIDTH: f32 = 46.0;
pub const WINDOW_RESIZE_BORDER_HEIGHT: f32 = 4.0;
pub const TOOL_ISLAND_WIDTH: f32 = 260.0;
pub const SIDE_PANEL_WIDTH: f32 = 310.0;
pub const BOTTOM_PANEL_HEIGHT: f32 = 196.0;
pub const ISLAND_RADIUS: f32 = 8.0;
pub const CANVAS_GAP: f32 = 6.0;

/// テーマが提供するカラートークン。
#[derive(Clone, Debug)]
pub struct ThemeColors {
    pub canvas: Rgba,
    pub title_bar: Rgba,
    pub island: Rgba,
    pub surface: Rgba,
    pub surface_hover: Rgba,
    pub surface_active: Rgba,
    pub border: Rgba,
    pub text: Rgba,
    pub muted: Rgba,
    pub subtle: Rgba,
    pub accent: Rgba,
    pub accent_soft: Rgba,
    pub orange: Rgba,
    pub close_hover: Rgba,
}

/// テーマの定義。ID とカラートークンのセット。
#[derive(Clone, Debug)]
pub struct ThemeDefinition {
    pub id: ThemeId,
    pub colors: ThemeColors,
}

impl ThemeDefinition {
    pub fn new(id: impl Into<ThemeId>, colors: ThemeColors) -> Self {
        Self {
            id: id.into(),
            colors,
        }
    }
}
