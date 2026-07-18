use std::{cell::RefCell, collections::BTreeMap};

use gpui::{Rgba, rgb};

use crate::extension_ui::ThemeId;

pub const TITLE_BAR_HEIGHT: f32 = 40.0;
pub const WINDOW_CONTROL_WIDTH: f32 = 46.0;
pub const WINDOW_RESIZE_BORDER_HEIGHT: f32 = 4.0;
pub const TOOL_ISLAND_WIDTH: f32 = 260.0;
pub const SIDE_PANEL_WIDTH: f32 = 310.0;
pub const BOTTOM_PANEL_HEIGHT: f32 = 196.0;
pub const ISLAND_RADIUS: f32 = 8.0;
pub const CANVAS_GAP: f32 = 6.0;

#[derive(Clone, Debug)]
pub struct ThemeColors {
    pub canvas: Rgba,
    pub title_bar: Rgba,
    pub island: Rgba,
    pub surface: Rgba,
    pub surface_hover: Rgba,
    pub surface_active: Rgba,
    pub border: Rgba,
    pub text: Rgba,
    pub muted: Rgba,
    pub subtle: Rgba,
    pub accent: Rgba,
    pub accent_soft: Rgba,
    pub orange: Rgba,
    pub close_hover: Rgba,
}

#[derive(Clone, Debug)]
pub struct ThemeDefinition {
    pub id: ThemeId,
    pub colors: ThemeColors,
}

impl ThemeDefinition {
    pub fn new(id: impl Into<ThemeId>, colors: ThemeColors) -> Self {
        Self {
            id: id.into(),
            colors,
        }
    }
}

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

pub fn active_id() -> ThemeId {
    THEMES.with(|registry| registry.borrow().active.clone())
}

fn color(select: impl FnOnce(&ThemeColors) -> Rgba) -> Rgba {
    THEMES.with(|registry| select(&registry.borrow().active_definition().colors))
}

pub fn canvas() -> Rgba {
    color(|colors| colors.canvas)
}

pub fn title_bar() -> Rgba {
    color(|colors| colors.title_bar)
}

pub fn island() -> Rgba {
    color(|colors| colors.island)
}

pub fn surface() -> Rgba {
    color(|colors| colors.surface)
}

pub fn surface_hover() -> Rgba {
    color(|colors| colors.surface_hover)
}

pub fn surface_active() -> Rgba {
    color(|colors| colors.surface_active)
}

pub fn border() -> Rgba {
    color(|colors| colors.border)
}

pub fn text() -> Rgba {
    color(|colors| colors.text)
}

pub fn muted() -> Rgba {
    color(|colors| colors.muted)
}

pub fn subtle() -> Rgba {
    color(|colors| colors.subtle)
}

pub fn accent() -> Rgba {
    color(|colors| colors.accent)
}

pub fn accent_soft() -> Rgba {
    color(|colors| colors.accent_soft)
}

pub fn orange() -> Rgba {
    color(|colors| colors.orange)
}

pub fn close_hover() -> Rgba {
    color(|colors| colors.close_hover)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registered_theme_can_be_activated_without_changing_token_callers() {
        let original = active_id();
        let mut colors =
            THEMES.with(|registry| registry.borrow().active_definition().colors.clone());
        colors.accent = rgb(0x123456);
        let alternate = ThemeId::new("test.alternate");
        register(ThemeDefinition::new(alternate.clone(), colors)).unwrap();
        assert!(set_active(&alternate));
        assert_eq!(accent(), rgb(0x123456));
        assert!(set_active(&original));
    }
}
