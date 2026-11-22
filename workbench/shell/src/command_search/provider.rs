use crate::{extension_ui::FeatureRegistry, localization::Localizer};
use extension_ui_contract::{CommandId, UiContribution, UiSlot};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchItem {
    pub command: CommandId,
    pub title: String,
    pub shortcut: String,
}

pub trait SearchProvider {
    fn search(&self, query: &str) -> Vec<SearchItem>;
}

#[derive(Clone, Debug, Default)]
pub struct CommandSearchProvider {
    items: Vec<SearchItem>,
}

impl CommandSearchProvider {
    pub fn from_registry(
        registry: &FeatureRegistry,
        locale: &Localizer,
        shortcut: impl Fn(&CommandId) -> String,
        view_title: impl Fn(&UiContribution) -> String,
    ) -> Self {
        let mut items: Vec<_> = registry
            .contributions(UiSlot::CommandPalette)
            .into_iter()
            .filter_map(|contribution| {
                let command = contribution.command.clone()?;
                Some(SearchItem {
                    title: locale.resolve(&contribution.title),
                    shortcut: shortcut(&command),
                    command,
                })
            })
            .collect();
        for (view, command) in registry.view_open_commands() {
            if let Some(contribution) = registry.declaration(view) {
                items.push(SearchItem {
                    title: view_title(contribution),
                    shortcut: shortcut(command),
                    command: command.clone(),
                });
            }
        }
        Self { items }
    }

    pub fn new(items: Vec<SearchItem>) -> Self {
        Self { items }
    }
}

impl SearchProvider for CommandSearchProvider {
    fn search(&self, query: &str) -> Vec<SearchItem> {
        let normalized = query.trim().to_lowercase();
        let terms = normalized.split_whitespace().collect::<Vec<_>>();
        let mut matches = self
            .items
            .iter()
            .filter_map(|item| {
                let title = item.title.to_lowercase();
                let command = item.command.as_str().to_lowercase();
                let haystack = format!("{title} {command}");
                if !terms.iter().all(|term| haystack.contains(term)) {
                    return None;
                }
                let score = if normalized.is_empty() {
                    3
                } else if title.starts_with(&normalized) {
                    0
                } else if title.contains(&normalized) {
                    1
                } else {
                    2
                };
                Some((score, item.clone()))
            })
            .collect::<Vec<_>>();
        matches.sort_by(|(left_score, left), (right_score, right)| {
            left_score
                .cmp(right_score)
                .then_with(|| left.title.cmp(&right.title))
        });
        matches.into_iter().map(|(_, item)| item).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_commands_appear_only_when_the_view_is_available() {
        use crate::extension_ui::ViewLifecycle;
        use extension_ui_contract::{FeatureDescriptor, UiContribution, UiSlot, ViewId};
        use std::{cell::RefCell, rc::Rc};

        struct Lifecycle;
        impl ViewLifecycle for Lifecycle {}
        let mut registry = FeatureRegistry::default();
        let view = ViewId::new("example.view");
        registry
            .register(
                FeatureDescriptor::core("example").contributes(
                    UiContribution::view(view.clone(), UiSlot::BottomDock, "title", "icon", 0)
                        .multiple_instances(),
                ),
            )
            .unwrap();
        registry.register_view_lifecycle(view.clone(), Rc::new(RefCell::new(Lifecycle)));
        registry.set_view_open_command(view.clone(), Some("example.open".into()));
        let locale = Localizer::bundled();
        let provider = CommandSearchProvider::from_registry(
            &registry,
            &locale,
            |_| String::new(),
            |_| "拡張の画面".to_owned(),
        );
        assert_eq!(provider.search("拡張")[0].command.as_str(), "example.open");
        registry.set_view_available(view, false);
        let provider = CommandSearchProvider::from_registry(
            &registry,
            &locale,
            |_| String::new(),
            |_| "拡張の画面".to_owned(),
        );
        assert!(provider.search("拡張").is_empty());
    }

    fn item(id: &str, title: &str) -> SearchItem {
        SearchItem {
            command: CommandId::new(id),
            title: title.to_owned(),
            shortcut: String::new(),
        }
    }

    #[test]
    fn command_provider_matches_title_and_stable_id_by_all_terms() {
        let provider = CommandSearchProvider::new(vec![
            item(
                "lapis.command.dev.toggle-devtool",
                "dev: Toggle Lapis DevTool",
            ),
            item("lapis.command.open-workspace", "Open Workspace…"),
        ]);

        assert_eq!(provider.search("toggle devtool").len(), 1);
        assert_eq!(
            provider.search("dev devtool")[0].title,
            "dev: Toggle Lapis DevTool"
        );
        assert_eq!(
            provider.search("open-workspace")[0].command.as_str(),
            "lapis.command.open-workspace"
        );
        assert!(provider.search("inspector workspace").is_empty());
    }

    #[test]
    fn command_provider_prefers_title_prefix_matches() {
        let provider = CommandSearchProvider::new(vec![
            item("command.alpha", "Toggle Alpha"),
            item("command.toggle", "Alpha Toggle"),
        ]);

        let matches = provider.search("alpha");
        assert_eq!(matches[0].title, "Alpha Toggle");
    }
}
