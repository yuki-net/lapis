use gpui::Rgba;
use serde::Deserialize;

use crate::extension_ui::ThemeId;

/// テーマが提供するセマンティックカラートークン。
#[derive(Clone, Debug)]
pub struct ThemeColors {
    // Background & Surface (4段階の面構造)
    pub background_primary: Rgba,
    pub background_secondary: Rgba,
    pub background_tertiary: Rgba,
    pub floating_background: Rgba,
    pub floating_border: Rgba,
    pub floating_shadow: Rgba,

    // Text
    pub text_primary: Rgba,
    pub text_secondary: Rgba,
    pub text_tertiary: Rgba,
    pub text_accent: Rgba,
    pub text_positive: Rgba,
    pub text_warning: Rgba,
    pub text_dangerous: Rgba,

    // Button & Control
    pub button_background: Rgba,
    pub button_background_hover: Rgba,
    pub button_background_selected: Rgba,
    pub button_background_focused: Rgba,
    pub button_border: Rgba,
    pub button_border_selected: Rgba,
    pub button_border_focused: Rgba,

    // Border
    pub border_default: Rgba,

    // States (4系統 × 4点セット)
    pub positive_background: Rgba,
    pub positive_background_hover: Rgba,
    pub positive_border: Rgba,
    pub positive_text: Rgba,
    pub warning_background: Rgba,
    pub warning_background_hover: Rgba,
    pub warning_border: Rgba,
    pub warning_text: Rgba,
    pub danger_background: Rgba,
    pub danger_background_hover: Rgba,
    pub danger_border: Rgba,
    pub danger_text: Rgba,
    pub info_background: Rgba,
    pub info_background_hover: Rgba,
    pub info_border: Rgba,
    pub info_text: Rgba,

    // Editor
    pub editor_caret: Rgba,
    pub editor_selection: Rgba,
    pub editor_search_match: Rgba,
    pub editor_current_line: Rgba,
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
    background_primary: String,
    background_secondary: String,
    background_tertiary: String,
    floating_background: String,
    floating_border: String,
    floating_shadow: String,
    text_primary: String,
    text_secondary: String,
    text_tertiary: String,
    text_accent: String,
    text_positive: String,
    text_warning: String,
    text_dangerous: String,
    button_background: String,
    button_background_hover: String,
    button_background_selected: String,
    button_background_focused: String,
    button_border: String,
    button_border_selected: String,
    button_border_focused: String,
    border_default: String,
    positive_background: String,
    positive_background_hover: String,
    positive_border: String,
    positive_text: String,
    warning_background: String,
    warning_background_hover: String,
    warning_border: String,
    warning_text: String,
    danger_background: String,
    danger_background_hover: String,
    danger_border: String,
    danger_text: String,
    info_background: String,
    info_background_hover: String,
    info_border: String,
    info_text: String,
    editor_caret: String,
    editor_selection: String,
    editor_search_match: String,
    editor_current_line: String,
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
            background_primary: color!(background_primary),
            background_secondary: color!(background_secondary),
            background_tertiary: color!(background_tertiary),
            floating_background: color!(floating_background),
            floating_border: color!(floating_border),
            floating_shadow: color!(floating_shadow),
            text_primary: color!(text_primary),
            text_secondary: color!(text_secondary),
            text_tertiary: color!(text_tertiary),
            text_accent: color!(text_accent),
            text_positive: color!(text_positive),
            text_warning: color!(text_warning),
            text_dangerous: color!(text_dangerous),
            button_background: color!(button_background),
            button_background_hover: color!(button_background_hover),
            button_background_selected: color!(button_background_selected),
            button_background_focused: color!(button_background_focused),
            button_border: color!(button_border),
            button_border_selected: color!(button_border_selected),
            button_border_focused: color!(button_border_focused),
            border_default: color!(border_default),
            positive_background: color!(positive_background),
            positive_background_hover: color!(positive_background_hover),
            positive_border: color!(positive_border),
            positive_text: color!(positive_text),
            warning_background: color!(warning_background),
            warning_background_hover: color!(warning_background_hover),
            warning_border: color!(warning_border),
            warning_text: color!(warning_text),
            danger_background: color!(danger_background),
            danger_background_hover: color!(danger_background_hover),
            danger_border: color!(danger_border),
            danger_text: color!(danger_text),
            info_background: color!(info_background),
            info_background_hover: color!(info_background_hover),
            info_border: color!(info_border),
            info_text: color!(info_text),
            editor_caret: color!(editor_caret),
            editor_selection: color!(editor_selection),
            editor_search_match: color!(editor_search_match),
            editor_current_line: color!(editor_current_line),
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

        let invalid = source.replace(r#""version": 1"#, r#""version": 2"#);
        assert!(ThemeDefinition::from_json(&invalid).is_err());
    }
}
