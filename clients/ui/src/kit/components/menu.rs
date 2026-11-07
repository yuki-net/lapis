use gpui::{
    AnyElement, App, ElementId, IntoElement, Pixels, RenderOnce, SharedString, Stateful, Window,
    div, prelude::*, px,
};

use super::{Icon, IconName, SurfaceVariant, floating_panel};
use crate::{theme, tokens};

const FONT_SIZE: Pixels = px(13.);
const ICON_SIZE: Pixels = px(16.);
const CHEVRON_SIZE: Pixels = px(14.);
const LABEL_GAP: Pixels = px(16.);

#[derive(Clone, Copy)]
pub(crate) enum TreeItem {
    Group { expanded: bool },
    Leaf,
}

#[derive(Clone)]
pub(crate) struct MenuItemSpec {
    pub(crate) id: SharedString,
    pub(crate) label: SharedString,
    pub(crate) shortcut: Option<SharedString>,
    pub(crate) icon: Option<IconName>,
    pub(crate) enabled: bool,
    pub(crate) selected: bool,
    pub(crate) indent: usize,
    pub(crate) tree: Option<TreeItem>,
    pub(crate) submenu: bool,
    pub(crate) separator_before: bool,
}

/// 項目データと、その行固有のクリック処理・サブメニューを保持する。
pub(crate) struct MenuEntry {
    spec: MenuItemSpec,
    element: AnyElement,
}

impl MenuEntry {
    pub(crate) fn new(
        spec: MenuItemSpec,
        cx: &App,
        decorate: impl FnOnce(Stateful<gpui::Div>) -> AnyElement,
    ) -> Self {
        let row = menu_item(spec.clone(), cx);
        Self {
            spec,
            element: decorate(row),
        }
    }
}

/// 配列内の最も幅を必要とする行に左右padding・borderを加え、Window幅で制限する。
#[derive(IntoElement)]
pub(crate) struct Menu {
    id: ElementId,
    entries: Vec<MenuEntry>,
    padding: Pixels,
    max_height: Option<Pixels>,
    debug_selector: String,
    min_width: Pixels,
    header: Option<AnyElement>,
}

impl Menu {
    pub(crate) fn new(id: impl Into<ElementId>, entries: Vec<MenuEntry>) -> Self {
        Self {
            id: id.into(),
            entries,
            padding: tokens::spacing::XS,
            max_height: None,
            debug_selector: String::new(),
            min_width: px(0.),
            header: None,
        }
    }

    pub(crate) fn padding(mut self, padding: Pixels) -> Self {
        self.padding = padding;
        self
    }

    pub(crate) fn min_width(mut self, width: Pixels) -> Self {
        self.min_width = width;
        self
    }

    pub(crate) fn header(mut self, header: impl IntoElement) -> Self {
        self.header = Some(header.into_any_element());
        self
    }

    pub(crate) fn max_height(mut self, height: Pixels) -> Self {
        self.max_height = Some(height);
        self
    }

    pub(crate) fn debug_selector(mut self, selector: impl Into<String>) -> Self {
        self.debug_selector = selector.into();
        self
    }
}

fn row_width(spec: &MenuItemSpec, measure: impl Fn(&SharedString) -> Pixels) -> Pixels {
    let trailing = spec
        .shortcut
        .as_ref()
        .map_or(px(0.), |text| LABEL_GAP + measure(text))
        + if spec.submenu {
            LABEL_GAP + CHEVRON_SIZE
        } else {
            px(0.)
        };
    tokens::spacing::SM * 2.
        + px(2.)
        + ICON_SIZE
        + tokens::spacing::SM
        + measure(&spec.label)
        + trailing
        + px(spec.indent as f32 * 16.)
        + if spec.tree.is_some() {
            ICON_SIZE
        } else {
            px(0.)
        }
}

