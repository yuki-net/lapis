use gpui::{Pixels, px};

pub mod spacing {
    use super::*;

    pub const NONE: Pixels = px(0.0);
    pub const XS: Pixels = px(4.0);
    pub const SM: Pixels = px(8.0);
    pub const MD: Pixels = px(12.0);
    pub const LG: Pixels = px(16.0);
    pub const GAP: Pixels = px(8.0);
}

pub mod radius {
    use super::*;

    pub const NONE: Pixels = px(0.0);
    pub const XS: Pixels = px(4.0);
    pub const SM: Pixels = px(6.0);
    pub const MD: Pixels = px(8.0);
    pub const LG: Pixels = px(12.0);
    pub const FULL: Pixels = px(9999.0);

    // 用途別セマンティックエイリアス
    pub const CONTROL: Pixels = px(6.0);
    pub const PANEL: Pixels = px(8.0);
    pub const MENU: Pixels = px(7.0);
    pub const MENU_ITEM: Pixels = px(4.0);
    pub const TAB: Pixels = px(6.0);
}

pub mod size {
    use super::*;

    pub const TITLE_BAR_HEIGHT: Pixels = px(40.0);
    pub const HEADER_BUTTON: Pixels = px(28.0);
    pub const BUTTON_XS: Pixels = px(22.0);
    pub const BUTTON_SM: Pixels = px(25.0);
    pub const BUTTON_MD: Pixels = px(28.0);
    pub const SCROLLBAR: Pixels = px(8.0);
    pub const PANEL_SCROLLBAR: Pixels = px(10.0);
    pub const WINDOW_CONTROL_WIDTH: Pixels = px(46.0);
    pub const WINDOW_RESIZE_BORDER: Pixels = px(4.0);
    pub const TOOL_ISLAND_WIDTH: Pixels = px(260.0);
    pub const SIDE_PANEL_WIDTH: Pixels = px(310.0);
    pub const BOTTOM_PANEL_HEIGHT: Pixels = px(196.0);
    pub const PANEL_MIN_WIDTH: Pixels = px(200.0);
    pub const PANEL_MIN_HEIGHT: Pixels = px(150.0);
}

pub mod typography {
    use super::*;

    pub const FONT_XS: Pixels = px(10.0);
    pub const FONT_SM: Pixels = px(12.0);
    pub const FONT_MD: Pixels = px(14.0);
    pub const FONT_LG: Pixels = px(16.0);
    pub const FONT_XL: Pixels = px(18.0);
    pub const FONT_2XL: Pixels = px(24.0);
}
