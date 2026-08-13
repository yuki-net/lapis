use crate::{
    extension_ui::{ActivationCondition, FeatureDescriptor, UiContribution, UiSlot, ViewId},
    features::id,
};

pub(super) fn descriptor() -> FeatureDescriptor {
    FeatureDescriptor::bundled(
        id::FEATURE_PREVIEW,
        [ActivationCondition::OnView(ViewId::new(id::VIEW_PREVIEW))],
    )
    .contributes(UiContribution::view(
        id::VIEW_PREVIEW,
        UiSlot::SideDock,
        "view.preview",
        "preview",
        10,
    ))
}

pub(super) fn markdown_descriptor() -> FeatureDescriptor {
    FeatureDescriptor::bundled(
        id::FEATURE_MARKDOWN,
        [ActivationCondition::OnLanguage("markdown".into())],
    )
}
