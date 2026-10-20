//! ユーザー全体に適用される設定のモデルと保存契約。

use lapis_localization::LocaleId;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobalSettings {
    pub version: u32,
    pub locale: LocaleId,
    #[serde(default = "default_theme")]
    pub theme: String,
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
    }
}
