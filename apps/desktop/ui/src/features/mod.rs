use crate::extension_ui::{FeatureDescriptor, FeatureRegistry};

pub(crate) mod command_search;
pub(crate) mod conversation;
pub(crate) mod editor;
pub(crate) mod files;
pub(crate) mod git;
pub(crate) mod preview;
pub(crate) mod problems;
pub(crate) mod search;
pub(crate) mod settings;
pub(crate) mod tasks;
pub(crate) mod terminal;

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
    pub const COMMAND_TOGGLE_BOTTOM_ZONE: &str = "lapis.command.toggle-bottom-zone";
    pub const COMMAND_TOGGLE_LEFT_ZONE: &str = "lapis.command.toggle-left-zone";
    pub const COMMAND_TOGGLE_RIGHT_ZONE: &str = "lapis.command.toggle-right-zone";
    pub const COMMAND_TOGGLE_ASSISTANT: &str = "lapis.command.toggle-assistant";
    pub const COMMAND_TOGGLE_PANEL_CONTROLS: &str = "lapis.command.toggle-panel-controls";
    pub const COMMAND_WORKBENCH_IDE: &str = "lapis.command.workbench-ide";
    pub const COMMAND_WORKBENCH_AGENT: &str = "lapis.command.workbench-agent";
    pub const COMMAND_BIND_VIEW_SHARED: &str = "lapis.command.bind-view-shared";
    pub const COMMAND_BIND_VIEW_TASK: &str = "lapis.command.bind-view-task";
    pub const COMMAND_START_CODEX: &str = "lapis.command.start-codex";
    pub const COMMAND_START_TERMINAL: &str = "lapis.command.start-terminal";
    #[cfg(debug_assertions)]
    pub const COMMAND_TOGGLE_DEVTOOL: &str = "lapis.command.dev.toggle-devtool";

    pub const VIEW_FILES: &str = "lapis.view.files";
    pub const VIEW_SEARCH: &str = "lapis.view.search";
    pub const VIEW_GIT: &str = "lapis.view.git";
    pub const VIEW_CHANGES: &str = "lapis.view.changes";
    pub const VIEW_PROJECT_TASKS: &str = "lapis.view.project-tasks";
    pub const VIEW_PREVIEW: &str = "lapis.view.preview";
    pub const VIEW_ASSISTANT: &str = "lapis.view.assistant";
    pub const VIEW_TERMINAL: &str = "lapis.view.terminal";
    pub const VIEW_PROBLEMS: &str = "lapis.view.problems";
    pub const VIEW_OUTPUT: &str = "lapis.view.output";
    pub const VIEW_COMMAND_SEARCH: &str = "lapis.view.command-search";
    pub const VIEW_SETTINGS: &str = "lapis.view.settings";
    pub const VIEW_NOTIFICATIONS: &str = "lapis.view.notifications";
}

