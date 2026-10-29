use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub(crate) mod gpui;

pub use self::gpui::apply_keymap_settings;

/// Preset の追加後も全 Window の Binding と表示を同時に更新する。
pub fn register_preset(cx: &mut ::gpui::App, preset: KeymapDefinition) -> Result<(), String> {
    if KeymapRegistry::active(cx)
        .definitions
        .contains_key(&preset.id)
    {
        return Err(format!("Preset は登録済みです: {}", preset.id.as_str()));
    }
    self::gpui::update_keymap(cx, |registry| {
        let _ = registry.register(preset);
    })
}

pub fn unregister_preset(cx: &mut ::gpui::App, id: &KeymapId) -> Result<(), String> {
    self::gpui::update_keymap(cx, |registry| {
        registry.unregister(id);
    })
}

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

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct KeymapDefinition {
    pub id: KeymapId,
    bindings: BTreeMap<CommandId, KeymapEntry>,
}

impl KeymapDefinition {
    pub fn new(
        id: impl Into<KeymapId>,
        bindings: impl IntoIterator<Item = KeyBindingDefinition>,
    ) -> Self {
        let mut result = Self {
            id: id.into(),
            bindings: BTreeMap::new(),
        };
        for binding in bindings {
            let entry = result
                .bindings
                .entry(binding.command)
                .or_insert_with(|| KeymapEntry::Bindings(Vec::new()));
            if let KeymapEntry::Bindings(shortcuts) = entry {
                shortcuts.push(Shortcut {
                    keystroke: binding.keystroke,
                    label: binding.label,
                });
            }
        }
        result
    }

    pub fn with_entry(mut self, command: impl Into<CommandId>, entry: KeymapEntry) -> Self {
        self.bindings.insert(command.into(), entry);
        self
    }

    fn binding(&self, command: &CommandId) -> Option<&KeymapEntry> {
        self.bindings.get(command)
    }
}

#[derive(Clone)]
pub struct KeymapRegistry {
    fallback: KeymapId,
    active: KeymapId,
    definitions: BTreeMap<KeymapId, KeymapDefinition>,
    overrides: BTreeMap<CommandId, KeymapEntry>,
}

impl ::gpui::Global for KeymapRegistry {}

impl Default for KeymapRegistry {
    fn default() -> Self {
        Self::bundled()
    }
}

