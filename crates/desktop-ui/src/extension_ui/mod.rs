mod activation;
mod contribution;
mod ids;
mod registry;

pub use activation::{ActivationCondition, ActivationEvent};
pub use contribution::{UiContribution, UiSlot};
pub use ids::{
    CommandId, FeatureId, IconId, IconThemeId, KeymapId, LocaleId, MessageId, ThemeId, ViewId,
    WorkspaceCapabilityId,
};
pub use registry::{FeatureDescriptor, FeatureKind, FeatureRegistry, FeatureState};
