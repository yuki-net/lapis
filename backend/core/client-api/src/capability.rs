use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

pub const WORKSPACES: &str = "workspace.list";
pub const FILES_READ: &str = "workspace.files.read";
pub const DOCUMENTS_READ: &str = "workspace.documents.read";
pub const DOCUMENTS_WRITE: &str = "workspace.documents.write";
pub const TERMINAL_START: &str = "workspace.terminal.start";
pub const TERMINAL_CONTROL: &str = "workspace.terminal.control";

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapabilityId(String);

impl CapabilityId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapabilitySet(BTreeSet<CapabilityId>);

impl CapabilitySet {
    pub fn new(values: impl IntoIterator<Item = CapabilityId>) -> Self {
        Self(values.into_iter().collect())
    }

    pub fn contains(&self, capability: &CapabilityId) -> bool {
        self.0.contains(capability)
    }

    pub fn iter(&self) -> impl Iterator<Item = &CapabilityId> {
        self.0.iter()
    }

    pub fn intersection(&self, peer: &Self) -> Self {
        Self(self.0.intersection(&peer.0).cloned().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intersection_only_grants_capabilities_held_by_both_sides() {
        let requested = CapabilitySet::new([
            CapabilityId::new(FILES_READ),
            CapabilityId::new(TERMINAL_START),
        ]);
        let allowed = CapabilitySet::new([
            CapabilityId::new(FILES_READ),
            CapabilityId::new(DOCUMENTS_READ),
        ]);

        let granted = requested.intersection(&allowed);

        assert!(granted.contains(&CapabilityId::new(FILES_READ)));
        assert!(!granted.contains(&CapabilityId::new(TERMINAL_START)));
    }
}
