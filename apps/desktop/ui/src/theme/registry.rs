use gpui::{App, Global};
use lapis_settings::SyntaxTheme;
use std::collections::BTreeMap;

use crate::extension_ui::ThemeId;

use super::definition::{ThemeAppearance, ThemeColors, ThemeDefinition};

/// テーマの登録・切り替えを管理するレジストリ。
struct ThemeRegistry {
    fallback: ThemeId,
    active: ThemeId,
    definitions: BTreeMap<ThemeId, ThemeDefinition>,
}

impl ThemeRegistry {
    fn bundled() -> Self {
        const BUILTIN: [&str; 2] = [
            include_str!("../../assets/themes/dark.json"),
            include_str!("../../assets/themes/white.json"),
        ];
        let mut definitions = BTreeMap::new();
        let mut fallback = None;
        for source in BUILTIN {
            match ThemeDefinition::from_json(source) {
                Ok(definition) => {
                    if definitions.contains_key(&definition.id) {
                        eprintln!("Duplicate bundled theme id");
                        continue;
                    }
                    if fallback.is_none() {
                        fallback = Some(definition.clone());
                    }
                    definitions.insert(definition.id.clone(), definition);
                }
                Err(error) => eprintln!("Bundled theme ignored: {error}"),
            }
        }
        let fallback = fallback.expect("at least one bundled theme must be valid");
        let id = fallback.id.clone();
        Self {
            fallback: id.clone(),
            active: id.clone(),
            definitions,
        }
    }

    fn active_definition(&self) -> &ThemeDefinition {
        self.definitions
            .get(&self.active)
            .or_else(|| self.definitions.get(&self.fallback))
            .expect("fallback theme is always registered")
    }
}

pub(super) struct ThemeState {
    registry: ThemeRegistry,
    pub(super) syntax_theme: Option<SyntaxTheme>,
}
impl Global for ThemeState {}

pub(crate) fn init(theme: &ThemeId, syntax: Option<SyntaxTheme>, cx: &mut App) {
    cx.set_global(ThemeState {
        registry: ThemeRegistry::bundled(),
        syntax_theme: syntax,
    });
    set_active(theme, cx);
}

/// テーマを登録する。同一 ID が既存の場合は `Err(id)` を返す。
pub fn register(definition: ThemeDefinition, cx: &mut App) -> Result<(), ThemeId> {
    let registry = &mut cx.global_mut::<ThemeState>().registry;
    if registry.definitions.contains_key(&definition.id) {
        return Err(definition.id);
    }
    registry
        .definitions
        .insert(definition.id.clone(), definition);
    Ok(())
}

/// アクティブテーマを切り替える。ID が未登録の場合は `false` を返す。
pub fn set_active(theme: &ThemeId, cx: &mut App) -> bool {
    let registry = &mut cx.global_mut::<ThemeState>().registry;
    if !registry.definitions.contains_key(theme) {
        return false;
    }
    registry.active = theme.clone();
    true
}

/// アクティブテーマの ID を返す。
pub fn active_colors(cx: &App) -> ThemeColors {
    cx.global::<ThemeState>()
        .registry
        .active_definition()
        .colors
        .clone()
}

pub fn active_id(cx: &App) -> ThemeId {
    cx.global::<ThemeState>().registry.active.clone()
}

/// アクティブテーマの明暗を返す。
pub fn active_appearance(cx: &App) -> ThemeAppearance {
    cx.global::<ThemeState>()
        .registry
        .active_definition()
        .appearance
}

pub fn available(cx: &App) -> Vec<(ThemeId, String)> {
    cx.global::<ThemeState>()
        .registry
        .definitions
        .values()
        .map(|definition| (definition.id.clone(), definition.name.clone()))
        .collect()
}

pub fn name(theme: &ThemeId, cx: &App) -> Option<String> {
    cx.global::<ThemeState>()
        .registry
        .definitions
        .get(theme)
        .map(|definition| definition.name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn fallback_and_duplicate_registration_keep_active_theme(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            init(&ThemeId::new("missing"), None, cx);
            assert_eq!(active_id(cx).as_str(), "lapis.dark");
            let id = ThemeId::new("test.custom");
            let definition = ThemeDefinition::new(id.clone(), active_colors(cx));
            assert!(register(definition.clone(), cx).is_ok());
            assert_eq!(register(definition, cx), Err(id.clone()));
            assert!(set_active(&id, cx));
            assert!(!set_active(&ThemeId::new("missing"), cx));
            assert_eq!(active_id(cx), id);
        });
    }

    #[gpui::test]
    fn bundled_themes_are_available(test_cx: &mut gpui::TestAppContext) {
        test_cx.update(|cx| {
            crate::theme::init(&crate::extension_ui::ThemeId::new("lapis.dark"), None, cx);
            let themes = available(cx);
            assert!(
                themes
                    .iter()
                    .any(|(id, name)| { id.as_str() == "lapis.dark" && name == "Dark" })
            );
            assert!(
                themes
                    .iter()
                    .any(|(id, name)| { id.as_str() == "lapis.white" && name == "White" })
            );
        });
    }
}
