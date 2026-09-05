use super::definition::UiNode;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ComponentKind {
    Container,
    Surface,
    Text,
    Button,
    Counter,
    Badge,
}

pub(super) fn component_kind(name: &str) -> Option<ComponentKind> {
    match name {
        "container" => Some(ComponentKind::Container),
        "surface" => Some(ComponentKind::Surface),
        "text" => Some(ComponentKind::Text),
        "button" => Some(ComponentKind::Button),
        "counter" => Some(ComponentKind::Counter),
        "badge" => Some(ComponentKind::Badge),
        _ => None,
    }
}

pub(super) fn text_for_key(key: &str) -> Option<&'static str> {
    match key {
        "hot_reload.title" => Some("Hot Reload Demo"),
        "hot_reload.subtitle" => Some("GPUI renderer / host-owned state"),
        "hot_reload.header" => Some("Declarative definition"),
        "hot_reload.sidebar" => Some("Registered components"),
        "hot_reload.main" => Some("Main area"),
        "hot_reload.counter" => Some("Host counter"),
        "hot_reload.button" => Some("Increment counter"),
        "hot_reload.badge" => Some("GPU"),
        _ => None,
    }
}

pub(super) fn is_registered_token(value: &str) -> bool {
    let value = value.strip_prefix("token.").unwrap_or(value);
    matches!(
        value,
        "background_primary"
            | "background_secondary"
            | "background_tertiary"
            | "floating_background"
            | "floating_border"
            | "text_primary"
            | "text_secondary"
            | "text_tertiary"
            | "text_accent"
            | "text_positive"
            | "text_warning"
            | "text_dangerous"
            | "button_background"
            | "button_background_hover"
            | "button_background_selected"
            | "button_background_focused"
            | "button_border"
            | "button_border_selected"
            | "button_border_focused"
            | "border_default"
            | "positive_background"
            | "positive_background_hover"
            | "positive_border"
            | "positive_text"
            | "warning_background"
            | "warning_background_hover"
            | "warning_border"
            | "warning_text"
            | "danger_background"
            | "danger_background_hover"
            | "danger_border"
            | "danger_text"
            | "info_background"
            | "info_background_hover"
            | "info_border"
            | "info_text"
            | "editor_caret"
            | "editor_selection"
            | "editor_search_match"
            | "editor_current_line"
    )
}

pub(super) fn resolve_text(node: &UiNode) -> String {
    node.text
        .clone()
        .or_else(|| {
            node.text_key
                .as_deref()
                .and_then(text_for_key)
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| node.id.clone())
}
