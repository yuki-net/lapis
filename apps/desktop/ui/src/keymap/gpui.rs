use crate::app::*;
use crate::features::command_search::{
    SearchBackspace, SearchConfirm, SearchDelete, SearchDismiss, SearchEnd, SearchHome, SearchLeft,
    SearchNext, SearchPrevious, SearchRight, SearchSelectAll,
};
use ::gpui::{App, KeyBinding};

/// GPUI の version を記録し、他の登録元の Binding を消さない。
struct BindingVersion(gpui::KeymapVersion);
impl gpui::Global for BindingVersion {}

pub(crate) fn bind_keys(cx: &mut App) {
    let registry = cx.default_global::<crate::keymap::KeymapRegistry>().clone();
    let bindings = resolved_bindings(&registry).expect("組み込み Keymap は有効");
    cx.bind_keys(bindings);
    let version = cx.key_bindings().borrow().version();
    cx.set_global(BindingVersion(version));
}

/// 設定を検証してから、内部入力を含む Binding を再登録する。
pub fn apply_keymap_settings(
    cx: &mut App,
    settings: &crate::keymap::KeymapSettings,
) -> Result<(), String> {
    update_keymap(cx, |registry| registry.apply_settings(settings))
}

pub(crate) fn update_keymap(
    cx: &mut App,
    update: impl FnOnce(&mut crate::keymap::KeymapRegistry),
) -> Result<(), String> {
    let mut registry = cx.default_global::<crate::keymap::KeymapRegistry>().clone();
    update(&mut registry);
    let bindings = resolved_bindings(&registry)?;
    let current = cx.key_bindings().borrow().version();
    if cx
        .try_global::<BindingVersion>()
        .is_none_or(|version| version.0 != current)
    {
        return Err("他の登録元が Key Binding を変更したため再適用できません".into());
    }
    cx.clear_key_bindings();
    cx.bind_keys(bindings);
    let version = cx.key_bindings().borrow().version();
    cx.set_global(BindingVersion(version));
    cx.set_global(registry);
    Ok(())
}

