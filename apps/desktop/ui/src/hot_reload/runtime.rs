#[cfg(debug_assertions)]
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

#[cfg(debug_assertions)]
use gpui::Timer;
use gpui::{AnyElement, Context, Div, FontWeight, Render, Rgba, div, prelude::*, px};

use crate::theme;

use super::{
    definition::{self, DefinitionStore, UiLayout, UiNode, UiStyle},
    registry::{self, ComponentKind},
};

pub(crate) struct HotReloadDemo {
    definition: DefinitionStore,
    #[cfg(debug_assertions)]
    definition_path: PathBuf,
    clicks: u32,
}

impl HotReloadDemo {
    pub(crate) fn new(_cx: &mut Context<Self>) -> Self {
        let initial = definition::UiDefinition::bundled()
            .expect("bundled hot reload definition must be valid");
        let demo = Self {
            definition: DefinitionStore::new(initial),
            #[cfg(debug_assertions)]
            definition_path: super::definition_path(),
            clicks: 0,
        };
        #[cfg(debug_assertions)]
        let mut demo = demo;
        #[cfg(debug_assertions)]
        demo.start_watcher(_cx);
        demo
    }

    #[cfg(debug_assertions)]
    fn start_watcher(&mut self, cx: &mut Context<Self>) {
        let path = self.definition_path.clone();
        cx.spawn(async move |this, cx| {
            let mut fingerprint = None;
            loop {
                Timer::after(Duration::from_millis(200)).await;
                let path_for_load = path.clone();
                let previous = fingerprint.clone();
                let poll = cx
                    .background_spawn(
                        async move { poll_definition(&path_for_load, previous.as_ref()) },
                    )
                    .await;
                let PollResult::Changed {
                    fingerprint: next,
                    definition,
                    parse_ms,
                    save_latency_ms,
                } = poll
                else {
                    continue;
                };
                fingerprint = Some(next.clone());

                if this
                    .update(cx, |demo, cx| {
                        demo.apply_poll(next, *definition, parse_ms, save_latency_ms, cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    #[cfg(debug_assertions)]
    fn apply_poll(
        &mut self,
        _fingerprint: Fingerprint,
        result: Result<definition::UiDefinition, String>,
        parse_ms: u128,
        save_latency_ms: Option<u128>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(definition) => {
                self.definition
                    .commit(definition, parse_ms, save_latency_ms);
                eprintln!(
                    "[hot-reload] committed {} in {} ms",
                    self.definition_path.display(),
                    parse_ms
                );
            }
            Err(error) => {
                let error = format!("{}: {error}", self.definition_path.display());
                eprintln!("[hot-reload] {error}");
                self.definition.reject(error);
            }
        }
        cx.notify();
    }

    fn render_node(&self, node: &UiNode, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors();
        let kind = registry::component_kind(&node.component)
            .expect("validated UI definitions only contain registered components");
        let mut element = div().flex();

        match kind {
            ComponentKind::Text => {
                element = element.child(registry::resolve_text(node));
            }
            ComponentKind::Counter => {
                element =
                    element.child(format!("{}: {}", registry::resolve_text(node), self.clicks));
            }
            ComponentKind::Button => {
                element = element
                    .cursor_pointer()
                    .bg(colors.button_background)
                    .border_1()
                    .border_color(colors.button_border)
                    .rounded(px(6.0))
                    .px(px(12.0))
                    .py(px(8.0))
                    .child(registry::resolve_text(node));
            }
            ComponentKind::Surface => {
                element = element
                    .bg(colors.background_secondary)
                    .border_1()
                    .border_color(colors.border_default)
                    .rounded(px(6.0));
            }
            ComponentKind::Badge => {
                element = element
                    .bg(colors.button_background_selected)
                    .text_color(colors.text_primary)
                    .rounded(px(9999.0))
                    .px(px(8.0))
                    .py(px(4.0))
                    .child(registry::resolve_text(node));
            }
            ComponentKind::Container => {}
        }

        element = apply_layout(element, &node.layout);
        element = apply_style(element, &node.style, &colors);
        if !node.visible {
            element = element.invisible();
        }
        if !node.enabled {
            element = element.opacity(0.55);
        }
        let mut children = node.children.iter().enumerate().collect::<Vec<_>>();
        children.sort_by_key(|(index, child)| (child.order, *index));
        let mut element = element
            .children(
                children
                    .into_iter()
                    .map(|(_, child)| self.render_node(child, cx)),
            )
            .id(gpui::SharedString::from(node.id.clone()));
        if kind == ComponentKind::Button && node.enabled {
            element = element.on_click(cx.listener(|demo, _, _, cx| {
                demo.clicks = demo.clicks.saturating_add(1);
                cx.notify();
            }));
        }
        element.into_any_element()
    }

    fn status_line(&self) -> String {
        let mode = if cfg!(debug_assertions) {
            "development polling watcher"
        } else {
            "release embedded definition"
        };
        let parse = self
            .definition
            .last_parse_ms()
            .map(|value| format!("{value}ms"))
            .unwrap_or_else(|| "-".to_owned());
        let save = self
            .definition
            .last_save_latency_ms()
            .map(|value| format!("{value}ms"))
            .unwrap_or_else(|| "-".to_owned());
        format!(
            "{mode} · generation {} · parse {parse} · observed save→visible {save}",
            self.definition.generation()
        )
    }
}

impl Render for HotReloadDemo {
    fn render(&mut self, _window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = theme::colors();
        let status_color = if self.definition.last_error().is_some() {
            colors.danger_text
        } else {
            colors.text_secondary
        };
        let error = self.definition.last_error().map(ToOwned::to_owned);
        let root = div()
            .flex_1()
            .min_h(px(0.0))
            .child(self.render_node(&self.definition.current().root, cx));

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.background_primary)
            .text_color(colors.text_primary)
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(16.0))
                    .p(px(16.0))
                    .border_b_1()
                    .border_color(colors.border_default)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .text_size(px(16.0))
                                    .font_weight(FontWeight::BOLD)
                                    .child("Hot Reload Demo"),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(colors.text_secondary)
                                    .child("TOML definition → typed tree → GPUI GPU renderer"),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(status_color)
                            .child(self.status_line()),
                    ),
            )
            .when_some(error, |element, error| {
                element.child(
                    div()
                        .flex_none()
                        .px(px(16.0))
                        .py(px(8.0))
                        .bg(colors.danger_background)
                        .text_color(colors.danger_text)
                        .child(error),
                )
            })
            .child(root)
    }
}

fn apply_layout(mut element: Div, layout: &UiLayout) -> Div {
    if let Some(direction) = layout.direction.as_deref() {
        element = match direction {
            "row" => element.flex_row(),
            "column" => element.flex_col(),
            "row-reverse" => element.flex_row_reverse(),
            "column-reverse" => element.flex_col_reverse(),
            _ => element,
        };
    }
    if let Some(gap) = layout.gap {
        element = element.gap(px(gap));
    }
    if let Some(align) = layout.align.as_deref() {
        element.style().align_items = Some(match align {
            "start" => gpui::AlignItems::Start,
            "center" => gpui::AlignItems::Center,
            "end" => gpui::AlignItems::End,
            "stretch" => gpui::AlignItems::Stretch,
            "baseline" => gpui::AlignItems::Baseline,
            _ => gpui::AlignItems::Stretch,
        });
    }
    if let Some(justify) = layout.justify.as_deref() {
        element.style().justify_content = Some(match justify {
            "start" => gpui::JustifyContent::Start,
            "center" => gpui::JustifyContent::Center,
            "end" => gpui::JustifyContent::End,
            "space-between" => gpui::JustifyContent::SpaceBetween,
            "space-around" => gpui::JustifyContent::SpaceAround,
            "space-evenly" => gpui::JustifyContent::SpaceEvenly,
            _ => gpui::JustifyContent::Start,
        });
    }
    element
}

fn apply_style(mut element: Div, style: &UiStyle, colors: &crate::theme::ThemeColors) -> Div {
    if let Some(width) = style.width {
        element = element.w(px(width));
    }
    if let Some(height) = style.height {
        element = element.h(px(height));
    }
    if let Some(width) = style.min_width {
        element = element.min_w(px(width));
    }
    if let Some(height) = style.min_height {
        element = element.min_h(px(height));
    }
    if let Some(width) = style.max_width {
        element = element.max_w(px(width));
    }
    if let Some(height) = style.max_height {
        element = element.max_h(px(height));
    }
    if let Some(padding) = style.padding {
        element = element.p(px(padding));
    }
    if let Some(padding) = style.padding_x {
        element = element.px(px(padding));
    }
    if let Some(padding) = style.padding_y {
        element = element.py(px(padding));
    }
    if let Some(margin) = style.margin {
        element = element.m(px(margin));
    }
    if let Some(margin) = style.margin_x {
        element = element.mx(px(margin));
    }
    if let Some(margin) = style.margin_y {
        element = element.my(px(margin));
    }
    if let Some(background) = style
        .background
        .as_deref()
        .and_then(|value| resolve_color(value, colors))
    {
        element = element.bg(background);
    }
    if let Some(foreground) = style
        .foreground
        .as_deref()
        .and_then(|value| resolve_color(value, colors))
    {
        element = element.text_color(foreground);
    }
    if let Some(border) = style.border {
        element = element.border(px(border));
    }
    if let Some(border_color) = style
        .border_color
        .as_deref()
        .and_then(|value| resolve_color(value, colors))
    {
        element = element.border_color(border_color);
    }
    if let Some(radius) = style.radius {
        element = element.rounded(px(radius));
    }
    if let Some(opacity) = style.opacity {
        element = element.opacity(opacity);
    }
    if let Some(font_size) = style.font_size {
        element = element.text_size(px(font_size));
    }
    if let Some(font_weight) = style.font_weight {
        element = element.font_weight(FontWeight(font_weight));
    }
    if let Some(line_height) = style.line_height {
        element = element.line_height(px(line_height));
    }
    element
}

fn resolve_color(value: &str, colors: &crate::theme::ThemeColors) -> Option<Rgba> {
    if let Some(hex) = value.strip_prefix('#') {
        let mut raw = u32::from_str_radix(hex, 16).ok()?;
        if hex.len() == 6 {
            raw = (raw << 8) | 0xff;
        }
        return Some(gpui::rgba(raw));
    }

    match value.strip_prefix("token.").unwrap_or(value) {
        "background_primary" => Some(colors.background_primary),
        "background_secondary" => Some(colors.background_secondary),
        "background_tertiary" => Some(colors.background_tertiary),
        "floating_background" => Some(colors.floating_background),
        "floating_border" => Some(colors.floating_border),
        "text_primary" => Some(colors.text_primary),
        "text_secondary" => Some(colors.text_secondary),
        "text_tertiary" => Some(colors.text_tertiary),
        "text_accent" => Some(colors.text_accent),
        "text_positive" => Some(colors.text_positive),
        "text_warning" => Some(colors.text_warning),
        "text_dangerous" => Some(colors.text_dangerous),
        "button_background" => Some(colors.button_background),
        "button_background_hover" => Some(colors.button_background_hover),
        "button_background_selected" => Some(colors.button_background_selected),
        "button_background_focused" => Some(colors.button_background_focused),
        "button_border" => Some(colors.button_border),
        "button_border_selected" => Some(colors.button_border_selected),
        "button_border_focused" => Some(colors.button_border_focused),
        "border_default" => Some(colors.border_default),
        "positive_background" => Some(colors.positive_background),
        "positive_background_hover" => Some(colors.positive_background_hover),
        "positive_border" => Some(colors.positive_border),
        "positive_text" => Some(colors.positive_text),
        "warning_background" => Some(colors.warning_background),
        "warning_background_hover" => Some(colors.warning_background_hover),
        "warning_border" => Some(colors.warning_border),
        "warning_text" => Some(colors.warning_text),
        "danger_background" => Some(colors.danger_background),
        "danger_background_hover" => Some(colors.danger_background_hover),
        "danger_border" => Some(colors.danger_border),
        "danger_text" => Some(colors.danger_text),
        "info_background" => Some(colors.info_background),
        "info_background_hover" => Some(colors.info_background_hover),
        "info_border" => Some(colors.info_border),
        "info_text" => Some(colors.info_text),
        "editor_caret" => Some(colors.editor_caret),
        "editor_selection" => Some(colors.editor_selection),
        "editor_search_match" => Some(colors.editor_search_match),
        "editor_current_line" => Some(colors.editor_current_line),
        _ => None,
    }
}

#[cfg(debug_assertions)]
#[derive(Clone, Debug, Eq, PartialEq)]
enum Fingerprint {
    Missing,
    Unavailable(String),
    Present {
        length: u64,
        modified: Option<SystemTime>,
    },
}

#[cfg(debug_assertions)]
enum PollResult {
    Unchanged,
    Changed {
        fingerprint: Fingerprint,
        definition: Box<Result<definition::UiDefinition, String>>,
        parse_ms: u128,
        save_latency_ms: Option<u128>,
    },
}

#[cfg(debug_assertions)]
fn poll_definition(path: &Path, previous: Option<&Fingerprint>) -> PollResult {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => {
            let fingerprint = if error.kind() == std::io::ErrorKind::NotFound {
                Fingerprint::Missing
            } else {
                Fingerprint::Unavailable(error.to_string())
            };
            if previous == Some(&fingerprint) {
                return PollResult::Unchanged;
            }
            return PollResult::Changed {
                fingerprint,
                definition: Box::new(Err(format!("definition file cannot be read: {error}"))),
                parse_ms: 0,
                save_latency_ms: None,
            };
        }
    };

    let fingerprint = Fingerprint::Present {
        length: metadata.len(),
        modified: metadata.modified().ok(),
    };
    if previous == Some(&fingerprint) {
        return PollResult::Unchanged;
    }

    let started = Instant::now();
    let definition = fs::read_to_string(path)
        .map_err(|error| format!("definition file cannot be read: {error}"))
        .and_then(|source| definition::UiDefinition::parse(&source));
    let parse_ms = started.elapsed().as_millis();
    let save_latency_ms = metadata
        .modified()
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .map(|duration| duration.as_millis());

    PollResult::Changed {
        fingerprint,
        definition: Box::new(definition),
        parse_ms,
        save_latency_ms,
    }
}
