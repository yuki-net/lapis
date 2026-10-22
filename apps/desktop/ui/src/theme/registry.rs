use std::collections::BTreeMap;

#[cfg(not(all(feature = "hot-reload", debug_assertions)))]
use std::cell::RefCell;

#[cfg(all(feature = "hot-reload", debug_assertions))]
use std::sync::{LazyLock, Mutex};

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

#[cfg(not(all(feature = "hot-reload", debug_assertions)))]
thread_local! {
    static THEMES: RefCell<ThemeRegistry> = RefCell::new(ThemeRegistry::bundled());
}

#[cfg(all(feature = "hot-reload", debug_assertions))]
static THEMES: LazyLock<Mutex<ThemeRegistry>> =
    LazyLock::new(|| Mutex::new(ThemeRegistry::bundled()));

#[cfg(not(all(feature = "hot-reload", debug_assertions)))]
fn with_themes<R>(f: impl FnOnce(&ThemeRegistry) -> R) -> R {
    THEMES.with(|registry| f(&registry.borrow()))
}

#[cfg(all(feature = "hot-reload", debug_assertions))]
fn with_themes<R>(f: impl FnOnce(&ThemeRegistry) -> R) -> R {
    f(&THEMES.lock().expect("theme registry lock poisoned"))
}

#[cfg(not(all(feature = "hot-reload", debug_assertions)))]
fn with_themes_mut<R>(f: impl FnOnce(&mut ThemeRegistry) -> R) -> R {
    THEMES.with(|registry| f(&mut registry.borrow_mut()))
}

#[cfg(all(feature = "hot-reload", debug_assertions))]
fn with_themes_mut<R>(f: impl FnOnce(&mut ThemeRegistry) -> R) -> R {
    f(&mut THEMES.lock().expect("theme registry lock poisoned"))
}

/// テーマを登録する。同一 ID が既存の場合は `Err(id)` を返す。
pub fn register(definition: ThemeDefinition) -> Result<(), ThemeId> {
    with_themes_mut(|registry| {
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
    with_themes_mut(|registry| {
        if !registry.definitions.contains_key(theme) {
            return false;
        }
        registry.active = theme.clone();
        true
    })
}

/// アクティブテーマの ID を返す。
pub fn active_colors() -> ThemeColors {
    with_themes(|registry| registry.active_definition().colors.clone())
}

pub fn active_id() -> ThemeId {
    with_themes(|registry| registry.active.clone())
}

/// アクティブテーマの明暗を返す。
pub fn active_appearance() -> ThemeAppearance {
    with_themes(|registry| registry.active_definition().appearance)
}

pub fn available() -> Vec<(ThemeId, String)> {
    with_themes(|registry| {
        registry
            .definitions
            .values()
            .map(|definition| (definition.id.clone(), definition.name.clone()))
            .collect()
    })
}

pub fn name(theme: &ThemeId) -> Option<String> {
    with_themes(|registry| {
        registry
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