fn resolved_bindings(registry: &crate::keymap::KeymapRegistry) -> Result<Vec<KeyBinding>, String> {
    let mut bindings = Vec::new();
    let mut presets = Vec::new();
    let mut overrides = Vec::new();
    let mut split_defaults = std::collections::HashSet::new();
    macro_rules! command_binding {
        ($primary:expr, $action:expr, $context:expr, $id:expr) => {{
            let id = crate::extension_ui::CommandId::new($id);
            let resolved = registry.bindings_for(&id);
            let layer = registry.binding_layer(&id);
            if !$primary { split_defaults.insert(id.clone()); }
            if layer == 0 {
                let defaults = if $primary {
                    if split_defaults.contains(&id) { &resolved[..resolved.len().min(1)] } else { resolved }
                } else {
                    &resolved[resolved.len().min(1)..]
                };
                for binding in defaults { bindings.push(KeyBinding::new(&binding.keystroke, $action, $context)); }
            } else if $primary {
                for binding in resolved {
                    if binding.keystroke.trim().is_empty()
                        || binding
                            .keystroke
                            .split_whitespace()
                            .any(|key| gpui::Keystroke::parse(key).is_err())
                    {
                        return Err(format!("無効なショートカット: {}", binding.keystroke));
                    }
                    let target = if layer == 1 { &mut presets } else { &mut overrides };
                    target.push(KeyBinding::new(&binding.keystroke, $action, $context));
                }
            }
        }};
    }
    bindings.push(KeyBinding::new("backspace", Backspace, Some("Editor")));
    bindings.push(KeyBinding::new("delete", Delete, Some("Editor")));
    bindings.push(KeyBinding::new("left", Left, Some("Editor")));
    bindings.push(KeyBinding::new("right", Right, Some("Editor")));
    bindings.push(KeyBinding::new("shift-left", SelectLeft, Some("Editor")));
    bindings.push(KeyBinding::new("shift-right", SelectRight, Some("Editor")));
    bindings.push(KeyBinding::new("cmd-a", SelectAll, Some("Editor")));
    bindings.push(KeyBinding::new("ctrl-a", SelectAll, Some("Editor")));
    bindings.push(KeyBinding::new("cmd-v", Paste, Some("Editor")));
    bindings.push(KeyBinding::new("ctrl-v", Paste, Some("Editor")));
    bindings.push(KeyBinding::new("cmd-c", Copy, Some("Editor")));
    bindings.push(KeyBinding::new("ctrl-c", Copy, Some("Editor")));
    bindings.push(KeyBinding::new("cmd-x", Cut, Some("Editor")));
    bindings.push(KeyBinding::new("ctrl-x", Cut, Some("Editor")));
    bindings.push(KeyBinding::new("home", Home, Some("Editor")));
    bindings.push(KeyBinding::new("end", End, Some("Editor")));
    bindings.push(KeyBinding::new("enter", Enter, Some("Editor")));
    let editor_input_end = bindings.len();
    command_binding!(
        false,
        Open,
        None,
        crate::features::id::COMMAND_OPEN_DOCUMENT
    );
    command_binding!(
        true,
        Open,
        Some("!Terminal"),
        crate::features::id::COMMAND_OPEN_DOCUMENT
    );
    command_binding!(
        false,
        Save,
        None,
        crate::features::id::COMMAND_SAVE_DOCUMENT
    );
    command_binding!(
        true,
        Save,
        Some("!Terminal"),
        crate::features::id::COMMAND_SAVE_DOCUMENT
    );
    command_binding!(
        true,
        SaveAs,
        Some("!Terminal"),
        crate::features::id::COMMAND_SAVE_AS
    );
    command_binding!(false, New, None, crate::features::id::COMMAND_NEW_DOCUMENT);
    command_binding!(
        true,
        New,
        Some("!Terminal"),
        crate::features::id::COMMAND_NEW_DOCUMENT
    );
    command_binding!(
        false,
        Undo,
        Some("Editor"),
        crate::features::id::COMMAND_UNDO
    );
    command_binding!(
        true,
        Undo,
        Some("Editor"),
        crate::features::id::COMMAND_UNDO
    );
    command_binding!(
        false,
        Redo,
        Some("Editor"),
        crate::features::id::COMMAND_REDO
    );
    command_binding!(
        true,
        Redo,
        Some("Editor"),
        crate::features::id::COMMAND_REDO
    );
    command_binding!(
        false,
        Find,
        Some("Editor"),
        crate::features::id::COMMAND_FIND
    );
    command_binding!(
        true,
        Find,
        Some("Editor"),
        crate::features::id::COMMAND_FIND
    );
    command_binding!(
        true,
        FindWorkspace,
        Some("!Terminal"),
        crate::features::id::COMMAND_FIND_WORKSPACE
    );
    command_binding!(
        false,
        FindNext,
        Some("Editor"),
        crate::features::id::COMMAND_FIND_NEXT
    );
    command_binding!(
        true,
        FindNext,
        Some("Editor"),
        crate::features::id::COMMAND_FIND_NEXT
    );
    command_binding!(
        true,
        Complete,
        Some("Editor"),
        crate::features::id::COMMAND_COMPLETE
    );
    command_binding!(
        true,
        GoToDefinition,
        Some("Editor"),
        crate::features::id::COMMAND_GO_TO_DEFINITION
    );
    command_binding!(
        false,
        ShowCommands,
        None,
        crate::features::id::COMMAND_SHOW_COMMANDS
    );
    command_binding!(
        true,
        ShowCommands,
        Some("!Terminal"),
        crate::features::id::COMMAND_SHOW_COMMANDS
    );
    let quick_search_start = bindings.len();
    bindings.push(KeyBinding::new(
        "backspace",
        SearchBackspace,
        Some("QuickSearch"),
    ));
    bindings.push(KeyBinding::new("delete", SearchDelete, Some("QuickSearch")));
    bindings.push(KeyBinding::new("left", SearchLeft, Some("QuickSearch")));
    bindings.push(KeyBinding::new("right", SearchRight, Some("QuickSearch")));
    bindings.push(KeyBinding::new(
        "cmd-a",
        SearchSelectAll,
        Some("QuickSearch"),
    ));
    bindings.push(KeyBinding::new(
        "ctrl-a",
        SearchSelectAll,
        Some("QuickSearch"),
    ));
    bindings.push(KeyBinding::new("home", SearchHome, Some("QuickSearch")));
    bindings.push(KeyBinding::new("end", SearchEnd, Some("QuickSearch")));
    bindings.push(KeyBinding::new("up", SearchPrevious, Some("QuickSearch")));
    bindings.push(KeyBinding::new("down", SearchNext, Some("QuickSearch")));
    bindings.push(KeyBinding::new("enter", SearchConfirm, Some("QuickSearch")));
    bindings.push(KeyBinding::new(
        "escape",
        SearchDismiss,
        Some("QuickSearch"),
    ));
    let quick_search_end = bindings.len();
    bindings.push(KeyBinding::new("escape", Dismiss, Some("!Terminal")));
    command_binding!(
        true,
        TogglePreview,
        Some("!Terminal"),
        crate::features::id::COMMAND_TOGGLE_PREVIEW
    );
    command_binding!(
        true,
        ToggleBottomZone,
        Some("!Terminal"),
        crate::features::id::COMMAND_TOGGLE_BOTTOM_ZONE
    );
    command_binding!(
        true,
        ToggleLeftZone,
        Some("!Terminal"),
        crate::features::id::COMMAND_TOGGLE_LEFT_ZONE
    );
    command_binding!(
        true,
        ToggleRightZone,
        Some("!Terminal"),
        crate::features::id::COMMAND_TOGGLE_RIGHT_ZONE
    );
    command_binding!(
        true,
        ToggleAssistant,
        Some("!Terminal"),
        crate::features::id::COMMAND_TOGGLE_ASSISTANT
    );
    command_binding!(false, Quit, None, crate::features::id::COMMAND_QUIT);
    command_binding!(
        true,
        Quit,
        Some("!Terminal"),
        crate::features::id::COMMAND_QUIT
    );
    for &id in EXTRA_COMMANDS {
        for binding in registry.bindings_for(&id.into()) {
            if binding.keystroke.trim().is_empty()
                || binding
                    .keystroke
                    .split_whitespace()
                    .any(|key| gpui::Keystroke::parse(key).is_err())
            {
                return Err(format!("無効なショートカット: {}", binding.keystroke));
            }
            let target = if registry.binding_layer(&id.into()) == 1 {
                &mut presets
            } else {
                &mut overrides
            };
            target.push(KeyBinding::new(
                &binding.keystroke,
                InvokeCommand(id.into()),
                Some("!Terminal"),
            ));
        }
    }
    // 同じキーへの設定が内部入力を奪わないよう、衝突した入力だけ最後に置く。
    let mut protected = Vec::new();
    for index in (quick_search_start..quick_search_end)
        .rev()
        .chain((0..editor_input_end).rev())
    {
        if presets
            .iter()
            .chain(&overrides)
            .any(|binding| binding.keystrokes() == bindings[index].keystrokes())
        {
            protected.push(bindings.remove(index));
        }
    }
    bindings.extend(presets);
    bindings.extend(overrides);
    bindings.extend(protected.into_iter().rev());
    Ok(bindings)
}

