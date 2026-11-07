use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

macro_rules! resource_id {
    ($name:ident, $description:literal) => {
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
        #[serde(transparent)]
        /// 生成された resource ID。空・128 byte 超は拒否する。
        pub struct $name(String);

        impl $name {
            pub fn try_new(value: impl Into<String>) -> Result<Self, InvalidResourceId> {
                let value = value.into();
                if value.trim().is_empty() || value.len() > MAX_RESOURCE_ID_BYTES {
                    return Err(InvalidResourceId);
                }
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                Self::try_new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
            }
        }

        impl TryFrom<String> for $name {
            type Error = InvalidResourceId;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::try_new(value)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        const _: &str = $description;
    };
}

const MAX_RESOURCE_ID_BYTES: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// resource ID が空、または長すぎる。
pub struct InvalidResourceId;

impl fmt::Display for InvalidResourceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("resource ID must contain 1 to 128 bytes")
    }
}

impl Error for InvalidResourceId {}

resource_id!(RequestId, "request");
resource_id!(ClientId, "client");
resource_id!(SessionId, "session");
resource_id!(WorkspaceId, "workspace");
resource_id!(DocumentId, "document");
resource_id!(TerminalId, "terminal");

impl TryFrom<&editor_core::WorkspaceId> for WorkspaceId {
    type Error = InvalidResourceId;

    fn try_from(value: &editor_core::WorkspaceId) -> Result<Self, Self::Error> {
        Self::try_new(value.as_str())
    }
}

impl From<WorkspaceId> for editor_core::WorkspaceId {
    fn from(value: WorkspaceId) -> Self {
        Self::new(value.into_inner())
    }
}

impl TryFrom<&editor_core::DocumentId> for DocumentId {
    type Error = InvalidResourceId;

    fn try_from(value: &editor_core::DocumentId) -> Result<Self, Self::Error> {
        Self::try_new(value.as_str())
    }
}

impl From<DocumentId> for editor_core::DocumentId {
    fn from(value: DocumentId) -> Self {
        Self::new(value.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_remain_distinct_types_and_round_trip_as_strings() {
        let id = DocumentId::try_new("document-1").unwrap();
        let json = serde_json::to_string(&id).unwrap();

        assert_eq!(json, r#""document-1""#);
        assert_eq!(serde_json::from_str::<DocumentId>(&json).unwrap(), id);
        assert!(serde_json::from_str::<DocumentId>(r#"" ""#).is_err());
        assert!(DocumentId::try_new("x".repeat(MAX_RESOURCE_ID_BYTES + 1)).is_err());
    }

    #[test]
    fn domain_ids_convert_through_try_from_and_from() {
        let domain = editor_core::WorkspaceId::new("workspace-1");
        let wire = WorkspaceId::try_from(&domain).unwrap();
        assert_eq!(wire.as_str(), "workspace-1");
        assert_eq!(editor_core::WorkspaceId::from(wire), domain);

        let domain = editor_core::DocumentId::new("document-1");
        let wire = DocumentId::try_from(&domain).unwrap();
        assert_eq!(wire.as_str(), "document-1");
        assert_eq!(editor_core::DocumentId::from(wire), domain);
    }
}
