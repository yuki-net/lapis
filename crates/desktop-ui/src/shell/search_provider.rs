use crate::{
    extension_ui::{CommandId, FeatureRegistry, UiSlot},
    keymap::KeymapRegistry,
    localization::LocaleRegistry,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SearchItem {
    pub command: CommandId,
    pub title: String,
    pub shortcut: String,
}

pub(crate) trait SearchProvider {
    fn search(&self, query: &str) -> Vec<SearchItem>;
}

#[derive(Clone, Debug, Default)]
pub(crate) struct CommandSearchProvider {
    items: Vec<SearchItem>,
}

impl CommandSearchProvider {
    pub(crate) fn from_registry(
        registry: &FeatureRegistry,
        locale: &LocaleRegistry,
        keymap: &KeymapRegistry,
    ) -> Self {
        let items = registry
            .contributions(UiSlot::CommandPalette)
            .into_iter()
            .filter_map(|contribution| {
                let command = contribution.command.clone()?;
                Some(SearchItem {
                    title: locale.resolve(&contribution.title),
                    shortcut: keymap.shortcut_label(&command),
                    command,
                })
            })
            .collect();
        Self { items }
    }

    #[cfg(test)]
    fn new(items: Vec<SearchItem>) -> Self {
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
                "lapis.command.dev.toggle-inspector",
                "dev: Toggle Inspector",
            ),
            item("lapis.command.open-workspace", "Open Workspace…"),
        ]);

        assert_eq!(provider.search("toggle inspector").len(), 1);
        assert_eq!(provider.search("dev inspector")[0].title, "dev: Toggle Inspector");
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
