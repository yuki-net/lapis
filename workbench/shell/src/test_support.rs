//! Test-only registry with the built-in View declarations the shell tests use.
//!
//! Production clients build the full registry in `desktop-ui`. Keeping a small
//! registry here avoids a dev-dependency cycle between the shell and its consumer.

use crate::extension_ui::FeatureRegistry;
use crate::ids;
use extension_ui_contract::{FeatureDescriptor, ToolInstancePolicy, UiContribution, UiSlot};

pub fn registry() -> FeatureRegistry {
    let simple = [
        (ids::VIEW_FILES, "view.files"),
        (ids::VIEW_SEARCH, "view.search"),
        (ids::VIEW_GIT, "view.git"),
        (ids::VIEW_CHANGES, "view.changes"),
        (ids::VIEW_PROJECT_TASKS, "view.projects"),
        (ids::VIEW_PREVIEW, "view.preview"),
        (ids::VIEW_ASSISTANT, "view.assistant"),
        (ids::VIEW_PROBLEMS, "view.problems"),
        (ids::VIEW_RESOURCES, "view.resources"),
        (ids::VIEW_OUTPUT, "view.output"),
        (ids::VIEW_COMMAND_SEARCH, "view.command-search"),
        (ids::VIEW_SETTINGS, "view.settings"),
    ];
    let mut descriptor = FeatureDescriptor::core("test.shell");
    for (view, title) in simple {
        descriptor = descriptor.contributes(UiContribution::view(
            view,
            UiSlot::ToolDock,
            title,
            "files",
            0,
        ));
    }
    descriptor = descriptor.contributes(
        UiContribution::panel_view(
            ids::VIEW_NOTIFICATIONS,
            extension_ui_contract::ZonePosition::Right,
            [extension_ui_contract::ZonePosition::Right],
            "view.notifications",
            "problems",
            0,
        )
        .hidden_in_picker(),
    );
    descriptor = descriptor.contributes(
        UiContribution::view(
            ids::VIEW_TERMINAL,
            UiSlot::BottomDock,
            "view.terminal",
            "terminal",
            0,
        )
        .references_external_resource()
        .multiple_instances(),
    );
    let mut registry = FeatureRegistry::default();
    registry
        .register(descriptor)
        .expect("test View IDs are unique");
    debug_assert_eq!(
        registry
            .declaration(&ids::VIEW_TERMINAL.into())
            .map(|declaration| declaration.instance_policy),
        Some(ToolInstancePolicy::Multiple)
    );
    registry
}
