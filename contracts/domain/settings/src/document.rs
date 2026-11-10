//! スコープ単位の保存文書と、旧形式からの移行。

use std::collections::BTreeMap;

use localization::LocaleId;
use serde::{Deserialize, Serialize};

use crate::definition::{
    DefinitionCatalog, TERMINAL_CARET_SHAPE, TERMINAL_FONT_FAMILY, TERMINAL_FONT_SIZE,
    TERMINAL_LINE_HEIGHT, UI_LOCALE, UI_SYNTAX_THEME, UI_THEME,
};
use crate::error::SettingsError;
use crate::scope::SettingsScope;
use crate::syntax::{SyntaxTheme, TerminalCaretShape};
use crate::value::SettingValue;

/// 現在の保存形式。旧 `settings-v1.json` は [`LegacyGlobalSettings`] として読む。
pub const DOCUMENT_VERSION: u32 = 2;

/// 旧 `GlobalSettings` の保存形式。
pub const LEGACY_DOCUMENT_VERSION: u32 = 1;

/// 1つのスコープが所有する保存文書。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsDocument {
    pub version: u32,
    pub scope: SettingsScope,
    #[serde(default)]
    pub values: BTreeMap<String, SettingValue>,
}

impl SettingsDocument {
    pub fn empty(scope: SettingsScope) -> Self {
        Self {
            version: DOCUMENT_VERSION,
            scope,
            values: BTreeMap::new(),
        }
    }

    pub fn scope(&self) -> &SettingsScope {
        &self.scope
    }

    pub fn values(&self) -> &BTreeMap<String, SettingValue> {
        &self.values
    }

    pub fn get(&self, key: crate::definition::SettingKey) -> Option<&SettingValue> {
        self.values.get(key.as_str())
    }

    pub fn insert(&mut self, key: crate::definition::SettingKey, value: SettingValue) {
        self.values.insert(key.as_str().to_owned(), value);
    }

    pub fn remove(&mut self, key: crate::definition::SettingKey) -> Option<SettingValue> {
        self.values.remove(key.as_str())
    }

    /// 定義データに照らして保存内容を検証する。
    pub fn validate(&self, catalog: &DefinitionCatalog) -> Result<(), SettingsError> {
        if self.version != DOCUMENT_VERSION {
            return Err(SettingsError::new(format!(
                "未対応の設定ファイルversionです: {}",
                self.version
            )));
        }
        for (key, value) in &self.values {
            let definition = catalog
                .settings()
                .iter()
                .find(|definition| definition.key.as_str() == key)
                .ok_or_else(|| SettingsError::new(format!("未知の設定キーです: {key}")))?;
            if !definition.accepts(value) {
                return Err(SettingsError::new(format!(
                    "設定値の型が一致しません: {key}"
                )));
            }
            if !definition.allows_scope(&self.scope) {
                return Err(SettingsError::new(format!(
                    "このスコープでは保存できない設定です: {key}"
                )));
            }
        }
        Ok(())
    }

    /// 旧形式をユーザースコープの文書へ移行する。
    pub fn from_legacy(legacy: LegacyGlobalSettings) -> Self {
        let mut document = Self::empty(SettingsScope::User);
        document.insert(
            UI_LOCALE,
            SettingValue::text(legacy.locale.as_str().to_owned()),
        );
        document.insert(UI_THEME, SettingValue::text(legacy.theme));
        if let Some(theme) = legacy.syntax_theme {
            document.insert(UI_SYNTAX_THEME, SettingValue::text(theme.as_id()));
        }
        if let Some(family) = legacy.terminal_font_family {
            document.insert(TERMINAL_FONT_FAMILY, SettingValue::text(family));
        }
        if let Some(size) = legacy.terminal_font_size {
            document.insert(TERMINAL_FONT_SIZE, SettingValue::integer(i64::from(size)));
        }
        if let Some(percent) = legacy.terminal_line_height_percent {
            document.insert(
                TERMINAL_LINE_HEIGHT,
                SettingValue::integer(i64::from(percent)),
            );
        }
        if let Some(shape) = legacy.terminal_caret_shape {
            document.insert(TERMINAL_CARET_SHAPE, SettingValue::text(shape.as_id()));
        }
        document
    }
}

/// 所有者ごとの保存契約。スコープと保存先の対応は呼び出し側が決める。
pub trait SettingsRepository: Send + Sync {
    fn load(&self) -> Result<SettingsDocument, SettingsError>;
    fn save(&self, document: &SettingsDocument) -> Result<(), SettingsError>;
}

