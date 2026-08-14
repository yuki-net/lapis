use crate::{extension_ui::FeatureDescriptor, features::id};

mod state;
mod view_state;

pub(crate) use state::ConversationFeature;
pub(crate) use view_state::{apply_view_state, capture_view_state};

pub(super) fn descriptor() -> FeatureDescriptor {
    FeatureDescriptor::core(id::FEATURE_CONVERSATION)
}
