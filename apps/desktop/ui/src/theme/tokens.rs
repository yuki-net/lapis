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

pub fn command_palette_border() -> Rgba {
    color(|colors| colors.command_palette_border)
}

pub fn focus_border() -> Rgba {
    color(|colors| colors.focus_border)
}

pub fn brand_text() -> Rgba {
    color(|colors| colors.brand_text)
}

pub fn on_accent_text() -> Rgba {
    color(|colors| colors.on_accent_text)
}

pub fn assistant_accent() -> Rgba {
    color(|colors| colors.assistant_accent)
}

pub fn editor_selection() -> Rgba {
    color(|colors| colors.editor_selection)
}

pub fn editor_search_match() -> Rgba {
    color(|colors| colors.editor_search_match)
}

pub fn editor_cursor() -> Rgba {
    color(|colors| colors.editor_cursor)
}

pub fn search_selection() -> Rgba {
    color(|colors| colors.search_selection)
}

pub fn command_input_border() -> Rgba {
    color(|colors| colors.command_input_border)
}

pub fn task_primary_border() -> Rgba {
    color(|colors| colors.task_primary_border)
}

pub fn task_primary_text() -> Rgba {
    color(|colors| colors.task_primary_text)
}

pub fn status_success() -> Rgba {
    color(|colors| colors.status_success)
}

pub fn status_error() -> Rgba {
    color(|colors| colors.status_error)
}

pub fn status_warning() -> Rgba {
    color(|colors| colors.status_warning)
}

pub fn status_info() -> Rgba {
    color(|colors| colors.status_info)
}

pub fn diff_added() -> Rgba {
    color(|colors| colors.diff_added)
}

pub fn diff_removed() -> Rgba {
    color(|colors| colors.diff_removed)
}

pub fn diff_changed() -> Rgba {
    color(|colors| colors.diff_changed)
}

pub fn problem_error() -> Rgba {
    color(|colors| colors.problem_error)
}

pub fn note() -> Rgba {
    color(|colors| colors.note)
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
                command_palette_border: command_palette_border(),
                focus_border: focus_border(),
                brand_text: brand_text(),
                on_accent_text: on_accent_text(),
                assistant_accent: assistant_accent(),
                editor_selection: editor_selection(),
                editor_search_match: editor_search_match(),
                editor_cursor: editor_cursor(),
                search_selection: search_selection(),
                command_input_border: command_input_border(),
                task_primary_border: task_primary_border(),
                task_primary_text: task_primary_text(),
                status_success: status_success(),
                status_error: status_error(),
                status_warning: status_warning(),
                status_info: status_info(),
                diff_added: diff_added(),
                diff_removed: diff_removed(),
                diff_changed: diff_changed(),
                problem_error: problem_error(),
                note: note(),
            },
        ));
        assert!(set_active(&alternate_id));
        assert_eq!(accent(), rgb(0x123456));
        // 元に戻す
        assert!(set_active(&original_id));
        assert_eq!(accent(), original_accent);
    }
}
