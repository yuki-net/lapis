use crate::{
    extension_ui::{FeatureDescriptor, UiContribution, UiSlot},
    features::id,
};

mod state;
mod view_state;

pub(crate) use state::{ConversationEvent, ConversationFeature};
pub(crate) use view_state::{apply_view_state, capture_view_state};

pub(super) fn descriptor() -> FeatureDescriptor {
    FeatureDescriptor::core(id::FEATURE_CONVERSATION).contributes(UiContribution::view(
        id::VIEW_PROJECT_TASKS,
        UiSlot::ToolDock,
        "view.projects",
        "history",
        5,
    ))
}
