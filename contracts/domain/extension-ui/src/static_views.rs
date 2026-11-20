//! 拡張が Host に渡す表示宣言。描画器の内部型は含まない。

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::StaticContributions;

pub const STATIC_VIEWS_CONTRACT: &str = "lapis.extension.views";
pub const STATIC_VIEWS_VERSION: &str = "1.0.0";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StaticViews {
    pub schema: u32,
    pub views: Vec<ContributedView>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContributedView {
    pub id: String,
    pub content_type: String,
    pub title: String,
    pub default_zone: ContributedZone,
    #[serde(default)]
    pub multiple_instances: bool,
    #[serde(default)]
    pub open_command: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContributedZone {
    Main,
    Left,
    Bottom,
    Right,
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'_'))
}

impl StaticViews {
    /// ID は署名済み提供元に束縛し、表示名は同じ提供元の既定言語に必須とする。
    pub fn validate(&self, owner: &str, contributions: &StaticContributions) -> Result<(), String> {
        contributions.validate(owner)?;
        if self.schema != 1 || self.views.len() > 64 {
            return Err("未対応または過大な表示宣言".into());
        }
        let fallback = contributions
            .translations
            .iter()
            .find(|pack| pack.target == owner && pack.locale == contributions.default_locale);
        let mut views = BTreeSet::new();
        let mut commands = BTreeSet::new();
        let owned = |id: &str| {
            valid_identifier(id)
                && id.starts_with(&format!("{owner}."))
                && id.len() > owner.len() + 1
        };
        for view in &self.views {
            if !owned(&view.id)
                || !views.insert(&view.id)
                || !valid_identifier(&view.content_type)
                || !valid_identifier(&view.title)
                || !fallback.is_some_and(|pack| pack.messages.contains_key(&view.title))
                || view
                    .open_command
                    .as_ref()
                    .is_some_and(|command| !owned(command) || !commands.insert(command))
            {
                return Err("表示宣言が不正または重複しています".into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn views_require_owned_ids_and_localized_titles() {
        let text = r#"{"schema":1,"translations":[{"target":"example.extension","locale":"en-US","messages":{"terminal.title":"Terminal"}}]}"#;
        let contributions: StaticContributions = serde_json::from_str(text).unwrap();
        let source = r#"{"schema":1,"views":[{"id":"example.extension.view","content_type":"lapis.content.terminal-grid.v1","title":"terminal.title","default_zone":"bottom","multiple_instances":true,"open_command":"example.extension.open"}]}"#;
        let mut declaration: StaticViews = serde_json::from_str(source).unwrap();
        assert!(
            declaration
                .validate("example.extension", &contributions)
                .is_ok()
        );
        assert!(
            declaration
                .validate("other.extension", &contributions)
                .is_err()
        );
        declaration.views.push(declaration.views[0].clone());
        assert!(
            declaration
                .validate("example.extension", &contributions)
                .is_err()
        );
        declaration.views.pop();
        declaration.views[0].open_command = Some("other.extension.open".into());
        assert!(
            declaration
                .validate("example.extension", &contributions)
                .is_err()
        );
        declaration.views[0].open_command = None;
        let missing: StaticContributions = serde_json::from_str(r#"{"schema":1}"#).unwrap();
        assert!(declaration.validate("example.extension", &missing).is_err());
    }

    #[test]
    fn terminal_package_declares_a_valid_view() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../extensions/terminal/component");
        let contributions: StaticContributions =
            serde_json::from_slice(&std::fs::read(root.join("contributions.json")).unwrap())
                .unwrap();
        let views: StaticViews =
            serde_json::from_slice(&std::fs::read(root.join("views.json")).unwrap()).unwrap();
        assert!(views.validate("lapis.terminal", &contributions).is_ok());
        assert_eq!(
            views.views[0].content_type,
            "lapis.content.terminal-grid.v1"
        );
    }
}
