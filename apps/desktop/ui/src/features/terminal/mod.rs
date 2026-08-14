use crate::{
    extension_ui::{ActivationCondition, FeatureDescriptor, UiContribution, UiSlot, ViewId},
    features::id,
};

mod state;
mod view;

pub(crate) use state::TerminalFeature;
pub(crate) use view::render_content;

pub(super) fn descriptor() -> FeatureDescriptor {
    FeatureDescriptor::bundled(
        id::FEATURE_TERMINAL,
        [
            ActivationCondition::OnView(ViewId::new(id::VIEW_TERMINAL)),
            ActivationCondition::OnCommand(id::COMMAND_START_TERMINAL.into()),
        ],
    )
    .contributes(UiContribution::view(
        id::VIEW_TERMINAL,
        UiSlot::BottomDock,
        "view.terminal",
        "terminal",
        10,
    ))
    .requires("workspace.process")
}