#[cfg(test)]
mod keymap_tests {
    use super::*;
    use crate::keymap::{KeymapEntry, KeymapOverride, KeymapSettings, Shortcut};

    #[gpui::test]
    fn multi_binding_preset_and_user_replace_and_unbind(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let shortcut = |key: &str| Shortcut {
                keystroke: key.into(),
                label: String::new(),
            };
            let key = |s| gpui::Keystroke::parse(s).unwrap();
            let has = |cx: &App, s| {
                cx.all_bindings_for_input(&[key(s)])
                    .iter()
                    .any(|binding| binding.action().partial_eq(&Save))
            };
            bind_keys(cx);
            assert!(has(cx, "ctrl-s") && has(cx, "cmd-s"));
            let preset = crate::keymap::KeymapDefinition::new("example.multiple", []).with_entry(
                crate::features::id::COMMAND_SAVE_DOCUMENT,
                KeymapEntry::Bindings(vec![shortcut("ctrl-alt-s")]),
            );
            crate::keymap::register_preset(cx, preset).unwrap();
            let mut settings = KeymapSettings {
                preset: Some("example.multiple".into()),
                overrides: vec![],
            };
            apply_keymap_settings(cx, &settings).unwrap();
            assert!(has(cx, "ctrl-alt-s"));
            assert!(!has(cx, "ctrl-s") && !has(cx, "cmd-s"));
            settings.overrides.push(KeymapOverride {
                command: crate::features::id::COMMAND_SAVE_DOCUMENT.into(),
                entry: KeymapEntry::Bindings(vec![
                    shortcut("ctrl-shift-s"),
                    shortcut("cmd-shift-s"),
                ]),
            });
            apply_keymap_settings(cx, &settings).unwrap();
            assert!(!has(cx, "ctrl-alt-s"));
            assert!(has(cx, "ctrl-shift-s") && has(cx, "cmd-shift-s"));
            {
                let map = cx.key_bindings();
                let map = map.borrow();
                for stroke in ["ctrl-shift-s", "cmd-shift-s"] {
                    let (matches, _) = map.bindings_for_input(
                        &[key(stroke)],
                        &[gpui::KeyContext::parse("Shell").unwrap()],
                    );
                    assert!(
                        matches
                            .iter()
                            .any(|binding| binding.action().partial_eq(&Save))
                    );
                    let (matches, _) = map.bindings_for_input(
                        &[key(stroke)],
                        &[gpui::KeyContext::parse("Terminal").unwrap()],
                    );
                    assert!(
                        !matches
                            .iter()
                            .any(|binding| binding.action().partial_eq(&Save))
                    );
                }
            }
            assert_eq!(cx.all_bindings_for_input(&[key("backspace")]).len(), 2);
            settings.overrides[0].entry = KeymapEntry::Unbind;
            apply_keymap_settings(cx, &settings).unwrap();
            for stroke in [
                "ctrl-s",
                "cmd-s",
                "ctrl-alt-s",
                "ctrl-shift-s",
                "cmd-shift-s",
            ] {
                assert!(!has(cx, stroke));
            }
            crate::keymap::unregister_preset(cx, &"example.multiple".into()).unwrap();
            let preset = crate::keymap::KeymapDefinition::new("example.unbind", []).with_entry(
                crate::features::id::COMMAND_SAVE_DOCUMENT,
                KeymapEntry::Unbind,
            );
            crate::keymap::register_preset(cx, preset).unwrap();
            apply_keymap_settings(
                cx,
                &KeymapSettings {
                    preset: Some("example.unbind".into()),
                    overrides: vec![],
                },
            )
            .unwrap();
            assert!(!has(cx, "ctrl-s") && !has(cx, "cmd-s"));
            assert!(
                crate::keymap::KeymapRegistry::active(cx)
                    .bindings_for(&crate::features::id::COMMAND_SAVE_DOCUMENT.into())
                    .is_empty()
            );
        });
    }

    #[gpui::test]
    fn rebind_replaces_command_and_preserves_internal_bindings(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            bind_keys(cx);
            assert_eq!(cx.key_bindings().borrow().bindings().len(), 57);
            let key = |s| gpui::Keystroke::parse(s).unwrap();
            assert_eq!(cx.all_bindings_for_input(&[key("ctrl-s")]).len(), 1);
            let before = cx.all_bindings_for_input(&[key("backspace")]).len();
            assert_eq!(before, 2);
            apply_keymap_settings(
                cx,
                &KeymapSettings {
                    preset: None,
                    overrides: vec![KeymapOverride {
                        command: crate::features::id::COMMAND_SAVE_DOCUMENT.into(),
                        entry: KeymapEntry::Bindings(vec![Shortcut {
                            keystroke: "ctrl-alt-s".into(),
                            label: "Ctrl Alt S".into(),
                        }]),
                    }],
                },
            )
            .unwrap();
            assert!(cx.all_bindings_for_input(&[key("ctrl-s")]).is_empty());
            assert_eq!(cx.all_bindings_for_input(&[key("ctrl-alt-s")]).len(), 1);
            assert_eq!(cx.all_bindings_for_input(&[key("backspace")]).len(), before);
            assert_eq!(
                crate::keymap::KeymapRegistry::active(cx)
                    .shortcut_label(&crate::features::id::COMMAND_SAVE_DOCUMENT.into()),
                "Ctrl Alt S"
            );
            apply_keymap_settings(
                cx,
                &KeymapSettings {
                    preset: Some("missing.preset".into()),
                    overrides: vec![KeymapOverride {
                        command: crate::features::id::COMMAND_SAVE_DOCUMENT.into(),
                        entry: KeymapEntry::Unbind,
                    }],
                },
            )
            .unwrap();
            assert!(cx.all_bindings_for_input(&[key("ctrl-alt-s")]).is_empty());
            assert!(cx.all_bindings_for_input(&[key("ctrl-s")]).is_empty());
            assert!(
                crate::keymap::KeymapRegistry::active(cx)
                    .shortcut_label(&crate::features::id::COMMAND_SAVE_DOCUMENT.into())
                    .is_empty()
            );
        });
    }

    #[gpui::test]
    fn rebind_refuses_foreign_registration(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            bind_keys(cx);
            cx.bind_keys([KeyBinding::new("ctrl-alt-x", Quit, None)]);
            assert!(apply_keymap_settings(cx, &KeymapSettings::default()).is_err());
            assert_eq!(
                cx.all_bindings_for_input(&[gpui::Keystroke::parse("ctrl-alt-x").unwrap()])
                    .len(),
                1
            );
        });
    }

    #[gpui::test]
    fn contributed_preset_and_override_rebind_then_fall_back(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            bind_keys(cx);
            crate::keymap::register_preset(
                cx,
                crate::keymap::KeymapDefinition::new(
                    "example.preset",
                    [crate::keymap::KeyBindingDefinition::new(
                        crate::features::id::COMMAND_SAVE_DOCUMENT,
                        "ctrl-p",
                        "Ctrl P",
                    )],
                ),
            )
            .unwrap();
            apply_keymap_settings(
                cx,
                &KeymapSettings {
                    preset: Some("example.preset".into()),
                    overrides: vec![],
                },
            )
            .unwrap();
            let key = |s| gpui::Keystroke::parse(s).unwrap();
            assert_eq!(cx.all_bindings_for_input(&[key("ctrl-p")]).len(), 1);
            apply_keymap_settings(
                cx,
                &KeymapSettings {
                    preset: Some("example.preset".into()),
                    overrides: vec![KeymapOverride {
                        command: crate::features::id::COMMAND_SAVE_DOCUMENT.into(),
                        entry: KeymapEntry::Bindings(vec![Shortcut {
                            keystroke: "ctrl-w".into(),
                            label: "Ctrl W".into(),
                        }]),
                    }],
                },
            )
            .unwrap();
            assert!(cx.all_bindings_for_input(&[key("ctrl-p")]).is_empty());
            assert_eq!(cx.all_bindings_for_input(&[key("ctrl-w")]).len(), 1);
            crate::keymap::unregister_preset(cx, &"example.preset".into()).unwrap();
            assert_eq!(cx.all_bindings_for_input(&[key("ctrl-w")]).len(), 1);
            assert!(cx.all_bindings_for_input(&[key("ctrl-s")]).is_empty());
        });
    }

    #[test]
    fn contexts_and_override_precedence_use_gpui_resolution() {
        let mut registry = crate::keymap::KeymapRegistry::bundled();
        registry.apply_settings(&KeymapSettings {
            preset: None,
            overrides: vec![KeymapOverride {
                command: crate::features::id::COMMAND_SAVE_DOCUMENT.into(),
                entry: KeymapEntry::Bindings(vec![Shortcut {
                    keystroke: "ctrl-n".into(),
                    label: "Ctrl N".into(),
                }]),
            }],
        });
        let keymap = gpui::Keymap::new(resolved_bindings(&registry).unwrap());
        let context = |s| gpui::KeyContext::parse(s).unwrap();
        let key = |s| gpui::Keystroke::parse(s).unwrap();
        let (bindings, _) = keymap.bindings_for_input(&[key("ctrl-n")], &[context("Shell")]);
        assert!(bindings[0].action().partial_eq(&Save));
        let (bindings, _) = keymap.bindings_for_input(&[key("ctrl-n")], &[context("Terminal")]);
        assert!(bindings.is_empty());
        let (bindings, _) = keymap.bindings_for_input(
            &[key("backspace")],
            &[context("Editor"), context("QuickSearch")],
        );
        assert!(bindings[0].action().partial_eq(&SearchBackspace));
        let (bindings, _) = keymap.bindings_for_input(&[key("backspace")], &[context("Editor")]);
        assert!(bindings[0].action().partial_eq(&Backspace));
    }

    #[test]
    fn quick_search_input_wins_over_a_command_override() {
        let mut registry = crate::keymap::KeymapRegistry::bundled();
        registry.apply_settings(&KeymapSettings {
            preset: None,
            overrides: vec![KeymapOverride {
                command: crate::features::id::COMMAND_SAVE_DOCUMENT.into(),
                entry: KeymapEntry::Bindings(vec![Shortcut {
                    keystroke: "ctrl-a".into(),
                    label: "Ctrl A".into(),
                }]),
            }],
        });
        let keymap = gpui::Keymap::new(resolved_bindings(&registry).unwrap());
        let (bindings, _) = keymap.bindings_for_input(
            &[gpui::Keystroke::parse("ctrl-a").unwrap()],
            &[
                gpui::KeyContext::parse("Shell").unwrap(),
                gpui::KeyContext::parse("QuickSearch").unwrap(),
            ],
        );
        assert!(bindings[0].action().partial_eq(&SearchSelectAll));
    }

    #[test]
    fn additional_builtin_bindings_are_not_limited_by_adapter_slots() {
        let mut registry = crate::keymap::KeymapRegistry::bundled();
        let defaults = registry.definitions.get_mut(&registry.fallback).unwrap();
        for command in [
            crate::features::id::COMMAND_SAVE_DOCUMENT,
            crate::features::id::COMMAND_TOGGLE_PREVIEW,
        ] {
            let Some(KeymapEntry::Bindings(bindings)) = defaults.bindings.get_mut(&command.into())
            else {
                panic!("missing default")
            };
            bindings.push(Shortcut {
                keystroke: "ctrl-alt-9".into(),
                label: String::new(),
            });
        }
        let bindings = resolved_bindings(&registry).unwrap();
        let matching = gpui::Keymap::new(bindings)
            .all_bindings_for_input(&[gpui::Keystroke::parse("ctrl-alt-9").unwrap()]);
        assert!(
            matching
                .iter()
                .any(|binding| binding.action().partial_eq(&Save))
        );
        assert!(
            matching
                .iter()
                .any(|binding| binding.action().partial_eq(&TogglePreview))
        );
    }
    #[test]
    fn default_bindings_match_previous_gpui_bindings() {
        let expected = [
            KeyBinding::new("backspace", Backspace, Some("Editor")),
            KeyBinding::new("delete", Delete, Some("Editor")),
            KeyBinding::new("left", Left, Some("Editor")),
            KeyBinding::new("right", Right, Some("Editor")),
            KeyBinding::new("shift-left", SelectLeft, Some("Editor")),
            KeyBinding::new("shift-right", SelectRight, Some("Editor")),
            KeyBinding::new("cmd-a", SelectAll, Some("Editor")),
            KeyBinding::new("ctrl-a", SelectAll, Some("Editor")),
            KeyBinding::new("cmd-v", Paste, Some("Editor")),
            KeyBinding::new("ctrl-v", Paste, Some("Editor")),
            KeyBinding::new("cmd-c", Copy, Some("Editor")),
            KeyBinding::new("ctrl-c", Copy, Some("Editor")),
            KeyBinding::new("cmd-x", Cut, Some("Editor")),
            KeyBinding::new("ctrl-x", Cut, Some("Editor")),
            KeyBinding::new("home", Home, Some("Editor")),
            KeyBinding::new("end", End, Some("Editor")),
            KeyBinding::new("enter", Enter, Some("Editor")),
            KeyBinding::new("cmd-o", Open, None),
            KeyBinding::new("ctrl-o", Open, Some("!Terminal")),
            KeyBinding::new("cmd-s", Save, None),
            KeyBinding::new("ctrl-s", Save, Some("!Terminal")),
            KeyBinding::new("ctrl-shift-s", SaveAs, Some("!Terminal")),
            KeyBinding::new("cmd-n", New, None),
            KeyBinding::new("ctrl-n", New, Some("!Terminal")),
            KeyBinding::new("cmd-z", Undo, Some("Editor")),
            KeyBinding::new("ctrl-z", Undo, Some("Editor")),
            KeyBinding::new("cmd-shift-z", Redo, Some("Editor")),
            KeyBinding::new("ctrl-y", Redo, Some("Editor")),
            KeyBinding::new("cmd-f", Find, Some("Editor")),
            KeyBinding::new("ctrl-f", Find, Some("Editor")),
            KeyBinding::new("ctrl-shift-f", FindWorkspace, Some("!Terminal")),
            KeyBinding::new("cmd-g", FindNext, Some("Editor")),
            KeyBinding::new("f3", FindNext, Some("Editor")),
            KeyBinding::new("ctrl-space", Complete, Some("Editor")),
            KeyBinding::new("f12", GoToDefinition, Some("Editor")),
            KeyBinding::new("cmd-shift-k", ShowCommands, None),
            KeyBinding::new("ctrl-shift-k", ShowCommands, Some("!Terminal")),
            KeyBinding::new("backspace", SearchBackspace, Some("QuickSearch")),
            KeyBinding::new("delete", SearchDelete, Some("QuickSearch")),
            KeyBinding::new("left", SearchLeft, Some("QuickSearch")),
            KeyBinding::new("right", SearchRight, Some("QuickSearch")),
            KeyBinding::new("cmd-a", SearchSelectAll, Some("QuickSearch")),
            KeyBinding::new("ctrl-a", SearchSelectAll, Some("QuickSearch")),
            KeyBinding::new("home", SearchHome, Some("QuickSearch")),
            KeyBinding::new("end", SearchEnd, Some("QuickSearch")),
            KeyBinding::new("up", SearchPrevious, Some("QuickSearch")),
            KeyBinding::new("down", SearchNext, Some("QuickSearch")),
            KeyBinding::new("enter", SearchConfirm, Some("QuickSearch")),
            KeyBinding::new("escape", SearchDismiss, Some("QuickSearch")),
            KeyBinding::new("escape", Dismiss, Some("!Terminal")),
            KeyBinding::new("ctrl-alt-p", TogglePreview, Some("!Terminal")),
            KeyBinding::new("ctrl-j", ToggleBottomZone, Some("!Terminal")),
            KeyBinding::new("ctrl-alt-l", ToggleLeftZone, Some("!Terminal")),
            KeyBinding::new("ctrl-alt-r", ToggleRightZone, Some("!Terminal")),
            KeyBinding::new("ctrl-shift-a", ToggleAssistant, Some("!Terminal")),
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("ctrl-q", Quit, Some("!Terminal")),
        ];
        let actual = resolved_bindings(&crate::keymap::KeymapRegistry::bundled()).unwrap();
        assert_eq!(actual.len(), expected.len());
        for (actual, expected) in actual.iter().zip(expected.iter()) {
            assert_eq!(actual.keystrokes(), expected.keystrokes());
            assert!(actual.action().partial_eq(expected.action()));
            assert_eq!(
                actual.predicate().map(|v| v.to_string()),
                expected.predicate().map(|v| v.to_string())
            );
        }
    }
}
