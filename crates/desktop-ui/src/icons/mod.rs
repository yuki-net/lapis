use std::collections::BTreeMap;

use crate::extension_ui::{IconId, IconThemeId};

pub mod id {
    pub const MENU: &str = "ui.menu";
    pub const TOOL_DOCK: &str = "ui.tool-dock";
    pub const SIDE_DOCK: &str = "ui.side-dock";
    pub const BOTTOM_DOCK: &str = "ui.bottom-dock";
    pub const ASSISTANT: &str = "assistant";
    pub const SEARCH: &str = "search";
    pub const RUN: &str = "ui.run";
    pub const MINIMIZE: &str = "window.minimize";
    pub const MAXIMIZE: &str = "window.maximize";
    pub const RESTORE: &str = "window.restore";
    pub const CLOSE: &str = "window.close";
}

#[derive(Clone, Debug)]
pub struct IconThemeDefinition {
    pub id: IconThemeId,
    icons: BTreeMap<IconId, String>,
}

impl IconThemeDefinition {
    pub fn new(
        id: impl Into<IconThemeId>,
        icons: impl IntoIterator<Item = (impl Into<IconId>, impl Into<String>)>,
    ) -> Self {
        Self {
            id: id.into(),
            icons: icons
                .into_iter()
                .map(|(id, glyph)| (id.into(), glyph.into()))
                .collect(),
        }
    }

    fn resolve(&self, icon: &IconId) -> Option<&str> {
        self.icons.get(icon).map(String::as_str)
    }
}

pub struct IconThemeRegistry {
    fallback: IconThemeId,
    active: IconThemeId,
    definitions: BTreeMap<IconThemeId, IconThemeDefinition>,
}

impl IconThemeRegistry {
    pub fn bundled() -> Self {
        let fallback = IconThemeDefinition::new(
            "lapis.fallback-icons",
            [
                (id::MENU, "☰"),
                (id::TOOL_DOCK, "▤"),
                (id::SIDE_DOCK, "▥"),
                (id::BOTTOM_DOCK, "▱"),
                (id::RUN, "▷"),
                (id::MINIMIZE, "—"),
                (id::MAXIMIZE, "□"),
                (id::RESTORE, "❐"),
                (id::CLOSE, "×"),
                ("files", "F"),
                ("search", "S"),
                ("git", "G"),
                ("history", "H"),
                ("preview", "P"),
                ("assistant", "A"),
                ("terminal", ">_"),
                ("problems", "!"),
                ("output", "O"),
            ],
        );
        let id = fallback.id.clone();
        Self {
            fallback: id.clone(),
            active: id.clone(),
            definitions: [(id, fallback)].into_iter().collect(),
        }
    }

    pub fn register(&mut self, definition: IconThemeDefinition) -> Result<(), IconThemeId> {
        if self.definitions.contains_key(&definition.id) {
            return Err(definition.id);
        }
        self.definitions.insert(definition.id.clone(), definition);
        Ok(())
    }

    pub fn set_active(&mut self, theme: &IconThemeId) -> bool {
        if !self.definitions.contains_key(theme) {
            return false;
        }
        self.active = theme.clone();
        true
    }

    pub fn resolve(&self, icon: &IconId) -> String {
        self.definitions
            .get(&self.active)
            .and_then(|theme| theme.resolve(icon))
            .or_else(|| {
                self.definitions
                    .get(&self.fallback)
                    .and_then(|theme| theme.resolve(icon))
            })
            .unwrap_or("·")
            .to_owned()
    }

    pub fn resolve_name(&self, icon: &str) -> String {
        self.resolve(&IconId::new(icon))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_theme_falls_back_per_icon() {
        let mut registry = IconThemeRegistry::bundled();
        registry
            .register(IconThemeDefinition::new("minimal", [(id::MENU, "M")]))
            .unwrap();
        assert!(registry.set_active(&IconThemeId::new("minimal")));
        assert_eq!(registry.resolve_name(id::MENU), "M");
        assert_eq!(registry.resolve_name(id::CLOSE), "×");
        assert_eq!(registry.resolve_name("unknown"), "·");
    }
}
