//! ユーザー全体に適用される設定のモデルと保存契約。

use localization::LocaleId;
use serde::{Deserialize, Serialize};

/// 構文配色の系統。明暗はUIテーマに追従する。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SyntaxTheme {
    #[serde(rename = "vscode", alias = "vscode.dark-plus")]
    VsCode,
    #[serde(alias = "zed.one-dark")]
    Zed,
    #[serde(alias = "intellij.dark")]
    Intellij,
    #[serde(alias = "intellij.darcula")]
    IntellijClassic,
    #[default]
    #[serde(alias = "lapis.dark", alias = "lapis.light", other)]
    Lapis,
}

/// ターミナルのカーソル形状。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TerminalCaretShape {
    #[default]
    Block,
    Bar,
    Underline,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobalSettings {
    pub version: u32,
    pub locale: LocaleId,
    #[serde(default = "default_theme")]
    pub theme: String,
    /// 配色の系統だけを保存する。省略時はLapis、明暗は常にUIテーマへ追従。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub syntax_theme: Option<SyntaxTheme>,
    /// ターミナルのフォント family。省略時は OS ごとの既定値。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_font_family: Option<String>,
    /// ターミナルのフォントサイズ（px）。省略時は 13。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_font_size: Option<u8>,
    /// 行送り（フォントサイズに対する百分率）。省略時は 125。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_line_height_percent: Option<u8>,
    /// カーソル形状。省略時は Block。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_caret_shape: Option<TerminalCaretShape>,
}

fn default_theme() -> String {
    "lapis.dark".to_owned()
}

impl Default for GlobalSettings {
    fn default() -> Self {
        Self {
            version: 1,
            locale: LocaleId::new("en-US"),
            theme: default_theme(),
            syntax_theme: None,
            terminal_font_family: None,
            terminal_font_size: None,
            terminal_line_height_percent: None,
            terminal_caret_shape: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsError(String);

impl SettingsError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for SettingsError {}

pub trait GlobalSettingsRepository: Send + Sync {
    fn load(&self) -> Result<GlobalSettings, SettingsError>;
    fn save(&self, settings: &GlobalSettings) -> Result<(), SettingsError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_settings_without_theme_use_dark() {
        let settings: GlobalSettings =
            serde_json::from_str(r#"{"version":1,"locale":"ja-JP"}"#).unwrap();

        assert_eq!(settings.theme, "lapis.dark");
        assert_eq!(settings.syntax_theme, None);
    }

    #[test]
    fn terminal_font_defaults_are_optional() {
        let settings: GlobalSettings =
            serde_json::from_str(r#"{"version":1,"locale":"ja-JP"}"#).unwrap();
        assert_eq!(settings.terminal_font_family, None);
        assert_eq!(settings.terminal_font_size, None);
        assert_eq!(settings.terminal_line_height_percent, None);
        assert_eq!(settings.terminal_caret_shape, None);

        let selected = GlobalSettings {
            terminal_font_family: Some("Menlo".to_owned()),
            terminal_font_size: Some(15),
            terminal_line_height_percent: Some(140),
            terminal_caret_shape: Some(TerminalCaretShape::Bar),
            ..settings
        };
        let loaded: GlobalSettings =
            serde_json::from_str(&serde_json::to_string(&selected).unwrap()).unwrap();
        assert_eq!(loaded, selected);
        assert_eq!(
            serde_json::to_value(&loaded).unwrap()["terminal_caret_shape"],
            "bar"
        );
    }

    #[test]
    fn syntax_selection_round_trips_independently_of_ui_theme() {
        let old: GlobalSettings =
            serde_json::from_str(r#"{"version":1,"locale":"en-US","theme":"lapis.white"}"#)
                .unwrap();
        assert!(old.syntax_theme.is_none());
        for family in [SyntaxTheme::Intellij, SyntaxTheme::IntellijClassic] {
            let selected = GlobalSettings {
                syntax_theme: Some(family),
                ..old.clone()
            };
            let loaded: GlobalSettings =
                serde_json::from_str(&serde_json::to_string(&selected).unwrap()).unwrap();
            assert_eq!(loaded, selected);
            assert_eq!(loaded.theme, "lapis.white");
        }
    }

    #[test]
    fn previous_palette_ids_migrate_to_families() {
        for (old, family, canonical) in [
            ("lapis.dark", SyntaxTheme::Lapis, "lapis"),
            ("lapis.light", SyntaxTheme::Lapis, "lapis"),
            ("vscode.dark-plus", SyntaxTheme::VsCode, "vscode"),
            ("zed.one-dark", SyntaxTheme::Zed, "zed"),
            ("intellij.dark", SyntaxTheme::Intellij, "intellij"),
            (
                "intellij.darcula",
                SyntaxTheme::IntellijClassic,
                "intellij-classic",
            ),
        ] {
            let json = format!(r#"{{"version":1,"locale":"en-US","syntax_theme":"{old}"}}"#);
            let settings: GlobalSettings = serde_json::from_str(&json).unwrap();
            assert_eq!(settings.syntax_theme, Some(family));
            assert_eq!(
                serde_json::to_value(&settings).unwrap()["syntax_theme"],
                canonical
            );
        }
        let unknown: SyntaxTheme = serde_json::from_str(r#""missing""#).unwrap();
        assert_eq!(unknown, SyntaxTheme::Lapis);
    }
}
