//! 署名済み package の静的な設定・翻訳宣言。
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use settings::{ExtensionSettingDefinition, ExtensionSettingsCatalog};

pub const STATIC_CONTRIBUTIONS_CONTRACT: &str = "lapis.extension.contributions";
pub const STATIC_CONTRIBUTIONS_VERSION: &str = "1.0.0";

/// 提供元 ID は Host が署名済み manifest から付ける。宣言内では指定できない。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StaticContributions {
    pub schema: u32,
    /// 自身の文言に必要な既定言語。省略時は en-US。
    #[serde(default = "default_locale")]
    pub default_locale: String,
    #[serde(default)]
    pub settings: Vec<ContributedSetting>,
    #[serde(default)]
    pub translations: Vec<ContributedTranslation>,
    /// 提供元拡張の名前空間に属する、クライアント向けテーマ定義。
    #[serde(default)]
    pub themes: Vec<ContributedTheme>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContributedTheme {
    pub id: String,
    pub definition: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContributedSetting {
    pub definition: ExtensionSettingDefinition,
    /// 提供元拡張の翻訳メッセージ ID。
    pub label: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContributedTranslation {
    /// 翻訳対象の拡張。提供元は署名済み manifest から別途決まる。
    pub target: String,
    pub locale: String,
    pub messages: BTreeMap<String, String>,
}

fn default_locale() -> String {
    "en-US".to_owned()
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'_'))
}

impl StaticContributions {
    /// 認証済みの提供元に設定を束縛し、表示に必要な既定文言も検証する。
    pub fn validate(&self, owner: &str) -> Result<(), String> {
        if owner == "lapis.core" || self.schema != 1 || !valid_identifier(&self.default_locale) {
            return Err("未対応の宣言 schema または不正な既定言語".into());
        }
        let mut catalog = ExtensionSettingsCatalog::default();
        catalog
            .register(
                owner,
                self.settings.iter().map(|s| s.definition.clone()).collect(),
            )
            .map_err(|error| error.to_string())?;
        let mut theme_ids = BTreeSet::new();
        for theme in &self.themes {
            if !valid_identifier(&theme.id)
                || !theme.id.starts_with(&format!("{owner}."))
                || theme.id.len() <= owner.len() + 1
                || !theme_ids.insert(&theme.id)
                || theme
                    .definition
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    != Some(theme.id.as_str())
                || theme
                    .definition
                    .get("version")
                    .and_then(serde_json::Value::as_u64)
                    != Some(1)
            {
                return Err("テーマ宣言が不正または重複しています".into());
            }
        }
        let mut languages = BTreeSet::new();
        for translation in &self.translations {
            if !valid_identifier(&translation.target)
                || !valid_identifier(&translation.locale)
                || (translation.target == "lapis.core"
                    && translation.locale.eq_ignore_ascii_case("en-US"))
                || !languages.insert((&translation.target, &translation.locale))
                || translation
                    .messages
                    .iter()
                    .any(|(key, value)| !valid_identifier(key) || value.is_empty())
            {
                return Err("不正または重複した翻訳宣言".into());
            }
        }
        let fallback = self
            .translations
            .iter()
            .find(|t| t.target == owner && t.locale == self.default_locale);
        for setting in &self.settings {
            if !valid_identifier(&setting.label)
                || setting
                    .description
                    .as_ref()
                    .is_some_and(|d| !valid_identifier(d))
                || !fallback.is_some_and(|t| {
                    t.messages.contains_key(&setting.label)
                        && setting
                            .description
                            .as_ref()
                            .is_none_or(|d| t.messages.contains_key(d))
                })
            {
                return Err("設定の既定文言がありません".into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_owner_controls_settings_namespace_and_fallback() {
        let payload = r#"{"schema":1,"settings":[{"definition":{"name":"font-size","value_kind":"integer","scopes":["user"],"default":12,"allowed_values":null},"label":"terminal.font"}],"translations":[{"target":"dev.example.terminal","locale":"en-US","messages":{"terminal.font":"Font"}}]}"#;
        let value: StaticContributions = serde_json::from_str(payload).unwrap();
        assert!(value.validate("dev.example.terminal").is_ok());
        assert!(value.validate("dev.other").is_err());
        assert_eq!(
            serde_json::from_slice::<StaticContributions>(&serde_json::to_vec(&value).unwrap())
                .unwrap(),
            value
        );
        assert!(
            serde_json::from_str::<StaticContributions>(r#"{"schema":1,"owner":"forged"}"#)
                .is_err()
        );
    }

    #[test]
    fn core_english_is_reserved_but_other_core_translations_are_allowed() {
        let english = r#"{"schema":1,"translations":[{"target":"lapis.core","locale":"en-US","messages":{"title":"Title"}}]}"#;
        let mut declaration: StaticContributions = serde_json::from_str(english).unwrap();
        assert!(declaration.validate("dev.example.language").is_err());
        declaration.translations[0].locale = "ja-JP".to_owned();
        assert!(declaration.validate("dev.example.language").is_ok());
        assert!(declaration.validate("lapis.core").is_err());
    }

    #[test]
    fn theme_ids_belong_to_the_signed_extension() {
        let source = r#"{"schema":1,"themes":[{"id":"dev.example.night","definition":{"version":1,"id":"dev.example.night"}}]}"#;
        let mut declaration: StaticContributions = serde_json::from_str(source).unwrap();
        assert!(declaration.validate("dev.example").is_ok());
        assert!(declaration.validate("dev.other").is_err());
        declaration.themes[0].definition["id"] = "lapis.dark".into();
        assert!(declaration.validate("dev.example").is_err());
    }

    #[test]
    fn bundled_japanese_package_has_valid_core_translations() {
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../extensions/language-ja/contributions.json");
        let declaration: StaticContributions =
            serde_json::from_slice(&std::fs::read(source).unwrap()).unwrap();
        declaration.validate("lapis.language.ja").unwrap();
        assert!(declaration.translations.iter().any(|translation| {
            translation.target == "lapis.core" && translation.locale == "ja-JP"
        }));
    }
}
