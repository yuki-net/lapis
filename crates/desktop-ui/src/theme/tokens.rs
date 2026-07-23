use gpui::Rgba;

use super::registry::color;

/// カラートークン関数群。アクティブテーマの色を返す。

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
    use gpui::rgb;

    use super::super::registry::{active_id, register, set_active};
    use super::*;

    #[test]
    fn registered_theme_can_be_activated_without_changing_token_callers() {
        use super::super::definition::{ThemeColors, ThemeDefinition};
        use crate::extension_ui::ThemeId;

        let original_id = active_id();
        let original_accent = accent();

        // accent だけ変えた別テーマを登録して切り替え、トークン関数が追従することを確認する
        let alternate_id = ThemeId::new("test.tokens.alternate");
        let _ = register(ThemeDefinition::new(
            alternate_id.clone(),
            ThemeColors {
                accent: rgb(0x123456),
                canvas: canvas(),
                title_bar: title_bar(),
                island: island(),
                surface: surface(),
                surface_hover: surface_hover(),
                surface_active: surface_active(),
                border: border(),
                text: text(),
                muted: muted(),
                subtle: subtle(),
                accent_soft: accent_soft(),
                orange: orange(),
                close_hover: close_hover(),
            },
        ));
        assert!(set_active(&alternate_id));
        assert_eq!(accent(), rgb(0x123456));
        // 元に戻す
        assert!(set_active(&original_id));
        assert_eq!(accent(), original_accent);
    }
}
