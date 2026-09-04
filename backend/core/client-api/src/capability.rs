use std::{collections::BTreeSet, error::Error, fmt};

use serde::{Deserialize, Serialize};

pub const WORKSPACES: &str = "workspace.list";
pub const WORKSPACES_CONNECT: &str = "workspace.connect";
pub const WORKSPACE_SYNC: &str = "workspace.sync";
pub const FILES_READ: &str = "workspace.files.read";
pub const DOCUMENTS_READ: &str = "workspace.documents.read";
pub const DOCUMENTS_WRITE: &str = "workspace.documents.write";
pub const TERMINAL_START: &str = "workspace.terminal.start";
pub const TERMINAL_CONTROL: &str = "workspace.terminal.control";

const MAX_CAPABILITY_ID_BYTES: usize = 128;
const MAX_CAPABILITIES: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct CapabilityId(String);

impl CapabilityId {
    pub fn try_new(value: impl Into<String>) -> Result<Self, InvalidCapabilityId> {
        let value = value.into();
        if value.trim().is_empty() || value.len() > MAX_CAPABILITY_ID_BYTES {
            return Err(InvalidCapabilityId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for CapabilityId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::try_new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidCapabilityId;

impl fmt::Display for InvalidCapabilityId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("capability ID must contain 1 to 128 bytes")
    }
}

impl Error for InvalidCapabilityId {}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct CapabilitySet(BTreeSet<CapabilityId>);

impl CapabilitySet {
    pub fn try_new(
        values: impl IntoIterator<Item = CapabilityId>,
    ) -> Result<Self, TooManyCapabilities> {
        let values = values.into_iter().collect::<BTreeSet<_>>();
        if values.len() > MAX_CAPABILITIES {
            return Err(TooManyCapabilities);
        }
        Ok(Self(values))
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

impl<'de> Deserialize<'de> for CapabilitySet {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::try_new(Vec::<CapabilityId>::deserialize(deserializer)?)
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooManyCapabilities;

impl fmt::Display for TooManyCapabilities {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("capability set must contain at most 64 values")
    }
}

impl Error for TooManyCapabilities {}

#[cfg(test)]
mod tests {
    use super::*;

    fn capability(value: &str) -> CapabilityId {
        CapabilityId::try_new(value).unwrap()
    }

    #[test]
    fn intersection_only_grants_capabilities_held_by_both_sides() {
        let requested =
            CapabilitySet::try_new([capability(FILES_READ), capability(TERMINAL_START)]).unwrap();
        let allowed =
            CapabilitySet::try_new([capability(FILES_READ), capability(DOCUMENTS_READ)]).unwrap();

        let granted = requested.intersection(&allowed);

        assert!(granted.contains(&capability(FILES_READ)));
        assert!(!granted.contains(&capability(TERMINAL_START)));
    }

    #[test]
    fn deserialization_rejects_invalid_and_excessive_capabilities() {
        assert!(serde_json::from_str::<CapabilityId>(r#""""#).is_err());
        let json = serde_json::to_string(
            &(0..=MAX_CAPABILITIES)
                .map(|index| format!("capability.{index}"))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        assert!(serde_json::from_str::<CapabilitySet>(&json).is_err());
    }
}
