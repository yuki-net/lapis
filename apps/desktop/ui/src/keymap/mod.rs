use std::collections::BTreeMap;

use crate::{
    extension_ui::{CommandId, KeymapId},
    features::id,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyBindingDefinition {
    pub command: CommandId,
    pub keystroke: String,
    pub label: String,
}

impl KeyBindingDefinition {
    pub fn new(
        command: impl Into<CommandId>,
        keystroke: impl Into<String>,
        label: impl Into<String>,
    ) -> Self {
        Self {
            command: command.into(),
            keystroke: keystroke.into(),
            label: label.into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct KeymapDefinition {
    pub id: KeymapId,
    bindings: BTreeMap<CommandId, KeyBindingDefinition>,
}

impl KeymapDefinition {
    pub fn new(
        id: impl Into<KeymapId>,
        bindings: impl IntoIterator<Item = KeyBindingDefinition>,
    ) -> Self {
        Self {
            id: id.into(),
            bindings: bindings
                .into_iter()
                .map(|binding| (binding.command.clone(), binding))
                .collect(),
        }
    }

    fn binding(&self, command: &CommandId) -> Option<&KeyBindingDefinition> {
        self.bindings.get(command)
    }
}

pub struct KeymapRegistry {
    fallback: KeymapId,
    active: KeymapId,
    definitions: BTreeMap<KeymapId, KeymapDefinition>,
}

impl KeymapRegistry {
    pub fn bundled() -> Self {
        let fallback = KeymapDefinition::new(
            "lapis.default-keymap",
            [
                KeyBindingDefinition::new(id::COMMAND_NEW_DOCUMENT, "ctrl-n", "Ctrl N"),
                KeyBindingDefinition::new(id::COMMAND_OPEN_WORKSPACE, "ctrl-o", "Ctrl O"),
                KeyBindingDefinition::new(id::COMMAND_SAVE_DOCUMENT, "ctrl-s", "Ctrl S"),
                KeyBindingDefinition::new(id::COMMAND_TOGGLE_PREVIEW, "ctrl-alt-p", "Ctrl Alt P"),
                KeyBindingDefinition::new(id::COMMAND_TOGGLE_BOTTOM, "ctrl-j", "Ctrl J"),
                KeyBindingDefinition::new(
                    id::COMMAND_TOGGLE_ASSISTANT,
                    "ctrl-shift-a",
                    "Ctrl Shift A",
                ),
            ],
        );
        let id = fallback.id.clone();
        Self {
            fallback: id.clone(),
            active: id.clone(),
            definitions: [(id, fallback)].into_iter().collect(),
        }
    }

    pub fn register(&mut self, definition: KeymapDefinition) -> Result<(), KeymapId> {
        if self.definitions.contains_key(&definition.id) {
            return Err(definition.id);
        }
        self.definitions.insert(definition.id.clone(), definition);
        Ok(())
    }

    pub fn set_active(&mut self, keymap: &KeymapId) -> bool {
        if !self.definitions.contains_key(keymap) {
            return false;
        }
        self.active = keymap.clone();
        true
    }

    pub fn binding(&self, command: &CommandId) -> Option<&KeyBindingDefinition> {
        self.definitions
            .get(&self.active)
            .and_then(|keymap| keymap.binding(command))
            .or_else(|| {
                self.definitions
                    .get(&self.fallback)
                    .and_then(|keymap| keymap.binding(command))
            })
    }

    pub fn shortcut_label(&self, command: &CommandId) -> String {
        self.binding(command)
            .map(|binding| binding.label.clone())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_keymap_falls_back_per_command() {
        let mut registry = KeymapRegistry::bundled();
        registry
            .register(KeymapDefinition::new(
                "vim-like",
                [KeyBindingDefinition::new(
                    id::COMMAND_SAVE_DOCUMENT,
                    "ctrl-w",
                    "Ctrl W",
                )],
            ))
            .unwrap();
        assert!(registry.set_active(&KeymapId::new("vim-like")));
        assert_eq!(
            registry.shortcut_label(&CommandId::new(id::COMMAND_SAVE_DOCUMENT)),
            "Ctrl W"
        );
        assert_eq!(
            registry.shortcut_label(&CommandId::new(id::COMMAND_OPEN_WORKSPACE)),
            "Ctrl O"
        );
        assert_eq!(
            registry.shortcut_label(&CommandId::new("extension.unknown")),
            ""
        );
    }
}
