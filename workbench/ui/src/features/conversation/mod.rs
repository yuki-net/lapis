use crate::features::id;
use extension_ui_contract::{FeatureDescriptor, UiContribution, UiSlot};

mod state;

pub(crate) use state::{ConversationEvent, ConversationFeature};

pub(super) fn descriptor() -> FeatureDescriptor {
    FeatureDescriptor::core(id::FEATURE_CONVERSATION).contributes(UiContribution::view(
        id::VIEW_PROJECT_TASKS,
        UiSlot::ToolDock,
        "view.projects",
        "history",
        5,
    ))
}
