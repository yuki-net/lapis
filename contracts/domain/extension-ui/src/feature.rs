use crate::{ActivationCondition, FeatureId, UiContribution, WorkspaceCapabilityId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeatureKind {
    Core,
    Bundled,
}

/// 1つの Feature が何を宣言するか。状態（installed / loaded / running）は持たない。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeatureDescriptor {
    pub id: FeatureId,
    pub kind: FeatureKind,
    pub activation: Vec<ActivationCondition>,
    pub contributions: Vec<UiContribution>,
    pub required_capabilities: Vec<WorkspaceCapabilityId>,
}

impl FeatureDescriptor {
    pub fn core(id: impl Into<FeatureId>) -> Self {
        Self {
            id: id.into(),
            kind: FeatureKind::Core,
            activation: Vec::new(),
            contributions: Vec::new(),
            required_capabilities: Vec::new(),
        }
    }

    pub fn bundled(
        id: impl Into<FeatureId>,
        activation: impl IntoIterator<Item = ActivationCondition>,
    ) -> Self {
        Self {
            id: id.into(),
            kind: FeatureKind::Bundled,
            activation: activation.into_iter().collect(),
            contributions: Vec::new(),
            required_capabilities: Vec::new(),
        }
    }

    pub fn contributes(mut self, contribution: UiContribution) -> Self {
        self.contributions.push(contribution);
        self
    }

    pub fn requires(mut self, capability: impl Into<WorkspaceCapabilityId>) -> Self {
        self.required_capabilities.push(capability.into());
        self
    }
}