/// 旧 `GlobalSettings` の保存形式。読み込み専用で、新規保存には使わない。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyGlobalSettings {
    pub version: u32,
    pub locale: LocaleId,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub syntax_theme: Option<SyntaxTheme>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_font_family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_font_size: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_line_height_percent: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_caret_shape: Option<TerminalCaretShape>,
}

fn default_theme() -> String {
    "lapis.dark".to_owned()
}

impl Default for LegacyGlobalSettings {
    fn default() -> Self {
        Self {
            version: LEGACY_DOCUMENT_VERSION,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::catalog;

    fn document(values: &[(&str, SettingValue)]) -> SettingsDocument {
        let mut document = SettingsDocument::empty(SettingsScope::User);
        for (key, value) in values {
            let definition = catalog()
                .settings()
                .iter()
                .find(|definition| definition.key.as_str() == *key)
                .expect("定義済みキー");
            document.insert(definition.key, value.clone());
        }
        document
    }

    #[test]
    fn legacy_settings_migrate_into_user_document() {
        let legacy: LegacyGlobalSettings = serde_json::from_str(
            r#"{"version":1,"locale":"ja-JP","theme":"lapis.white","syntax_theme":"intellij.darcula",
                "terminal_font_family":"Menlo","terminal_font_size":15,
                "terminal_line_height_percent":140,"terminal_caret_shape":"bar"}"#,
        )
        .unwrap();
        let document = SettingsDocument::from_legacy(legacy);
        assert_eq!(document.version, DOCUMENT_VERSION);
        assert_eq!(document.scope(), &SettingsScope::User);
        assert_eq!(document.get(UI_LOCALE), Some(&SettingValue::text("ja-JP")));
        assert_eq!(
            document.get(UI_THEME),
            Some(&SettingValue::text("lapis.white"))
        );
        assert_eq!(
            document.get(UI_SYNTAX_THEME),
            Some(&SettingValue::text("intellij-classic"))
        );
        assert_eq!(
            document.get(TERMINAL_FONT_FAMILY),
            Some(&SettingValue::text("Menlo"))
        );
        assert_eq!(
            document.get(TERMINAL_FONT_SIZE),
            Some(&SettingValue::integer(15))
        );
        assert_eq!(
            document.get(TERMINAL_CARET_SHAPE),
            Some(&SettingValue::text("bar"))
        );
        assert_eq!(document.validate(catalog()), Ok(()));
    }

    #[test]
    fn legacy_defaults_keep_previous_behaviour() {
        let legacy: LegacyGlobalSettings =
            serde_json::from_str(r#"{"version":1,"locale":"en-US"}"#).unwrap();
        assert_eq!(legacy.theme, "lapis.dark");
        assert_eq!(legacy.syntax_theme, None);
        assert_eq!(legacy.terminal_font_size, None);
        let document = SettingsDocument::from_legacy(legacy);
        assert!(document.get(UI_SYNTAX_THEME).is_none());
        assert!(document.get(TERMINAL_FONT_SIZE).is_none());
    }

    #[test]
    fn document_round_trips_through_json() {
        let document = document(&[
            (UI_THEME.as_str(), SettingValue::text("lapis.white")),
            (TERMINAL_FONT_SIZE.as_str(), SettingValue::integer(14)),
        ]);
        let json = serde_json::to_string(&document).unwrap();
        let loaded: SettingsDocument = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded, document);
        assert!(json.contains("\"scope\":{\"kind\":\"user\"}"));
    }

    #[test]
    fn validation_rejects_unknown_keys_scope_and_types() {
        let mut unknown = SettingsDocument::empty(SettingsScope::User);
        unknown
            .values
            .insert("missing.key".into(), SettingValue::text("x"));
        assert!(unknown.validate(catalog()).is_err());

        let wrong_type = document(&[(TERMINAL_FONT_SIZE.as_str(), SettingValue::text("14"))]);
        assert!(wrong_type.validate(catalog()).is_err());

        let wrong_scopes = SettingsDocument::empty(SettingsScope::ProjectLocal {
            id: crate::scope::ProjectId::new("lapis"),
        });
        let mut wrong_scopes = SettingsDocument {
            values: document(&[(UI_LOCALE.as_str(), SettingValue::text("ja-JP"))]).values,
            ..wrong_scopes
        };
        assert!(wrong_scopes.validate(catalog()).is_err());
        wrong_scopes.values.clear();
        assert_eq!(wrong_scopes.validate(catalog()), Ok(()));
    }

    #[test]
    fn old_version_is_rejected_by_the_new_reader() {
        let document: SettingsDocument = serde_json::from_str(
            r#"{"version":1,"scope":{"kind":"user"},"values":{"ui.theme":"lapis.white"}}"#,
        )
        .unwrap();
        assert!(document.validate(catalog()).is_err());
    }
}
