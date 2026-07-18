use crate::{
    extension_ui::{ActivationCondition, FeatureDescriptor, UiContribution, UiSlot},
    features::id,
};

pub(super) fn descriptor() -> FeatureDescriptor {
    FeatureDescriptor::bundled(id::FEATURE_FILES, [ActivationCondition::OnWorkspaceOpen])
        .contributes(UiContribution::view(
            id::VIEW_FILES,
            UiSlot::ToolDock,
            "view.files",
            "files",
            10,
        ))
        .requires("workspace.files.read")
}
