use crate::{
    extension_ui::{ActivationCondition, FeatureDescriptor, UiContribution, UiSlot, ViewId},
    features::id,
};

mod state;

pub(crate) use state::TasksFeature;

pub(super) fn descriptor() -> FeatureDescriptor {
    FeatureDescriptor::bundled(
        id::FEATURE_ASSISTANT,
        [
            ActivationCondition::OnView(ViewId::new(id::VIEW_ASSISTANT)),
            ActivationCondition::OnCommand(id::COMMAND_START_CODEX.into()),
        ],
    )
    .contributes(UiContribution::view(
        id::VIEW_ASSISTANT,
        UiSlot::SideDock,
        "view.assistant",
        "assistant",
        20,
    ))
    .requires("workspace.process")
}
