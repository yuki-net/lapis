use gpui::{ElementId, Stateful, prelude::*, px};

use super::{Icon, SurfaceVariant, surface};
use crate::{components::IconName, theme, tokens};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ButtonSize {
    Xs,
    Sm,
    Md,
}

pub(crate) fn button(
    id: impl Into<ElementId>,
    content: impl IntoElement,
    size: ButtonSize,
    cx: &gpui::App,
) -> Stateful<gpui::Div> {
    let height = match size {
        ButtonSize::Xs => tokens::size::BUTTON_XS,
        ButtonSize::Sm => tokens::size::BUTTON_SM,
        ButtonSize::Md => tokens::size::BUTTON_MD,
    };

    surface(SurfaceVariant::Control, cx)
        .id(id)
        .h(height)
        .px(tokens::spacing::XS)
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .occlude()
        .text_color(theme::colors(cx).text_secondary)
        .child(content)
}

pub(crate) fn header_button(
    id: impl Into<ElementId>,
    content: impl IntoElement,
    cx: &gpui::App,
) -> Stateful<gpui::Div> {
    button(id, content, ButtonSize::Md, cx)
}

pub(crate) fn icon_button(
    id: impl Into<ElementId>,
    icon: IconName,
    cx: &gpui::App,
) -> Stateful<gpui::Div> {
    header_button(id, Icon::new(icon), cx).w(tokens::size::HEADER_BUTTON)
}

pub(crate) fn action_button(
    label: impl Into<gpui::SharedString>,
    primary: bool,
    cx: &gpui::App,
) -> gpui::Stateful<gpui::Div> {
    let label = label.into();
    button(label.clone(), label, ButtonSize::Sm, cx)
        .px_2()
        .rounded(px(5.0))
        .border_1()
        .border_color(if primary {
            theme::colors(cx).button_border_focused
        } else {
            theme::colors(cx).border_default
        })
        .bg(if primary {
            theme::colors(cx).button_background_selected
        } else {
            theme::colors(cx).background_tertiary
        })
        .text_size(px(10.0))
        .text_color(if primary {
            theme::colors(cx).text_primary
        } else {
            theme::colors(cx).text_secondary
        })
        .flex()
        .items_center()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn action_buttons_do_not_install_hover_styles_twice(test_cx: &mut gpui::TestAppContext) {
        test_cx.update(|cx| {
            crate::theme::init(&crate::extension_ui::ThemeId::new("lapis.dark"), None, cx);
            let _ = action_button("primary", true, cx);
            let _ = action_button("secondary", false, cx);
        });
    }
}
