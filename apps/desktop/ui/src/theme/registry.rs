use std::{cell::RefCell, collections::BTreeMap};

use crate::extension_ui::ThemeId;

use super::definition::{ThemeColors, ThemeDefinition};

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

thread_local! {
    static THEMES: RefCell<ThemeRegistry> = RefCell::new(ThemeRegistry::bundled());
}

/// テーマを登録する。同一 ID が既存の場合は `Err(id)` を返す。
pub fn register(definition: ThemeDefinition) -> Result<(), ThemeId> {
    THEMES.with(|registry| {
        let mut registry = registry.borrow_mut();
        if registry.definitions.contains_key(&definition.id) {
            return Err(definition.id);
        }
        registry
            .definitions
            .insert(definition.id.clone(), definition);
        Ok(())
    })
}

/// アクティブテーマを切り替える。ID が未登録の場合は `false` を返す。
pub fn set_active(theme: &ThemeId) -> bool {
    THEMES.with(|registry| {
        let mut registry = registry.borrow_mut();
        if !registry.definitions.contains_key(theme) {
            return false;
        }
        registry.active = theme.clone();
        true
    })
}

/// アクティブテーマの ID を返す。
pub fn active_colors() -> ThemeColors {
    THEMES.with(|registry| registry.borrow().active_definition().colors.clone())
}

pub fn active_id() -> ThemeId {
    THEMES.with(|registry| registry.borrow().active.clone())
}

pub fn available() -> Vec<(ThemeId, String)> {
    THEMES.with(|registry| {
        registry
            .borrow()
            .definitions
            .values()
            .map(|definition| (definition.id.clone(), definition.name.clone()))
            .collect()
    })
}

pub fn name(theme: &ThemeId) -> Option<String> {
    THEMES.with(|registry| {
        registry
            .borrow()
            .definitions
            .get(theme)
            .map(|definition| definition.name.clone())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_themes_are_available() {
        let themes = available();
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
    }
}
