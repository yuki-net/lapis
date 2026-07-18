use crate::{extension_ui::FeatureDescriptor, features::id};

mod state;

pub(crate) use state::ConversationFeature;

pub(super) fn descriptor() -> FeatureDescriptor {
    FeatureDescriptor::core(id::FEATURE_CONVERSATION)
}
