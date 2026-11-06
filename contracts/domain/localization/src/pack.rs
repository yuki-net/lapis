use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{LocaleId, MessageId};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalePackManifest {
    pub id: LocaleId,
    pub name: String,
    pub version: String,
    pub fallback: LocaleId,
}

#[derive(Clone, Debug)]
pub struct LocalePack {
    pub manifest: LocalePackManifest,
    messages: BTreeMap<MessageId, String>,
}

impl LocalePack {
    pub fn from_json(manifest: &str, documents: &[&str]) -> Result<Self, LocalePackError> {
        let manifest = serde_json::from_str(manifest)
            .map_err(|error| LocalePackError::InvalidManifest(error.to_string()))?;
        let mut messages = BTreeMap::new();
        for document in documents {
            let entries: BTreeMap<String, String> = serde_json::from_str(document)
                .map_err(|error| LocalePackError::InvalidMessages(error.to_string()))?;
            for (id, value) in entries {
                if messages.insert(MessageId::new(id.clone()), value).is_some() {
                    return Err(LocalePackError::DuplicateMessage(MessageId::new(id)));
                }
            }
        }
        Ok(Self { manifest, messages })
    }

    pub(crate) fn resolve(&self, message: &MessageId) -> Option<&str> {
        self.messages.get(message).map(String::as_str)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalePackError {
    InvalidManifest(String),
    InvalidMessages(String),
    DuplicateLocale(LocaleId),
    DuplicateMessage(MessageId),
}
