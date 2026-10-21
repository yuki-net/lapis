//! ユーザー全体に適用される設定のモデルと保存契約。

use lapis_localization::LocaleId;
use serde::{Deserialize, Serialize};

/// 構文配色の系統。明暗はUIテーマに追従する。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SyntaxTheme {
    #[serde(rename = "vscode", alias = "vscode.dark-plus")]
    VsCode,
    #[serde(alias = "zed.one-dark")]
    Zed,
    #[serde(alias = "intellij.dark", alias = "intellij.darcula")]
    Intellij,
    #[default]
    #[serde(alias = "lapis.dark", alias = "lapis.light", other)]
    Lapis,
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
    fn syntax_selection_round_trips_independently_of_ui_theme() {
        let old: GlobalSettings =
            serde_json::from_str(r#"{"version":1,"locale":"en-US","theme":"lapis.white"}"#)
                .unwrap();
        assert!(old.syntax_theme.is_none());
        let selected = GlobalSettings {
            syntax_theme: Some(SyntaxTheme::Intellij),
            ..old
        };
        let loaded: GlobalSettings =
            serde_json::from_str(&serde_json::to_string(&selected).unwrap()).unwrap();
        assert_eq!(loaded, selected);
        assert_eq!(loaded.theme, "lapis.white");
    }

    #[test]
    fn previous_palette_ids_migrate_to_families() {
        for (old, family, canonical) in [
            ("lapis.dark", SyntaxTheme::Lapis, "lapis"),
            ("lapis.light", SyntaxTheme::Lapis, "lapis"),
            ("vscode.dark-plus", SyntaxTheme::VsCode, "vscode"),
            ("zed.one-dark", SyntaxTheme::Zed, "zed"),
            ("intellij.dark", SyntaxTheme::Intellij, "intellij"),
            ("intellij.darcula", SyntaxTheme::Intellij, "intellij"),
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
