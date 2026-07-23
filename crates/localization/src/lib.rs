//! UI 文言の解決と言語パックの検証を担う。UI・設定保存・取得元には依存しない。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LocaleId(String);

impl LocaleId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for LocaleId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for LocaleId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MessageId(String);

impl MessageId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for MessageId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for MessageId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanguagePackManifest {
    pub id: LocaleId,
    pub name: String,
    pub version: String,
    pub fallback: LocaleId,
}

#[derive(Clone, Debug)]
pub struct LanguagePack {
    pub manifest: LanguagePackManifest,
    messages: BTreeMap<MessageId, String>,
}

impl LanguagePack {
    pub fn new(
        manifest: LanguagePackManifest,
        messages: impl IntoIterator<Item = (impl Into<MessageId>, impl Into<String>)>,
    ) -> Self {
        Self {
            manifest,
            messages: messages
                .into_iter()
                .map(|(id, value)| (id.into(), value.into()))
                .collect(),
        }
    }

    pub fn from_json(
        manifest: &str,
        message_documents: &[&str],
    ) -> Result<Self, LanguagePackError> {
        let manifest = serde_json::from_str(manifest)
            .map_err(|error| LanguagePackError::InvalidManifest(error.to_string()))?;
        let mut messages = BTreeMap::new();
        for document in message_documents {
            let entries: BTreeMap<String, String> = serde_json::from_str(document)
                .map_err(|error| LanguagePackError::InvalidMessages(error.to_string()))?;
            for (id, value) in entries {
                if messages.insert(MessageId::new(id.clone()), value).is_some() {
                    return Err(LanguagePackError::DuplicateMessage(MessageId::new(id)));
                }
            }
        }
        Ok(Self { manifest, messages })
    }

    fn resolve(&self, message: &MessageId) -> Option<&str> {
        self.messages.get(message).map(String::as_str)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LanguagePackError {
    InvalidManifest(String),
    InvalidMessages(String),
    DuplicateLocale(LocaleId),
    DuplicateMessage(MessageId),
}

/// 言語パックを登録し、選択中ロケールと英語フォールバックから文言を解決する。
pub struct Localizer {
    fallback: LocaleId,
    active: LocaleId,
    packs: BTreeMap<LocaleId, LanguagePack>,
}

impl Localizer {
    pub fn bundled() -> Self {
        let mut localizer = Self::new(LocaleId::new("en-US"));
        for pack in [
            LanguagePack::from_json(
                include_str!("../../../locales/en-US/manifest.json"),
                &[include_str!("../../../locales/en-US/messages/common.json")],
            ),
            LanguagePack::from_json(
                include_str!("../../../locales/ja-JP/manifest.json"),
                &[include_str!("../../../locales/ja-JP/messages/common.json")],
            ),
        ] {
            localizer
                .register(pack.expect("bundled language pack must be valid"))
                .expect("bundled locales must be unique");
        }
        localizer
    }

    pub fn new(fallback: LocaleId) -> Self {
        Self {
            active: fallback.clone(),
            fallback,
            packs: BTreeMap::new(),
        }
    }

    pub fn register(&mut self, pack: LanguagePack) -> Result<(), LanguagePackError> {
        let id = pack.manifest.id.clone();
        if self.packs.contains_key(&id) {
            return Err(LanguagePackError::DuplicateLocale(id));
        }
        self.packs.insert(id, pack);
        Ok(())
    }

    pub fn active(&self) -> &LocaleId {
        &self.active
    }

    pub fn set_active(&mut self, locale: &LocaleId) -> bool {
        if !self.packs.contains_key(locale) {
            return false;
        }
        self.active = locale.clone();
        true
    }

    pub fn resolve(&self, message: &MessageId) -> String {
        self.packs
            .get(&self.active)
            .and_then(|pack| pack.resolve(message))
            .or_else(|| {
                self.packs
                    .get(&self.fallback)
                    .and_then(|pack| pack.resolve(message))
            })
            .unwrap_or_else(|| message.as_str())
            .to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_locale_falls_back_per_message() {
        let mut localizer = Localizer::bundled();
        assert!(localizer.set_active(&LocaleId::new("ja-JP")));
        assert_eq!(
            localizer.resolve(&MessageId::new("command.save-document")),
            "保存"
        );
        assert_eq!(
            localizer.resolve(&MessageId::new("extension.unknown")),
            "extension.unknown"
        );
    }
}
