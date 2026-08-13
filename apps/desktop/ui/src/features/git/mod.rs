use crate::{
    extension_ui::{ActivationCondition, FeatureDescriptor, UiContribution, UiSlot, ViewId},
    features::id,
};

mod state;

pub(crate) use state::GitFeature;

pub(super) fn descriptor() -> FeatureDescriptor {
    FeatureDescriptor::bundled(
        id::FEATURE_GIT,
        [ActivationCondition::OnView(ViewId::new(id::VIEW_GIT))],
    )
    .contributes(UiContribution::view(
        id::VIEW_GIT,
        UiSlot::ToolDock,
        "view.git",
        "git",
        30,
    ))
    .requires("workspace.git")
}

pub(super) fn history_descriptor() -> FeatureDescriptor {
    FeatureDescriptor::bundled(
        id::FEATURE_HISTORY,
        [ActivationCondition::OnView(ViewId::new(id::VIEW_HISTORY))],
    )
    .contributes(UiContribution::view(
        id::VIEW_HISTORY,
        UiSlot::ToolDock,
        "view.history",
        "history",
        40,
    ))
}
