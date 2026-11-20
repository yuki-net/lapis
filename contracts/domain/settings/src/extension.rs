//! 拡張が提供する設定定義。保存値とは別に、起動中の定義だけを所有する。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{ScopeKind, SettingValue, SettingsError, SettingsScope, ValueKind};

/// 拡張が所有する1つの設定。値は `extensions/<拡張ID>/<名前>` で保存する。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionSettingDefinition {
    pub name: String,
    pub value_kind: ValueKind,
    pub scopes: Vec<ScopeKind>,
    pub default: Option<SettingValue>,
    /// 指定時は、同じ型の候補のみを保存可能にする。
    pub allowed_values: Option<Vec<SettingValue>>,
}

impl ExtensionSettingDefinition {
    pub fn accepts(&self, value: &SettingValue) -> bool {
        value.kind() == self.value_kind
            && self
                .allowed_values
                .as_ref()
                .is_none_or(|values| values.contains(value))
    }

    pub fn allows_scope(&self, scope: &SettingsScope) -> bool {
        self.scopes.contains(&scope.kind())
    }
}

/// 登録の単位は拡張。無効化時は定義だけを外し、文書の値を残す。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExtensionSettingsCatalog {
    definitions: BTreeMap<String, ExtensionSettingDefinition>,
    owners: BTreeMap<String, Vec<String>>,
}

impl ExtensionSettingsCatalog {
    /// 検証に失敗した登録は一切反映しない。
    pub fn register(
        &mut self,
        owner: &str,
        definitions: Vec<ExtensionSettingDefinition>,
    ) -> Result<(), SettingsError> {
        if !valid_owner(owner) || self.owners.contains_key(owner) {
            return Err(SettingsError::new(format!(
                "不正または登録済みの拡張 ID: {owner}"
            )));
        }
        let mut keys = BTreeSet::new();
        for definition in &definitions {
            if !valid_name(&definition.name) || definition.scopes.is_empty() {
                return Err(SettingsError::new(format!(
                    "不正な拡張設定の定義: {}",
                    definition.name
                )));
            }
            if let Some(values) = &definition.allowed_values
                && (values.is_empty()
                    || values
                        .iter()
                        .any(|value| value.kind() != definition.value_kind))
            {
                return Err(SettingsError::new(format!(
                    "不正な拡張設定の候補: {}",
                    definition.name
                )));
            }
            if definition
                .default
                .as_ref()
                .is_some_and(|value| !definition.accepts(value))
            {
                return Err(SettingsError::new(format!(
                    "不正な拡張設定の既定値: {}",
                    definition.name
                )));
            }
            let key = format!("extensions/{owner}/{}", definition.name);
            if self.definitions.contains_key(&key) || !keys.insert(key) {
                return Err(SettingsError::new(format!(
                    "重複する拡張設定: {}",
                    definition.name
                )));
            }
        }
        let keys: Vec<_> = keys.into_iter().collect();
        for definition in definitions {
            self.definitions.insert(
                format!("extensions/{owner}/{}", definition.name),
                definition,
            );
        }
        self.owners.insert(owner.to_owned(), keys);
        Ok(())
    }

    pub fn unregister(&mut self, owner: &str) -> bool {
        let Some(keys) = self.owners.remove(owner) else {
            return false;
        };
        for key in keys {
            self.definitions.remove(&key);
        }
        true
    }

    pub fn definition(&self, key: &str) -> Option<&ExtensionSettingDefinition> {
        self.definitions.get(key)
    }

    pub fn definitions(&self) -> impl Iterator<Item = (&str, &ExtensionSettingDefinition)> {
        self.definitions
            .iter()
            .map(|(key, value)| (key.as_str(), value))
    }
}

/// 定義が現在ない場合でも、拡張無効中の保存値として保持できる名前空間。
pub(crate) fn is_extension_key(key: &str) -> bool {
    let mut parts = key.split('/');
    matches!((parts.next(), parts.next(), parts.next(), parts.next()),
        (Some("extensions"), Some(owner), Some(name), None) if valid_owner(owner) && valid_name(name))
}

fn valid_owner(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && !value.ends_with('.')
        && value
            .bytes()
            .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'-' | b'_'))
}

fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'-' | b'_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn definition(name: &str) -> ExtensionSettingDefinition {
        ExtensionSettingDefinition {
            name: name.into(),
            value_kind: ValueKind::Text,
            scopes: vec![ScopeKind::User],
            default: Some(SettingValue::text("a")),
            allowed_values: Some(vec![SettingValue::text("a"), SettingValue::text("b")]),
        }
    }

    #[test]
    fn duplicate_or_invalid_registration_does_not_partially_change_catalog() {
        let mut catalog = ExtensionSettingsCatalog::default();
        assert!(
            catalog
                .register("dev.example", vec![definition("one"), definition("one")])
                .is_err()
        );
        assert_eq!(catalog.definitions().count(), 0);
        let mut invalid = definition("two");
        invalid.default = Some(SettingValue::text("not-allowed"));
        assert!(
            catalog
                .register("dev.example", vec![definition("one"), invalid])
                .is_err()
        );
        assert_eq!(catalog.definitions().count(), 0);
        catalog
            .register("dev.example", vec![definition("one")])
            .unwrap();
        assert!(
            catalog
                .definition("extensions/dev.example/one")
                .unwrap()
                .accepts(&SettingValue::text("b"))
        );
        assert!(
            !catalog
                .definition("extensions/dev.example/one")
                .unwrap()
                .accepts(&SettingValue::text("c"))
        );
        assert!(
            catalog
                .register("dev.example", vec![definition("other")])
                .is_err()
        );
        assert!(catalog.unregister("dev.example"));
        assert_eq!(catalog.definitions().count(), 0);
    }

    #[test]
    fn inactive_key_namespace_is_unambiguous() {
        assert!(is_extension_key("extensions/dev.example/setting.name"));
        assert!(is_extension_key("extensions/.example/setting"));
        assert!(!is_extension_key("extensions/../setting"));
        assert!(!is_extension_key("extensions/dev.example/../setting"));
        assert!(!is_extension_key("unknown.key"));
        assert!(!is_extension_key("extensions/dev.example/"));
    }
}
