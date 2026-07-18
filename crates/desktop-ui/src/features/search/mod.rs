use crate::{
    extension_ui::{ActivationCondition, FeatureDescriptor, UiContribution, UiSlot},
    features::id,
};

mod state;

pub(crate) use state::SearchFeature;

pub(super) fn descriptor() -> FeatureDescriptor {
    FeatureDescriptor::bundled(
        id::FEATURE_SEARCH,
        [ActivationCondition::OnCommand(
            id::COMMAND_FIND_WORKSPACE.into(),
        )],
    )
    .contributes(UiContribution::view(
        id::VIEW_SEARCH,
        UiSlot::ToolDock,
        "view.search",
        "search",
        20,
    ))
    .requires("workspace.files.read")
}
