use std::collections::BTreeMap;

use crate::extension_ui::{LocaleId, MessageId};

#[derive(Clone, Debug)]
pub struct LocaleDefinition {
    pub id: LocaleId,
    messages: BTreeMap<MessageId, String>,
}

impl LocaleDefinition {
    pub fn new(
        id: impl Into<LocaleId>,
        messages: impl IntoIterator<Item = (impl Into<MessageId>, impl Into<String>)>,
    ) -> Self {
        Self {
            id: id.into(),
            messages: messages
                .into_iter()
                .map(|(id, message)| (id.into(), message.into()))
                .collect(),
        }
    }

    pub fn resolve(&self, message: &MessageId) -> Option<&str> {
        self.messages.get(message).map(String::as_str)
    }
}

pub struct LocaleRegistry {
    fallback: LocaleId,
    active: LocaleId,
    definitions: BTreeMap<LocaleId, LocaleDefinition>,
}

impl LocaleRegistry {
    pub fn bundled() -> Self {
        let fallback = LocaleDefinition::new(
            "en",
            [
                ("view.files", "Files"),
                ("view.search", "Search"),
                ("view.git", "Git"),
                ("view.history", "History"),
                ("view.preview", "Preview"),
                ("view.assistant", "AI Assistant"),
                ("view.terminal", "Terminal"),
                ("view.problems", "Problems"),
                ("view.output", "Output"),
                ("view.command-search", "Search"),
                ("command.new-document", "New Document"),
                ("command.open-workspace", "Open Workspace…"),
                ("command.save-document", "Save"),
                ("command.toggle-preview", "Markdown Preview"),
                ("command.toggle-bottom-panel", "Bottom Panel"),
                ("command.toggle-assistant", "AI Assistant"),
                ("command.dev.toggle-inspector", "dev: Toggle Inspector"),
            ],
        );
        let id = fallback.id.clone();
        Self {
            fallback: id.clone(),
            active: id.clone(),
            definitions: [(id, fallback)].into_iter().collect(),
        }
    }

    pub fn register(&mut self, definition: LocaleDefinition) -> Result<(), LocaleId> {
        if self.definitions.contains_key(&definition.id) {
            return Err(definition.id);
        }
        self.definitions.insert(definition.id.clone(), definition);
        Ok(())
    }

    pub fn set_active(&mut self, locale: &LocaleId) -> bool {
        if !self.definitions.contains_key(locale) {
            return false;
        }
        self.active = locale.clone();
        true
    }

    pub fn resolve(&self, message: &MessageId) -> String {
        self.definitions
            .get(&self.active)
            .and_then(|locale| locale.resolve(message))
            .or_else(|| {
                self.definitions
                    .get(&self.fallback)
                    .and_then(|locale| locale.resolve(message))
            })
            .unwrap_or_else(|| message.as_str())
            .to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_locale_falls_back_per_message_and_unknown_ids_stay_stable() {
        let mut registry = LocaleRegistry::bundled();
        registry
            .register(LocaleDefinition::new("ja", [("view.files", "ファイル")]))
            .unwrap();
        assert!(registry.set_active(&LocaleId::new("ja")));
        assert_eq!(registry.resolve(&MessageId::new("view.files")), "ファイル");
        assert_eq!(registry.resolve(&MessageId::new("view.git")), "Git");
        assert_eq!(
            registry.resolve(&MessageId::new("extension.unknown")),
            "extension.unknown"
        );
    }
}
