use crate::{
    extension_ui::{ActivationCondition, FeatureDescriptor, UiContribution, UiSlot, ViewId},
    features::id,
};

mod state;
mod view;

pub(crate) use state::ProblemsFeature;
pub(crate) use view::{render_content, render_output};

pub(super) fn descriptor() -> FeatureDescriptor {
    FeatureDescriptor::bundled(
        id::FEATURE_PROBLEMS,
        [ActivationCondition::OnView(ViewId::new(id::VIEW_PROBLEMS))],
    )
    .contributes(UiContribution::view(
        id::VIEW_PROBLEMS,
        UiSlot::BottomDock,
        "view.problems",
        "problems",
        20,
    ))
}

pub(super) fn output_descriptor() -> FeatureDescriptor {
    FeatureDescriptor::bundled(
        id::FEATURE_OUTPUT,
        [ActivationCondition::OnView(ViewId::new(id::VIEW_OUTPUT))],
    )
    .contributes(UiContribution::view(
        id::VIEW_OUTPUT,
        UiSlot::BottomDock,
        "view.output",
        "output",
        30,
    ))
}