impl RenderOnce for Menu {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let style = window.text_style();
        let measure = |text: &SharedString| {
            window
                .text_system()
                .shape_line(text.clone(), FONT_SIZE, &[style.to_run(text.len())], None)
                .width
        };
        let content_width = self
            .entries
            .iter()
            .map(|entry| row_width(&entry.spec, measure))
            .fold(px(0.), Pixels::max);
        let width = (content_width + self.padding * 2. + px(2.))
            .ceil()
            .max(self.min_width)
            .min((window.viewport_size().width - tokens::spacing::SM * 2.).max(px(0.)));
        floating_panel(self.id, SurfaceVariant::Menu, cx)
            .debug_selector(move || self.debug_selector)
            .flex()
            .flex_col()
            .w(width)
            .p(self.padding)
            .when_some(self.header, |menu, header| menu.child(header))
            .when_some(self.max_height, |menu, height| {
                menu.max_h(height).overflow_y_scroll()
            })
            .children(self.entries.into_iter().flat_map(|entry| {
                let separator = entry
                    .spec
                    .separator_before
                    .then(|| super::separator(cx).into_any_element());
                separator.into_iter().chain(std::iter::once(entry.element))
            }))
    }
}

fn menu_item(spec: MenuItemSpec, cx: &gpui::App) -> Stateful<gpui::Div> {
    let id = spec.id.clone();
    let enabled = spec.enabled;
    let selected = enabled && spec.selected;
    let colors = theme::colors(cx);
    let shortcut_and_submenu = spec.shortcut.is_some() && spec.submenu;
    div()
        .id(ElementId::Name(id))
        .h(px(30.0))
        .flex_shrink_0()
        .w_full()
        .px(tokens::spacing::SM)
        .pl(tokens::spacing::SM + px(spec.indent as f32 * 16.))
        .rounded(tokens::radius::MENU_ITEM)
        .border_1()
        .border_color(if selected {
            colors.button_border_focused
        } else {
            gpui::rgba(0x00000000)
        })
        .when(selected, |item| item.bg(colors.button_background_focused))
        .flex()
        .items_center()
        .text_size(FONT_SIZE)
        .whitespace_nowrap()
        .text_color(if enabled {
            colors.text_primary
        } else {
            colors.text_tertiary
        })
        .when(enabled && !selected, |item| {
            item.hover(|style| style.bg(colors.button_background_hover))
        })
        .when_some(spec.tree, |item, tree| {
            item.child(
                div()
                    .size(ICON_SIZE)
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when_some(
                        match tree {
                            TreeItem::Group { expanded: true } => Some(IconName::ChevronDown),
                            TreeItem::Group { expanded: false } => Some(IconName::ChevronRight),
                            TreeItem::Leaf => None,
                        },
                        |slot, icon| slot.child(Icon::new(icon).size(CHEVRON_SIZE)),
                    ),
            )
        })
        .child(
            div()
                .size(ICON_SIZE)
                .mr(tokens::spacing::SM)
                .flex_shrink_0()
                .when_some(spec.icon, |slot, icon| slot.child(Icon::new(icon))),
        )
        .child(div().min_w(px(0.)).truncate().child(spec.label))
        .child(
            div()
                .flex_1()
                .when(spec.shortcut.is_some() || spec.submenu, |gap| {
                    gap.min_w(LABEL_GAP)
                }),
        )
        .when_some(spec.shortcut, |item, shortcut| {
            item.child(
                div()
                    .min_w(px(0.))
                    .truncate()
                    .text_color(if selected {
                        colors.text_primary
                    } else {
                        colors.text_secondary
                    })
                    .child(shortcut),
            )
        })
        .when(shortcut_and_submenu, |row| {
            row.child(div().w(LABEL_GAP).flex_shrink_0())
        })
        .when(spec.submenu, |row| {
            row.child(Icon::new(IconName::ChevronRight).size(CHEVRON_SIZE))
        })
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::*;
    use gpui::{Context, Render, TestAppContext, VisualTestContext};

    struct Fixture {
        label: SharedString,
        shortcut: Option<SharedString>,
        submenu: bool,
        padding: Pixels,
    }

    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let specs = [
                MenuItemSpec {
                    id: "fixed".into(),
                    label: "Fixed".into(),
                    shortcut: None,
                    icon: None,
                    enabled: true,
                    selected: false,
                    indent: 0,
                    tree: None,
                    submenu: false,
                    separator_before: false,
                },
                MenuItemSpec {
                    id: "variable".into(),
                    label: self.label.clone(),
                    shortcut: self.shortcut.clone(),
                    icon: None,
                    enabled: true,
                    selected: false,
                    indent: 0,
                    tree: None,
                    submenu: self.submenu,
                    separator_before: true,
                },
            ];
            let entries = specs
                .into_iter()
                .map(|spec| MenuEntry::new(spec, cx, |row| row.into_any_element()))
                .collect();
            div().flex().flex_col().items_start().child(
                Menu::new("fixture", entries)
                    .debug_selector("fit-menu")
                    .padding(self.padding),
            )
        }
    }

    fn width(visual: &mut VisualTestContext) -> Pixels {
        visual.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear();
        });
        visual.debug_bounds("fit-menu").unwrap().size.width
    }

    #[gpui::test]
    fn selected_menu_uses_tab_focus_colors_and_separator_contrasts_in_both_themes(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            for id in ["lapis.dark", "lapis.white"] {
                theme::init(&crate::extension_ui::ThemeId::new(id), None, cx);
                let colors = theme::colors(cx);
                let mut spec = MenuItemSpec {
                    id: "selection".into(),
                    label: "Selected".into(),
                    shortcut: None,
                    icon: None,
                    enabled: true,
                    selected: true,
                    indent: 0,
                    tree: None,
                    submenu: false,
                    separator_before: false,
                };
                let mut selected = menu_item(spec.clone(), cx);
                assert_eq!(
                    selected.style().background,
                    Some(colors.button_background_focused.into())
                );
                assert_eq!(
                    selected.style().border_color,
                    Some(colors.button_border_focused.into())
                );
                assert_ne!(
                    colors.button_background_focused,
                    colors.button_background_hover
                );
                let mut separator = super::super::separator(cx);
                assert_eq!(
                    separator.style().background,
                    Some(colors.floating_border.into())
                );
                assert_ne!(colors.floating_border, colors.floating_background);
                spec.enabled = false;
                assert!(menu_item(spec, cx).style().background.is_none());
            }
        });
    }

    #[gpui::test]
    fn width_tracks_longest_entry_trailing_content_padding_and_viewport(cx: &mut TestAppContext) {
        let (fixture, visual) = cx.add_window_view(|_, cx| {
            theme::init(&crate::extension_ui::ThemeId::new("lapis.dark"), None, cx);
            Fixture {
                label: "Tiny".into(),
                shortcut: None,
                submenu: false,
                padding: px(4.),
            }
        });
        let short = width(visual);
        fixture.update(visual, |fixture, cx| {
            fixture.label = "A substantially longer menu label".into();
            cx.notify();
        });
        let long = width(visual);
        assert!(long > short);
        fixture.update(visual, |fixture, cx| {
            fixture.shortcut = Some("Ctrl+Shift+P".into());
            cx.notify();
        });
        let shortcut = width(visual);
        assert!(shortcut > long);
        fixture.update(visual, |fixture, cx| {
            fixture.submenu = true;
            cx.notify();
        });
        let submenu = width(visual);
        assert!(submenu > shortcut);
        fixture.update(visual, |fixture, cx| {
            fixture.padding = px(10.);
            cx.notify();
        });
        assert_eq!(width(visual) - submenu, px(12.));
        fixture.update(visual, |fixture, cx| {
            fixture.label = "Very long label ".repeat(200).into();
            cx.notify();
        });
        let viewport = visual.update(|window, _| window.viewport_size());
        assert_eq!(width(visual), viewport.width - tokens::spacing::SM * 2.);
        fixture.update(visual, |fixture, cx| {
            fixture.label = "Tiny".into();
            fixture.shortcut = None;
            fixture.submenu = false;
            fixture.padding = px(4.);
            cx.notify();
        });
        assert_eq!(width(visual), short);
    }
}
