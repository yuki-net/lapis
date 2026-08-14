use crate::extension_ui::{FeatureDescriptor, FeatureRegistry};

pub(crate) mod command_search;
pub(crate) mod conversation;
pub(crate) mod editor;
mod files;
mod git;
mod preview;
mod problems;
mod search;
mod tasks;
mod terminal;

pub mod id {
    pub const FEATURE_SHELL: &str = "core.shell";
    pub const FEATURE_EDITOR: &str = "core.editor";
    pub const FEATURE_COMMAND_SEARCH: &str = "core.command-search";
    #[cfg(debug_assertions)]
    pub const FEATURE_DEVTOOLS: &str = "bundled.devtools";
    pub const FEATURE_CONVERSATION: &str = "core.conversation";
    pub const FEATURE_FILES: &str = "bundled.files";
    pub const FEATURE_SEARCH: &str = "bundled.search";
    pub const FEATURE_GIT: &str = "bundled.git";
    pub const FEATURE_HISTORY: &str = "bundled.history";
    pub const FEATURE_PREVIEW: &str = "bundled.preview";
    pub const FEATURE_ASSISTANT: &str = "bundled.assistant";
    pub const FEATURE_TERMINAL: &str = "bundled.terminal";
    pub const FEATURE_PROBLEMS: &str = "bundled.problems";
    pub const FEATURE_OUTPUT: &str = "bundled.output";
    pub const FEATURE_RUST: &str = "bundled.language.rust";
    pub const FEATURE_MARKDOWN: &str = "bundled.language.markdown";

    pub const COMMAND_FIND_WORKSPACE: &str = "lapis.command.find-workspace";
    pub const COMMAND_NEW_DOCUMENT: &str = "lapis.command.new-document";
    pub const COMMAND_OPEN_WORKSPACE: &str = "lapis.command.open-workspace";
    pub const COMMAND_SAVE_DOCUMENT: &str = "lapis.command.save-document";
    pub const COMMAND_TOGGLE_PREVIEW: &str = "lapis.command.toggle-preview";
    pub const COMMAND_TOGGLE_BOTTOM: &str = "lapis.command.toggle-bottom-panel";
    pub const COMMAND_TOGGLE_ASSISTANT: &str = "lapis.command.toggle-assistant";
    pub const COMMAND_START_CODEX: &str = "lapis.command.start-codex";
    pub const COMMAND_START_TERMINAL: &str = "lapis.command.start-terminal";
    #[cfg(debug_assertions)]
    pub const COMMAND_TOGGLE_INSPECTOR: &str = "lapis.command.dev.toggle-inspector";

    pub const VIEW_FILES: &str = "lapis.view.files";
    pub const VIEW_SEARCH: &str = "lapis.view.search";
    pub const VIEW_GIT: &str = "lapis.view.git";
    pub const VIEW_HISTORY: &str = "lapis.view.history";
    pub const VIEW_PREVIEW: &str = "lapis.view.preview";
    pub const VIEW_ASSISTANT: &str = "lapis.view.assistant";
    pub const VIEW_TERMINAL: &str = "lapis.view.terminal";
    pub const VIEW_PROBLEMS: &str = "lapis.view.problems";
    pub const VIEW_OUTPUT: &str = "lapis.view.output";
    pub const VIEW_COMMAND_SEARCH: &str = "lapis.view.command-search";
    pub const VIEW_SETTINGS: &str = "lapis.view.settings";
}

