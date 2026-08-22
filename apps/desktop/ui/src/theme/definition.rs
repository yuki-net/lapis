use gpui::Rgba;
use serde::Deserialize;

use crate::extension_ui::ThemeId;

/// レイアウト定数
pub const TITLE_BAR_HEIGHT: f32 = 40.0;
pub const HEADER_BUTTON_SIZE: f32 = 28.0;
pub const BUTTON_HEIGHT_XS: f32 = 22.0;
pub const BUTTON_HEIGHT_SM: f32 = 25.0;
pub const SCROLLBAR_WIDTH: f32 = 8.0;
/// Panel本文ではスクロール領域を視認しやすくするため、標準より広く確保する。
/// GPUIはスクロールバーの色を直接指定できないため、幅で強調する。
pub const PANEL_SCROLLBAR_WIDTH: f32 = 10.0;
pub const WINDOW_CONTROL_WIDTH: f32 = 46.0;
pub const WINDOW_RESIZE_BORDER_HEIGHT: f32 = 4.0;
pub const TOOL_ISLAND_WIDTH: f32 = 260.0;
pub const SIDE_PANEL_WIDTH: f32 = 310.0;
pub const BOTTOM_PANEL_HEIGHT: f32 = 196.0;
pub const ISLAND_RADIUS: f32 = 8.0;
pub const CANVAS_GAP: f32 = 8.0;

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
    pub command_palette_border: Rgba,
    pub focus_border: Rgba,
    pub brand_text: Rgba,
    pub on_accent_text: Rgba,
    pub assistant_accent: Rgba,
    pub editor_selection: Rgba,
    pub editor_search_match: Rgba,
    pub editor_cursor: Rgba,
    pub search_selection: Rgba,
    pub command_input_border: Rgba,
    pub task_primary_border: Rgba,
    pub task_primary_text: Rgba,
    pub status_success: Rgba,
    pub status_error: Rgba,
    pub status_warning: Rgba,
    pub status_info: Rgba,
    pub diff_added: Rgba,
    pub diff_removed: Rgba,
    pub diff_changed: Rgba,
    pub problem_error: Rgba,
    pub note: Rgba,
}

/// テーマの定義。ID とカラートークンのセット。
#[derive(Clone, Debug)]
pub struct ThemeDefinition {
    pub id: ThemeId,
    pub name: String,
    pub colors: ThemeColors,
}

impl ThemeDefinition {
    pub fn new(id: impl Into<ThemeId>, colors: ThemeColors) -> Self {
        let id = id.into();
        Self {
            name: id.as_str().to_owned(),
            id,
            colors,
        }
    }

    pub fn named(id: impl Into<ThemeId>, name: impl Into<String>, colors: ThemeColors) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            colors,
        }
    }

    pub fn from_json(source: &str) -> Result<Self, String> {
        let file: ThemeFile = serde_json::from_str(source)
            .map_err(|error| format!("theme JSON parse failed: {error}"))?;
        if file.version != 1 {
            return Err(format!("unsupported theme version: {}", file.version));
        }
        if file.id.trim().is_empty() {
            return Err("theme id must not be empty".to_owned());
        }
        if file.name.trim().is_empty() {
            return Err("theme name must not be empty".to_owned());
        }
        Ok(Self::named(file.id, file.name, file.colors.try_into()?))
    }
}

#[derive(Debug, Deserialize)]
struct ThemeFile {
    version: u32,
    id: String,
    name: String,
    colors: ThemeColorsFile,
}

#[derive(Debug, Deserialize)]
struct ThemeColorsFile {
    canvas: String,
    title_bar: String,
    island: String,
    surface: String,
    surface_hover: String,
    surface_active: String,
    border: String,
    text: String,
    muted: String,
    subtle: String,
    accent: String,
    accent_soft: String,
    orange: String,
    close_hover: String,
    command_palette_border: String,
    focus_border: String,
    brand_text: String,
    on_accent_text: String,
    assistant_accent: String,
    editor_selection: String,
    editor_search_match: String,
    editor_cursor: String,
    search_selection: String,
    command_input_border: String,
    task_primary_border: String,
    task_primary_text: String,
    status_success: String,
    status_error: String,
    status_warning: String,
    status_info: String,
    diff_added: String,
    diff_removed: String,
    diff_changed: String,
    problem_error: String,
    note: String,
}

impl TryFrom<ThemeColorsFile> for ThemeColors {
    type Error = String;

    fn try_from(file: ThemeColorsFile) -> Result<Self, Self::Error> {
        macro_rules! color {
            ($field:ident) => {
                parse_color(stringify!($field), &file.$field)?
            };
        }

        Ok(Self {
            canvas: color!(canvas),
            title_bar: color!(title_bar),
            island: color!(island),
            surface: color!(surface),
            surface_hover: color!(surface_hover),
            surface_active: color!(surface_active),
            border: color!(border),
            text: color!(text),
            muted: color!(muted),
            subtle: color!(subtle),
            accent: color!(accent),
            accent_soft: color!(accent_soft),
            orange: color!(orange),
            close_hover: color!(close_hover),
            command_palette_border: color!(command_palette_border),
            focus_border: color!(focus_border),
            brand_text: color!(brand_text),
            on_accent_text: color!(on_accent_text),
            assistant_accent: color!(assistant_accent),
            editor_selection: color!(editor_selection),
            editor_search_match: color!(editor_search_match),
            editor_cursor: color!(editor_cursor),
            search_selection: color!(search_selection),
            command_input_border: color!(command_input_border),
            task_primary_border: color!(task_primary_border),
            task_primary_text: color!(task_primary_text),
            status_success: color!(status_success),
            status_error: color!(status_error),
            status_warning: color!(status_warning),
            status_info: color!(status_info),
            diff_added: color!(diff_added),
            diff_removed: color!(diff_removed),
            diff_changed: color!(diff_changed),
            problem_error: color!(problem_error),
            note: color!(note),
        })
    }
}

fn parse_color(name: &str, value: &str) -> Result<Rgba, String> {
    let hex = value
        .strip_prefix('#')
        .ok_or_else(|| format!("{name} must start with '#'"))?;
    if hex.len() != 6 && hex.len() != 8 {
        return Err(format!("{name} must contain 6 or 8 hex digits"));
    }
    let mut raw = u32::from_str_radix(hex, 16)
        .map_err(|error| format!("{name} is not a valid color: {error}"))?;
    if hex.len() == 6 {
        raw = (raw << 8) | 0xff;
    }
    Ok(gpui::rgba(raw))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rgb_and_rgba_colors() {
        assert_eq!(
            parse_color("rgb", "#123456").unwrap(),
            gpui::rgba(0x123456ff)
        );
        assert_eq!(
            parse_color("rgba", "#12345678").unwrap(),
            gpui::rgba(0x12345678)
        );
    }

    #[test]
    fn rejects_invalid_theme_color() {
        assert!(parse_color("accent", "123456").is_err());
        assert!(parse_color("accent", "#12345").is_err());
        assert!(parse_color("accent", "#gggggg").is_err());
    }

    #[test]
    fn parses_bundled_theme_and_rejects_unknown_version() {
        let source = include_str!("../../assets/themes/dark.json");
        let definition = ThemeDefinition::from_json(source).unwrap();
        assert_eq!(definition.id.as_str(), "lapis.dark");
        assert_eq!(definition.name, "Dark");

        let invalid = source.replace("\"version\": 1", "\"version\": 2");
        assert!(ThemeDefinition::from_json(&invalid).is_err());
    }
}
