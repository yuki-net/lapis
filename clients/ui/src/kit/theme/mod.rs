mod definition;
mod registry;
pub(crate) mod syntax;

// テーマ定義とレジストリ
pub use definition::{ThemeAppearance, ThemeColors, ThemeDefinition};
pub use registry::{
    active_appearance, active_colors, active_id, available, name, register, set_active,
};

/// アクティブテーマのカラートークンを取得する。
pub fn colors(cx: &gpui::App) -> ThemeColors {
    active_colors(cx)
}

pub(crate) use registry::init;

#[cfg(test)]
mod tests {
    use gpui::rgb;

    use super::*;
    use crate::extension_ui::ThemeId;

    #[gpui::test]
    fn registered_theme_can_be_activated_without_changing_token_callers(
        test_cx: &mut gpui::TestAppContext,
    ) {
        test_cx.update(|cx| {
            crate::theme::init(&crate::extension_ui::ThemeId::new("lapis.dark"), None, cx);
            let original_id = active_id(cx);
            let original_colors = colors(cx);

            let alternate_id = ThemeId::new("test.tokens.alternate");
            let mut alternate_colors = original_colors.clone();
            alternate_colors.text_accent = rgb(0x123456);

            let _ = register(
                ThemeDefinition::new(alternate_id.clone(), alternate_colors),
                cx,
            );
            assert!(set_active(&alternate_id, cx));
            assert_eq!(colors(cx).text_accent, rgb(0x123456));
            assert!(set_active(&original_id, cx));
            assert_eq!(colors(cx).text_accent, original_colors.text_accent);
        });
    }
}
