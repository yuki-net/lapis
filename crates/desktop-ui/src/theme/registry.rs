use std::{cell::RefCell, collections::BTreeMap};

use gpui::{rgb, Rgba};

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
        let fallback = ThemeDefinition::new(
            "lapis.fallback-dark",
            ThemeColors {
                canvas: rgb(0x0f1012),
                title_bar: rgb(0x0d0e10),
                island: rgb(0x18191d),
                surface: rgb(0x202127),
                surface_hover: rgb(0x272931),
                surface_active: rgb(0x2d3039),
                border: rgb(0x25262b),
                text: rgb(0xe6e7eb),
                muted: rgb(0x989ba5),
                subtle: rgb(0x676b75),
                accent: rgb(0x7a7df5),
                accent_soft: rgb(0x2a2b43),
                orange: rgb(0xe4a86c),
                close_hover: rgb(0xc42b1c),
            },
        );
        let id = fallback.id.clone();
        Self {
            fallback: id.clone(),
            active: id.clone(),
            definitions: [(id, fallback)].into_iter().collect(),
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
pub fn active_id() -> ThemeId {
    THEMES.with(|registry| registry.borrow().active.clone())
}

/// アクティブテーマの特定カラーを返す内部ヘルパー。
pub(super) fn color(select: impl FnOnce(&ThemeColors) -> Rgba) -> Rgba {
    THEMES.with(|registry| select(&registry.borrow().active_definition().colors))
}