pub fn bundled_registry() -> FeatureRegistry {
    let mut registry = FeatureRegistry::default();
    let descriptors = vec![
        FeatureDescriptor::core(id::FEATURE_SHELL).contributes(
            crate::extension_ui::UiContribution::settings_view(
                id::VIEW_SETTINGS,
                "view.settings",
                "settings",
            ),
        ),
        FeatureDescriptor::core(id::FEATURE_COMMAND_SEARCH).contributes(
            crate::extension_ui::UiContribution::view(
                id::VIEW_COMMAND_SEARCH,
                crate::extension_ui::UiSlot::SideDock,
                "view.command-search",
                "search",
                5,
            ),
        ),
        editor::descriptor(),
        conversation::descriptor(),
        files::descriptor(),
        search::descriptor(),
        git::descriptor(),
        git::history_descriptor(),
        preview::descriptor(),
        tasks::descriptor(),
        terminal::descriptor(),
        problems::descriptor(),
        problems::output_descriptor(),
        editor::rust_descriptor(),
        preview::markdown_descriptor(),
    ];
    #[cfg(debug_assertions)]
    let descriptors = {
        let mut descriptors = descriptors;
        descriptors.push(
            FeatureDescriptor::bundled(
                id::FEATURE_DEVTOOLS,
                [crate::extension_ui::ActivationCondition::OnCommand(
                    id::COMMAND_TOGGLE_INSPECTOR.into(),
                )],
            )
            .contributes(crate::extension_ui::UiContribution::command(
                id::COMMAND_TOGGLE_INSPECTOR,
                "command.dev.toggle-inspector",
                "search",
                900,
            )),
        );
        descriptors
    };
    for descriptor in descriptors {
        registry
            .register(descriptor)
            .expect("built-in feature IDs are unique");
    }
    registry
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension_ui::{ActivationEvent, CommandId, UiSlot, ViewId};

    #[test]
    fn built_in_features_follow_documented_activation_boundaries() {
        let mut registry = bundled_registry();
        assert!(registry.state(id::FEATURE_EDITOR).running);
        assert!(!registry.state(id::FEATURE_FILES).loaded);
        assert!(!registry.state(id::FEATURE_GIT).running);

        registry.activate(ActivationEvent::WorkspaceOpened);
        assert!(registry.state(id::FEATURE_FILES).running);
        assert!(!registry.state(id::FEATURE_GIT).running);

        registry.set_active_view(UiSlot::ToolDock, Some(ViewId::new(id::VIEW_GIT)));
        assert!(registry.state(id::FEATURE_GIT).running);
        registry.set_active_view(UiSlot::ToolDock, Some(ViewId::new(id::VIEW_FILES)));
        assert!(!registry.state(id::FEATURE_GIT).running);

        registry.activate(ActivationEvent::CommandInvoked(CommandId::new(
            id::COMMAND_FIND_WORKSPACE,
        )));
        assert!(registry.state(id::FEATURE_SEARCH).running);
    }

    #[test]
    fn process_features_declare_workspace_process_capability() {
        for feature in [
            id::FEATURE_TERMINAL,
            id::FEATURE_ASSISTANT,
            id::FEATURE_RUST,
        ] {
            let descriptor = bundled_registry()
                .descriptor(feature)
                .expect("built-in descriptor exists")
                .clone();
            assert!(
                descriptor
                    .required_capabilities
                    .iter()
                    .any(|capability| capability.as_str() == "workspace.process")
            );
        }
    }

    #[test]
    fn settings_view_is_available_only_in_main_panel() {
        let registry = bundled_registry();
        let contribution = registry
            .panel_contributions(crate::extension_ui::PanelPosition::Main)
            .into_iter()
            .find(|contribution| {
                contribution
                    .view
                    .as_ref()
                    .is_some_and(|view| view.as_str() == id::VIEW_SETTINGS)
            })
            .expect("settings view is registered");

        assert_eq!(
            contribution.allowed_panels,
            [crate::extension_ui::PanelPosition::Main]
        );
        assert!(
            registry
                .panel_contributions(crate::extension_ui::PanelPosition::Left)
                .into_iter()
                .all(|candidate| candidate
                    .view
                    .as_ref()
                    .is_none_or(|view| view.as_str() != id::VIEW_SETTINGS))
        );
    }

    #[cfg(debug_assertions)]
    #[test]
    fn debug_build_registers_inspector_command() {
        let registry = bundled_registry();
        assert!(
            registry
                .contributions(UiSlot::CommandPalette)
                .iter()
                .any(|contribution| contribution
                    .command
                    .as_ref()
                    .is_some_and(|command| { command.as_str() == id::COMMAND_TOGGLE_INSPECTOR }))
        );
    }
}
