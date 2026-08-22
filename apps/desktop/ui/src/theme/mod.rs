mod definition;
mod registry;

// テーマ定義とレジストリ
pub use definition::{ThemeColors, ThemeDefinition};
pub use registry::{active_colors, active_id, available, name, register, set_active};

/// アクティブテーマのカラートークンを取得する。
pub fn colors() -> ThemeColors {
    active_colors()
}

#[cfg(test)]
mod tests {
    use gpui::rgb;

    use super::*;
    use crate::extension_ui::ThemeId;

    #[test]
    fn registered_theme_can_be_activated_without_changing_token_callers() {
        let original_id = active_id();
        let original_colors = colors();

        let alternate_id = ThemeId::new("test.tokens.alternate");
        let mut alternate_colors = original_colors.clone();
        alternate_colors.accent = rgb(0x123456);

        let _ = register(ThemeDefinition::new(alternate_id.clone(), alternate_colors));
        assert!(set_active(&alternate_id));
        assert_eq!(colors().accent, rgb(0x123456));
        assert!(set_active(&original_id));
        assert_eq!(colors().accent, original_colors.accent);
    }
}
