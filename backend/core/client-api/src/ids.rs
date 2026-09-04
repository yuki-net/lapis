use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

macro_rules! resource_id {
    ($name:ident, $description:literal) => {
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
        #[serde(transparent)]
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
}
