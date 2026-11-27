use crate::extension_ui::FeatureRegistry;
use extension_ui_contract::FeatureDescriptor;

pub(crate) mod command_search;
pub(crate) mod conversation;
pub(crate) mod editor;
pub(crate) mod files;
pub(crate) mod git;
pub(crate) mod preview;
pub(crate) mod resource_manager;
pub(crate) mod search;
pub(crate) mod settings;
pub(crate) mod settings_coordinator;
pub(crate) mod settings_sources;
pub(crate) mod tasks;
#[path = "../../../../extensions/terminal/ui/mod.rs"]
pub(crate) mod terminal;
pub(crate) mod update;

pub mod id {
    pub use workbench_shell::ids::*;
}

pub fn bundled_registry() -> FeatureRegistry {
    let mut registry = FeatureRegistry::default();
    let descriptors = vec![
        FeatureDescriptor::core(id::FEATURE_SHELL)
            .contributes(
                extension_ui_contract::UiContribution::panel_view(
                    id::VIEW_NOTIFICATIONS,
                    extension_ui_contract::ZonePosition::Right,
                    [extension_ui_contract::ZonePosition::Right],
                    "view.notifications",
                    "problems",
                    0,
                )
                .hidden_in_picker(),
            )
            .contributes(
                extension_ui_contract::UiContribution::settings_view(
                    id::VIEW_SETTINGS,
                    "view.settings",
                    "settings",
                )
                .feature_owned_scroll(),
            )
            .contributes(extension_ui_contract::UiContribution::command(
                id::COMMAND_TOGGLE_PANEL_CONTROLS,
                "command.toggle-panel-controls",
                "panel",
                40,
            ))
            .contributes(extension_ui_contract::UiContribution::command(
                id::COMMAND_WORKBENCH_IDE,
                "command.workbench-ide",
                "panel",
                41,
            ))
            .contributes(extension_ui_contract::UiContribution::command(
                id::COMMAND_WORKBENCH_AGENT,
                "command.workbench-agent",
                "panel",
                42,
            ))
            .contributes(extension_ui_contract::UiContribution::command(
                id::COMMAND_BIND_VIEW_SHARED,
                "command.bind-view-shared",
                "panel",
                43,
            ))
            .contributes(extension_ui_contract::UiContribution::command(
                id::COMMAND_BIND_VIEW_TASK,
                "command.bind-view-task",
                "panel",
                44,
            ))
            .contributes(extension_ui_contract::UiContribution::command(
                id::COMMAND_ACCEPT_REMOTE_WORKBENCH,
                "command.accept-remote-workbench",
                "panel",
                45,
            ))
            .contributes(extension_ui_contract::UiContribution::command(
                id::COMMAND_RETRY_LOCAL_WORKBENCH,
                "command.retry-local-workbench",
                "panel",
                46,
            ))
            .contributes(extension_ui_contract::UiContribution::command(
                id::COMMAND_TOGGLE_BOTTOM_ZONE,
                "command.toggle-bottom-zone",
                "bottom-panel",
                50,
            ))
            .contributes(extension_ui_contract::UiContribution::command(
                id::COMMAND_TOGGLE_LEFT_ZONE,
                "command.toggle-left-zone",
                "left-panel",
                51,
            ))
            .contributes(extension_ui_contract::UiContribution::command(
                id::COMMAND_TOGGLE_RIGHT_ZONE,
                "command.toggle-right-zone",
                "right-panel",
                52,
            )),
        FeatureDescriptor::core(id::FEATURE_COMMAND_SEARCH).contributes(
            extension_ui_contract::UiContribution::view(
                id::VIEW_COMMAND_SEARCH,
                extension_ui_contract::UiSlot::SideDock,
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
        resource_manager::descriptor(),
        editor::rust_descriptor(),
        preview::markdown_descriptor(),
    ];
    #[cfg(debug_assertions)]
    let descriptors = {
        let mut descriptors = descriptors;
        descriptors.push(
            FeatureDescriptor::bundled(
                id::FEATURE_DEVTOOLS,
                [extension_ui_contract::ActivationCondition::OnCommand(
                    id::COMMAND_TOGGLE_DEVTOOL.into(),
                )],
            )
            .contributes(extension_ui_contract::UiContribution::command(
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
    use crate::extension_ui::ActivationEvent;
    use extension_ui_contract::{
        CommandId, PanelScrollPolicy, ScrollAxis, UiSlot, ViewId, ZonePosition,
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
    fn terminal_is_the_only_multiple_instance_built_in_view() {
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
            .map(|contribution| contribution.instance_policy)
            .expect("built-in view is registered")
        };
        // Terminal だけをタブ単位の個体にする。他の Tool は単一実体を共有する。
        assert_eq!(
            policy(id::VIEW_TERMINAL),
            extension_ui_contract::ToolInstancePolicy::Multiple
        );
        for view in [
            id::VIEW_FILES,
            id::VIEW_GIT,
            id::VIEW_CHANGES,
            id::VIEW_ASSISTANT,
            id::VIEW_RESOURCES,
            id::VIEW_SEARCH,
            id::VIEW_PROJECT_TASKS,
            id::VIEW_PREVIEW,
            id::VIEW_SETTINGS,
        ] {
            assert_eq!(
                policy(view),
                extension_ui_contract::ToolInstancePolicy::Shared,
                "{view}"
            );
        }
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
            id::VIEW_SETTINGS,
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
            id::VIEW_RESOURCES,
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
            .zone_contributions(extension_ui_contract::ZonePosition::Main)
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
            [extension_ui_contract::ZonePosition::Main]
        );
        assert!(
            registry
                .zone_contributions(extension_ui_contract::ZonePosition::Left)
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
            id::COMMAND_ACCEPT_REMOTE_WORKBENCH,
            id::COMMAND_RETRY_LOCAL_WORKBENCH,
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
                        command.as_str() != "lapis.command.dev.toggle-devtool"
                    })
                })
        );
    }
}