/// 保存先に依存しない利用者の選択。Preset 本体は含めない。
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct KeymapSettings {
    pub preset: Option<String>,
    pub overrides: Vec<KeymapOverride>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct KeymapOverride {
    pub command: String,
    pub entry: KeymapEntry,
}

/// Entry がない場合は下位層を継承する。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", content = "bindings", rename_all = "snake_case")]
pub enum KeymapEntry {
    Unbind,
    Bindings(Vec<Shortcut>),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Shortcut {
    pub keystroke: String,
    /// 互換用の表示値。表示時は keystroke から再生成する。
    pub label: String,
}

impl KeymapRegistry {
    pub(crate) fn binding_layer(&self, command: &CommandId) -> u8 {
        if self.overrides.contains_key(command) {
            2
        } else if self.active != self.fallback
            && self
                .definitions
                .get(&self.active)
                .and_then(|map| map.binding(command))
                .is_some()
        {
            1
        } else {
            0
        }
    }
    pub fn active(cx: &mut ::gpui::App) -> &Self {
        if !cx.has_global::<Self>() {
            cx.set_global(Self::default());
        }
        cx.global::<Self>()
    }
    pub(crate) fn default_binding(&self, command: &CommandId) -> Option<&KeymapEntry> {
        self.definitions
            .get(&self.fallback)
            .and_then(|map| map.binding(command))
    }

    pub fn bundled() -> Self {
        let fallback = KeymapDefinition::new(
            "lapis.default-keymap",
            [
                KeyBindingDefinition::new(id::COMMAND_NEW_DOCUMENT, "ctrl-n", "Ctrl N"),
                KeyBindingDefinition::new(id::COMMAND_OPEN_DOCUMENT, "ctrl-o", "Ctrl O"),
                KeyBindingDefinition::new(id::COMMAND_SAVE_AS, "ctrl-shift-s", "Ctrl Shift S"),
                KeyBindingDefinition::new(id::COMMAND_UNDO, "ctrl-z", "Ctrl Z"),
                KeyBindingDefinition::new(id::COMMAND_REDO, "ctrl-y", "Ctrl Y"),
                KeyBindingDefinition::new(id::COMMAND_FIND, "ctrl-f", "Ctrl F"),
                KeyBindingDefinition::new(
                    id::COMMAND_FIND_WORKSPACE,
                    "ctrl-shift-f",
                    "Ctrl Shift F",
                ),
                KeyBindingDefinition::new(id::COMMAND_FIND_NEXT, "f3", "F3"),
                KeyBindingDefinition::new(id::COMMAND_COMPLETE, "ctrl-space", "Ctrl Space"),
                KeyBindingDefinition::new(id::COMMAND_GO_TO_DEFINITION, "f12", "F12"),
                KeyBindingDefinition::new(
                    id::COMMAND_SHOW_COMMANDS,
                    "ctrl-shift-k",
                    "Ctrl Shift K",
                ),
                KeyBindingDefinition::new(id::COMMAND_QUIT, "ctrl-q", "Ctrl Q"),
                KeyBindingDefinition::new(id::COMMAND_SAVE_DOCUMENT, "ctrl-s", "Ctrl S"),
                KeyBindingDefinition::new(id::COMMAND_TOGGLE_PREVIEW, "ctrl-alt-p", "Ctrl Alt P"),
                KeyBindingDefinition::new(id::COMMAND_TOGGLE_BOTTOM_ZONE, "ctrl-j", "Ctrl J"),
                KeyBindingDefinition::new(id::COMMAND_TOGGLE_LEFT_ZONE, "ctrl-alt-l", "Ctrl Alt L"),
                KeyBindingDefinition::new(
                    id::COMMAND_TOGGLE_RIGHT_ZONE,
                    "ctrl-alt-r",
                    "Ctrl Alt R",
                ),
                KeyBindingDefinition::new(
                    id::COMMAND_TOGGLE_ASSISTANT,
                    "ctrl-shift-a",
                    "Ctrl Shift A",
                ),
                KeyBindingDefinition::new(id::COMMAND_OPEN_DOCUMENT, "cmd-o", "Cmd O"),
                KeyBindingDefinition::new(id::COMMAND_SAVE_DOCUMENT, "cmd-s", "Cmd S"),
                KeyBindingDefinition::new(id::COMMAND_NEW_DOCUMENT, "cmd-n", "Cmd N"),
                KeyBindingDefinition::new(id::COMMAND_UNDO, "cmd-z", "Cmd Z"),
                KeyBindingDefinition::new(id::COMMAND_REDO, "cmd-shift-z", "Cmd Shift Z"),
                KeyBindingDefinition::new(id::COMMAND_FIND, "cmd-f", "Cmd F"),
                KeyBindingDefinition::new(id::COMMAND_FIND_NEXT, "cmd-g", "Cmd G"),
                KeyBindingDefinition::new(id::COMMAND_SHOW_COMMANDS, "cmd-shift-k", "Cmd Shift K"),
                KeyBindingDefinition::new(id::COMMAND_QUIT, "cmd-q", "Cmd Q"),
            ],
        );
        let id = fallback.id.clone();
        Self {
            fallback: id.clone(),
            active: id.clone(),
            definitions: [(id, fallback)].into_iter().collect(),
            overrides: BTreeMap::new(),
        }
    }

    fn register(&mut self, definition: KeymapDefinition) -> Result<(), KeymapId> {
        if self.definitions.contains_key(&definition.id) {
            return Err(definition.id);
        }
        self.definitions.insert(definition.id.clone(), definition);
        Ok(())
    }

    #[cfg(test)]
    fn set_active(&mut self, keymap: &KeymapId) -> bool {
        if !self.definitions.contains_key(keymap) {
            return false;
        }
        self.active = keymap.clone();
        true
    }

    pub fn bindings_for(&self, command: &CommandId) -> &[Shortcut] {
        let entry = self
            .overrides
            .get(command)
            .or_else(|| {
                self.definitions
                    .get(&self.active)
                    .and_then(|map| map.binding(command))
            })
            .or_else(|| self.default_binding(command));
        match entry {
            Some(KeymapEntry::Bindings(bindings)) => bindings,
            _ => &[],
        }
    }

    pub fn shortcut_for(&self, command: &CommandId) -> Option<&Shortcut> {
        if self.default_binding(command).is_none()
            && !crate::app::EXTRA_COMMANDS.contains(&command.as_str())
        {
            return None;
        }
        self.bindings_for(command).first()
    }

    /// 未登録の Preset は既定に戻し、override は保持する。
    pub(crate) fn apply_settings(&mut self, settings: &KeymapSettings) {
        self.active = settings
            .preset
            .as_deref()
            .map(KeymapId::new)
            .filter(|id| self.definitions.contains_key(id))
            .unwrap_or_else(|| self.fallback.clone());
        self.overrides = settings
            .overrides
            .iter()
            .map(|entry| (CommandId::new(&entry.command), entry.entry.clone()))
            .collect();
    }

    fn unregister(&mut self, id: &KeymapId) -> bool {
        if *id == self.fallback {
            return false;
        }
        let removed = self.definitions.remove(id).is_some();
        if self.active == *id {
            self.active = self.fallback.clone();
        }
        removed
    }

    pub fn shortcut_label(&self, command: &CommandId) -> String {
        self.shortcut_for(command)
            .map(|binding| {
                binding
                    .keystroke
                    .split_whitespace()
                    .map(|stroke| {
                        stroke
                            .split('-')
                            .map(|part| match part {
                                "ctrl" => "Ctrl".to_owned(),
                                "alt" => "Alt".to_owned(),
                                "shift" => "Shift".to_owned(),
                                "cmd" => "Cmd".to_owned(),
                                _ if part.len() == 1
                                    || part.starts_with('f')
                                        && part[1..].chars().all(|c| c.is_ascii_digit()) =>
                                {
                                    part.to_uppercase()
                                }
                                _ => {
                                    let mut chars = part.chars();
                                    chars
                                        .next()
                                        .map(|c| {
                                            c.to_uppercase().collect::<String>() + chars.as_str()
                                        })
                                        .unwrap_or_default()
                                }
                            })
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shortcut(key: &str) -> Shortcut {
        Shortcut {
            keystroke: key.into(),
            label: String::new(),
        }
    }

    #[test]
    fn layers_replace_or_unbind_all_shortcuts_and_serialize() {
        let save: CommandId = id::COMMAND_SAVE_DOCUMENT.into();
        let mut registry = KeymapRegistry::bundled();
        let keys = |registry: &KeymapRegistry| {
            registry
                .bindings_for(&save)
                .iter()
                .map(|s| s.keystroke.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(keys(&registry), ["ctrl-s", "cmd-s"]);
        assert_eq!(registry.shortcut_for(&save).unwrap().keystroke, "ctrl-s");
        let preset = KeymapDefinition::new("extension.preset", [])
            .with_entry(
                save.clone(),
                KeymapEntry::Bindings(vec![shortcut("ctrl-alt-s"), shortcut("cmd-alt-s")]),
            )
            .with_entry(id::COMMAND_NEW_DOCUMENT, KeymapEntry::Unbind);
        let preset: KeymapDefinition =
            serde_json::from_str(&serde_json::to_string(&preset).unwrap()).unwrap();
        registry.register(preset).unwrap();
        let mut settings = KeymapSettings {
            preset: Some("extension.preset".into()),
            overrides: vec![],
        };
        registry.apply_settings(&settings);
        assert_eq!(keys(&registry), ["ctrl-alt-s", "cmd-alt-s"]);
        assert!(
            registry
                .bindings_for(&id::COMMAND_NEW_DOCUMENT.into())
                .is_empty()
        );
        assert_eq!(registry.shortcut_label(&save), "Ctrl Alt S");
        assert_eq!(
            registry
                .bindings_for(&id::COMMAND_OPEN_DOCUMENT.into())
                .len(),
            2
        );
        settings.overrides.push(KeymapOverride {
            command: save.as_str().into(),
            entry: KeymapEntry::Bindings(vec![shortcut("ctrl-shift-s"), shortcut("cmd-shift-s")]),
        });
        let settings: KeymapSettings =
            serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        registry.apply_settings(&settings);
        assert_eq!(keys(&registry), ["ctrl-shift-s", "cmd-shift-s"]);
        assert_eq!(registry.shortcut_label(&save), "Ctrl Shift S");
        let mut settings = settings;
        settings.overrides[0].entry = KeymapEntry::Unbind;
        let settings: KeymapSettings =
            serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        registry.apply_settings(&settings);
        assert!(keys(&registry).is_empty());
    }

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
            registry.shortcut_label(&CommandId::new(id::COMMAND_OPEN_DOCUMENT)),
            "Ctrl O"
        );
        assert_eq!(
            registry.shortcut_label(&CommandId::new("extension.unknown")),
            ""
        );
    }

    #[test]
    fn settings_override_and_unbind_survive_preset_removal() {
        let mut registry = KeymapRegistry::bundled();
        let preset = KeymapId::new("extension.preset");
        registry
            .register(KeymapDefinition::new(
                preset.clone(),
                [KeyBindingDefinition::new(
                    id::COMMAND_SAVE_DOCUMENT,
                    "ctrl-p",
                    "Ctrl P",
                )],
            ))
            .unwrap();
        let settings = KeymapSettings {
            preset: Some(preset.as_str().to_owned()),
            overrides: vec![
                KeymapOverride {
                    command: id::COMMAND_SAVE_DOCUMENT.into(),
                    entry: KeymapEntry::Bindings(vec![Shortcut {
                        keystroke: "ctrl-w".into(),
                        label: "Ctrl W".into(),
                    }]),
                },
                KeymapOverride {
                    command: id::COMMAND_OPEN_DOCUMENT.into(),
                    entry: KeymapEntry::Unbind,
                },
            ],
        };
        let serialized = serde_json::to_string(&settings).unwrap();
        registry.apply_settings(&serde_json::from_str(&serialized).unwrap());
        assert_eq!(
            registry.shortcut_label(&id::COMMAND_SAVE_DOCUMENT.into()),
            "Ctrl W"
        );
        assert_eq!(
            registry.shortcut_label(&id::COMMAND_OPEN_DOCUMENT.into()),
            ""
        );
        assert!(registry.unregister(&preset));
        assert_eq!(
            registry.shortcut_label(&id::COMMAND_SAVE_DOCUMENT.into()),
            "Ctrl W"
        );
        assert_eq!(
            registry.shortcut_label(&id::COMMAND_OPEN_DOCUMENT.into()),
            ""
        );
        registry.apply_settings(&settings);
        assert_eq!(
            registry.shortcut_label(&id::COMMAND_SAVE_DOCUMENT.into()),
            "Ctrl W"
        );
    }

    #[test]
    fn unavailable_command_shortcut_is_not_presented_as_active() {
        let mut registry = KeymapRegistry::bundled();
        registry.apply_settings(&KeymapSettings {
            preset: Some("missing".into()),
            overrides: vec![KeymapOverride {
                command: "extension.missing".into(),
                entry: KeymapEntry::Bindings(vec![Shortcut {
                    keystroke: "ctrl-k".into(),
                    label: "Ctrl K".into(),
                }]),
            }],
        });
        assert_eq!(registry.shortcut_label(&"extension.missing".into()), "");
        assert_eq!(
            registry.shortcut_label(&id::COMMAND_SAVE_DOCUMENT.into()),
            "Ctrl S"
        );
    }

    #[test]
    fn lookup_formats_the_actual_keystroke() {
        let mut registry = KeymapRegistry::bundled();
        registry.apply_settings(&KeymapSettings {
            preset: None,
            overrides: vec![KeymapOverride {
                command: id::COMMAND_SAVE_DOCUMENT.into(),
                entry: KeymapEntry::Bindings(vec![Shortcut {
                    keystroke: "ctrl-alt-s".into(),
                    label: "F12".into(),
                }]),
            }],
        });
        assert_eq!(
            registry.shortcut_label(&id::COMMAND_SAVE_DOCUMENT.into()),
            "Ctrl Alt S"
        );
    }
}