pub fn bundled_registry() -> FeatureRegistry {
    let mut registry = FeatureRegistry::default();
    let descriptors = vec![
        FeatureDescriptor::core(id::FEATURE_SHELL)
            .contributes(
                crate::extension_ui::UiContribution::panel_view(
                    id::VIEW_NOTIFICATIONS,
                    crate::extension_ui::ZonePosition::Right,
                    [crate::extension_ui::ZonePosition::Right],
                    "view.notifications",
                    "problems",
                    0,
                )
                .hidden_in_picker(),
            )
            .contributes(crate::extension_ui::UiContribution::settings_view(
                id::VIEW_SETTINGS,
                "view.settings",
                "settings",
            ))
            .contributes(crate::extension_ui::UiContribution::command(
                id::COMMAND_TOGGLE_PANEL_CONTROLS,
                "command.toggle-panel-controls",
                "panel",
                40,
            ))
            .contributes(crate::extension_ui::UiContribution::command(
                id::COMMAND_WORKBENCH_IDE,
                "command.workbench-ide",
                "panel",
                41,
            ))
            .contributes(crate::extension_ui::UiContribution::command(
                id::COMMAND_WORKBENCH_AGENT,
                "command.workbench-agent",
                "panel",
                42,
            ))
            .contributes(crate::extension_ui::UiContribution::command(
                id::COMMAND_BIND_VIEW_SHARED,
                "command.bind-view-shared",
                "panel",
                43,
            ))
            .contributes(crate::extension_ui::UiContribution::command(
                id::COMMAND_BIND_VIEW_TASK,
                "command.bind-view-task",
                "panel",
                44,
            ))
            .contributes(crate::extension_ui::UiContribution::command(
                id::COMMAND_TOGGLE_BOTTOM_ZONE,
                "command.toggle-bottom-zone",
                "bottom-panel",
                50,
            ))
            .contributes(crate::extension_ui::UiContribution::command(
                id::COMMAND_TOGGLE_LEFT_ZONE,
                "command.toggle-left-zone",
                "left-panel",
                51,
            ))
            .contributes(crate::extension_ui::UiContribution::command(
                id::COMMAND_TOGGLE_RIGHT_ZONE,
                "command.toggle-right-zone",
                "right-panel",
                52,
            )),
        FeatureDescriptor::core(id::FEATURE_COMMAND_SEARCH).contributes(
            crate::extension_ui::UiContribution::view(
                id::VIEW_COMMAND_SEARCH,
                crate::extension_ui::UiSlot::SideDock,
                "view.command-search",
                "search",
                5,
            )
            .feature_owned_scroll(),
        ),
        editor::descriptor(),
        conversation::descriptor(),
        files::descriptor(),
        search::descriptor(),
        git::descriptor(),
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
                    id::COMMAND_TOGGLE_DEVTOOL.into(),
                )],
            )
            .contributes(crate::extension_ui::UiContribution::command(
                id::COMMAND_TOGGLE_DEVTOOL,
                "command.dev.toggle-devtool",
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
    use crate::extension_ui::{
        ActivationEvent, CommandId, PanelScrollPolicy, ScrollAxis, UiSlot, ViewId, ZonePosition,
    };

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
        registry.set_active_view(UiSlot::ToolDock, Some(ViewId::new(id::VIEW_CHANGES)));
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
    fn built_in_views_declare_scroll_ownership() {
        let registry = bundled_registry();
        let policy = |view_id: &str| {
            [
                ZonePosition::Main,
                ZonePosition::Left,
                ZonePosition::Bottom,
                ZonePosition::Right,
            ]
            .into_iter()
            .flat_map(|position| registry.zone_contributions(position))
            .find(|contribution| {
                contribution
                    .view
                    .as_ref()
                    .is_some_and(|view| view.as_str() == view_id)
            })
            .map(|contribution| contribution.scroll_policy)
            .expect("built-in view is registered")
        };

        for view in [
            id::VIEW_COMMAND_SEARCH,
            id::VIEW_ASSISTANT,
            id::VIEW_TERMINAL,
        ] {
            assert_eq!(policy(view), PanelScrollPolicy::FeatureOwned);
        }

        for view in [
            id::VIEW_FILES,
            id::VIEW_SEARCH,
            id::VIEW_GIT,
            id::VIEW_CHANGES,
            id::VIEW_PROJECT_TASKS,
            id::VIEW_PREVIEW,
            id::VIEW_PROBLEMS,
            id::VIEW_OUTPUT,
            id::VIEW_SETTINGS,
        ] {
            assert_eq!(
                policy(view),
                PanelScrollPolicy::Panel(ScrollAxis::Both),
                "{view}"
            );
        }
    }
    #[test]
    fn settings_view_is_available_only_in_main_panel() {
        let registry = bundled_registry();
        let contribution = registry
            .zone_contributions(crate::extension_ui::ZonePosition::Main)
            .into_iter()
            .find(|contribution| {
                contribution
                    .view
                    .as_ref()
                    .is_some_and(|view| view.as_str() == id::VIEW_SETTINGS)
            })
            .expect("settings view is registered");

        assert_eq!(
            contribution.allowed_zones,
            [crate::extension_ui::ZonePosition::Main]
        );
        assert!(
            registry
                .zone_contributions(crate::extension_ui::ZonePosition::Left)
                .into_iter()
                .all(|candidate| candidate
                    .view
                    .as_ref()
                    .is_none_or(|view| view.as_str() != id::VIEW_SETTINGS))
        );
    }

    #[test]
    fn workbench_commands_are_registered_through_the_core_registry() {
        let registry = bundled_registry();
        let commands = registry
            .contributions(UiSlot::CommandPalette)
            .into_iter()
            .filter_map(|contribution| contribution.command.as_ref())
            .map(|command| command.as_str().to_owned())
            .collect::<Vec<_>>();

        for command in [
            id::COMMAND_WORKBENCH_IDE,
            id::COMMAND_WORKBENCH_AGENT,
            id::COMMAND_BIND_VIEW_SHARED,
            id::COMMAND_BIND_VIEW_TASK,
        ] {
            assert!(commands.iter().any(|registered| registered == command));
        }
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
                    .is_some_and(|command| { command.as_str() == id::COMMAND_TOGGLE_DEVTOOL }))
        );
    }

    #[cfg(not(debug_assertions))]
    #[test]
    fn release_build_does_not_register_inspector_command() {
        assert!(
            bundled_registry()
                .contributions(UiSlot::CommandPalette)
                .iter()
                .all(|contribution| {
                    contribution.command.as_ref().is_none_or(|command| {
                        command.as_str() != "lapis.command.dev.toggle-inspector"
                    })
                })
        );
    }
}
