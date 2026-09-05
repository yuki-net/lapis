use std::{collections::BTreeSet, fmt};

use serde::Deserialize;

use super::registry;

const FORMAT_VERSION: u32 = 1;
const MAX_NODES: usize = 512;
const MAX_DEPTH: usize = 32;

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(super) struct UiDefinition {
    pub(super) version: u32,
    pub(super) root: UiNode,
}

impl UiDefinition {
    pub(super) fn parse(source: &str) -> Result<Self, String> {
        let definition = toml::from_str::<Self>(source)
            .map_err(|error| format!("TOML parse failed: {error}"))?;
        definition.validate()?;
        Ok(definition)
    }

    pub(super) fn bundled() -> Result<Self, String> {
        Self::parse(include_str!("../../assets/hot-reload/demo.toml"))
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != FORMAT_VERSION {
            return Err(format!(
                "unsupported UI definition version: {} (expected {FORMAT_VERSION})",
                self.version
            ));
        }

        let mut ids = BTreeSet::new();
        let mut node_count = 0;
        validate_node(&self.root, &mut ids, &mut node_count, 0)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(super) struct UiNode {
    pub(super) id: String,
    pub(super) component: String,
    #[serde(default)]
    pub(super) text: Option<String>,
    #[serde(default)]
    pub(super) text_key: Option<String>,
    #[serde(default = "default_true")]
    pub(super) visible: bool,
    #[serde(default = "default_true")]
    pub(super) enabled: bool,
    #[serde(default)]
    pub(super) order: i32,
    #[serde(default)]
    pub(super) layout: UiLayout,
    #[serde(default)]
    pub(super) style: UiStyle,
    #[serde(default)]
    pub(super) children: Vec<UiNode>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub(super) struct UiLayout {
    pub(super) direction: Option<String>,
    pub(super) gap: Option<f32>,
    pub(super) align: Option<String>,
    pub(super) justify: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub(super) struct UiStyle {
    pub(super) width: Option<f32>,
    pub(super) height: Option<f32>,
    pub(super) min_width: Option<f32>,
    pub(super) min_height: Option<f32>,
    pub(super) max_width: Option<f32>,
    pub(super) max_height: Option<f32>,
    pub(super) padding: Option<f32>,
    pub(super) padding_x: Option<f32>,
    pub(super) padding_y: Option<f32>,
    pub(super) margin: Option<f32>,
    pub(super) margin_x: Option<f32>,
    pub(super) margin_y: Option<f32>,
    pub(super) background: Option<String>,
    pub(super) foreground: Option<String>,
    pub(super) border: Option<f32>,
    pub(super) border_color: Option<String>,
    pub(super) radius: Option<f32>,
    pub(super) opacity: Option<f32>,
    pub(super) font_size: Option<f32>,
    pub(super) font_weight: Option<f32>,
    pub(super) line_height: Option<f32>,
}

#[derive(Clone, Debug)]
pub(super) struct DefinitionStore {
    current: UiDefinition,
    generation: u64,
    last_error: Option<String>,
    last_parse_ms: Option<u128>,
    last_save_latency_ms: Option<u128>,
}

impl DefinitionStore {
    pub(super) fn new(initial: UiDefinition) -> Self {
        Self {
            current: initial,
            generation: 0,
            last_error: None,
            last_parse_ms: None,
            last_save_latency_ms: None,
        }
    }

    pub(super) fn current(&self) -> &UiDefinition {
        &self.current
    }

    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    pub(super) fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub(super) fn last_parse_ms(&self) -> Option<u128> {
        self.last_parse_ms
    }

    pub(super) fn last_save_latency_ms(&self) -> Option<u128> {
        self.last_save_latency_ms
    }

    #[cfg(any(debug_assertions, test))]
    pub(super) fn commit(
        &mut self,
        definition: UiDefinition,
        parse_ms: u128,
        save_latency_ms: Option<u128>,
    ) {
        self.current = definition;
        self.generation = self.generation.saturating_add(1);
        self.last_error = None;
        self.last_parse_ms = Some(parse_ms);
        self.last_save_latency_ms = save_latency_ms;
    }

    #[cfg(any(debug_assertions, test))]
    pub(super) fn reject(&mut self, error: String) {
        self.last_error = Some(error);
    }
}

fn validate_node(
    node: &UiNode,
    ids: &mut BTreeSet<String>,
    node_count: &mut usize,
    depth: usize,
) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err(format!("node '{}' exceeds maximum nesting depth", node.id));
    }
    *node_count += 1;
    if *node_count > MAX_NODES {
        return Err(format!("UI definition exceeds {MAX_NODES} nodes"));
    }
    if node.id.trim().is_empty() {
        return Err("component id must not be empty".to_owned());
    }
    if !ids.insert(node.id.clone()) {
        return Err(format!("duplicate component id: {}", node.id));
    }
    if registry::component_kind(&node.component).is_none() {
        return Err(format!(
            "component '{}' is not registered for hot reload",
            node.component
        ));
    }
    if node.text.is_some() && node.text_key.is_some() {
        return Err(format!(
            "component '{}' cannot define both text and text_key",
            node.id
        ));
    }
    if let Some(text) = &node.text
        && text.chars().count() > 4096
    {
        return Err(format!("component '{}' text is too long", node.id));
    }
    if let Some(key) = &node.text_key
        && registry::text_for_key(key).is_none()
    {
        return Err(format!(
            "component '{}' has unknown text_key '{key}'",
            node.id
        ));
    }

    validate_choice(
        &format!("component '{}' layout.direction", node.id),
        node.layout.direction.as_deref(),
        &["row", "column", "row-reverse", "column-reverse"],
    )?;
    validate_choice(
        &format!("component '{}' layout.align", node.id),
        node.layout.align.as_deref(),
        &["start", "center", "end", "stretch", "baseline"],
    )?;
    validate_choice(
        &format!("component '{}' layout.justify", node.id),
        node.layout.justify.as_deref(),
        &[
            "start",
            "center",
            "end",
            "space-between",
            "space-around",
            "space-evenly",
        ],
    )?;
    validate_non_negative(
        &format!("component '{}' layout.gap", node.id),
        node.layout.gap,
    )?;

    for (name, value) in [
        ("width", node.style.width),
        ("height", node.style.height),
        ("min_width", node.style.min_width),
        ("min_height", node.style.min_height),
        ("max_width", node.style.max_width),
        ("max_height", node.style.max_height),
        ("padding", node.style.padding),
        ("padding_x", node.style.padding_x),
        ("padding_y", node.style.padding_y),
        ("margin", node.style.margin),
        ("margin_x", node.style.margin_x),
        ("margin_y", node.style.margin_y),
        ("border", node.style.border),
        ("radius", node.style.radius),
        ("font_size", node.style.font_size),
        ("line_height", node.style.line_height),
    ] {
        validate_non_negative(&format!("component '{}' style.{name}", node.id), value)?;
    }
    validate_range(
        &format!("component '{}' style.opacity", node.id),
        node.style.opacity,
        0.0,
        1.0,
    )?;
    validate_range(
        &format!("component '{}' style.font_weight", node.id),
        node.style.font_weight,
        100.0,
        900.0,
    )?;

    for (name, value) in [
        ("background", node.style.background.as_ref()),
        ("foreground", node.style.foreground.as_ref()),
        ("border_color", node.style.border_color.as_ref()),
    ] {
        if let Some(value) = value {
            validate_color(&format!("component '{}' style.{name}", node.id), value)?;
        }
    }

    for child in &node.children {
        validate_node(child, ids, node_count, depth + 1)?;
    }
    Ok(())
}

fn validate_choice(name: &str, value: Option<&str>, allowed: &[&str]) -> Result<(), String> {
    if let Some(value) = value
        && !allowed.contains(&value)
    {
        return Err(format!(
            "{name} has unsupported value '{value}' (expected one of {})",
            allowed.join(", ")
        ));
    }
    Ok(())
}

fn validate_non_negative(name: &str, value: Option<f32>) -> Result<(), String> {
    if let Some(value) = value
        && (!value.is_finite() || value < 0.0)
    {
        return Err(format!("{name} must be a finite non-negative number"));
    }
    Ok(())
}

fn validate_range(name: &str, value: Option<f32>, min: f32, max: f32) -> Result<(), String> {
    if let Some(value) = value
        && (!value.is_finite() || !(min..=max).contains(&value))
    {
        return Err(format!("{name} must be between {min} and {max}"));
    }
    Ok(())
}

fn validate_color(name: &str, value: &str) -> Result<(), String> {
    if let Some(hex) = value.strip_prefix('#') {
        if (hex.len() != 6 && hex.len() != 8)
            || !hex.chars().all(|character| character.is_ascii_hexdigit())
        {
            return Err(format!("{name} must be a 6 or 8 digit hex color"));
        }
        return Ok(());
    }
    if registry::is_registered_token(value) {
        Ok(())
    } else {
        Err(format!(
            "{name} must reference a registered theme token or a hex color"
        ))
    }
}

fn default_true() -> bool {
    true
}

impl fmt::Display for DefinitionStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "generation {}, error={}",
            self.generation,
            self.last_error.as_deref().unwrap_or("none")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_definition_is_valid_and_contains_nested_components() {
        let definition = UiDefinition::bundled().unwrap();
        assert_eq!(definition.version, FORMAT_VERSION);
        assert_eq!(definition.root.id, "demo-shell");
        assert!(definition.root.children.iter().any(|node| {
            node.id == "body" && node.children.iter().any(|child| child.id == "sidebar")
        }));
    }

    #[test]
    fn invalid_definition_is_rejected_without_replacing_the_last_success() {
        let initial = UiDefinition::bundled().unwrap();
        let mut store = DefinitionStore::new(initial.clone());

        let invalid = UiDefinition::parse(
            r#"
version = 1

[root]
id = "demo-shell"
component = "not-registered"
"#,
        );
        assert!(invalid.is_err());

        store.reject("invalid component".to_owned());
        assert_eq!(store.current(), &initial);
        assert_eq!(store.generation(), 0);
        assert_eq!(store.last_error(), Some("invalid component"));
    }

    #[test]
    fn validation_rejects_duplicate_ids_and_invalid_color_tokens() {
        let duplicate = r#"
version = 1

[root]
id = "same"
component = "container"

[[root.children]]
id = "same"
component = "text"
text = "duplicate"
"#;
        assert!(
            UiDefinition::parse(duplicate)
                .unwrap_err()
                .contains("duplicate component id")
        );

        let invalid_color = r#"
version = 1

[root]
id = "root"
component = "container"

[root.style]
background = "token.not_registered"
"#;
        assert!(
            UiDefinition::parse(invalid_color)
                .unwrap_err()
                .contains("theme token")
        );
    }

    #[test]
    fn definition_store_clears_error_only_after_a_valid_commit() {
        let initial = UiDefinition::bundled().unwrap();
        let mut store = DefinitionStore::new(initial.clone());
        store.reject("syntax error".to_owned());
        assert!(store.last_error().is_some());

        store.commit(initial, 3, Some(12));
        assert_eq!(store.last_error(), None);
        assert_eq!(store.last_parse_ms(), Some(3));
        assert_eq!(store.last_save_latency_ms(), Some(12));
        assert_eq!(store.generation(), 1);
    }
}
