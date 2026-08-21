use std::{error::Error, fmt};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceRelativePath(String);

impl WorkspaceRelativePath {
    pub fn parse(value: impl Into<String>) -> Result<Self, InvalidWorkspacePath> {
        let value = value.into();
        if value.is_empty()
            || value.starts_with('/')
            || value.starts_with('\\')
            || value.contains('\\')
            || value.contains('\0')
            || value.contains(':')
            || value
                .split('/')
                .any(|component| component.is_empty() || matches!(component, "." | ".."))
        {
            return Err(InvalidWorkspacePath);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Serialize for WorkspaceRelativePath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for WorkspaceRelativePath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidWorkspacePath;

impl fmt::Display for InvalidWorkspacePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("workspace path must be a normalized relative path")
    }
}

impl Error for InvalidWorkspacePath {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_normalized_unicode_relative_path() {
        let path = WorkspaceRelativePath::parse("src/画面/main.ts").unwrap();
        assert_eq!(path.as_str(), "src/画面/main.ts");
    }

    #[test]
    fn rejects_escape_and_platform_absolute_forms() {
        for invalid in [
            "",
            "/etc/passwd",
            "../secret",
            "src/../secret",
            "src//main.rs",
            r"C:\workspace\file.rs",
            r"\\server\share\file.rs",
        ] {
            assert!(
                WorkspaceRelativePath::parse(invalid).is_err(),
                "accepted invalid path: {invalid}"
            );
        }
    }

    #[test]
    fn deserialization_enforces_the_invariant() {
        assert!(serde_json::from_str::<WorkspaceRelativePath>(r#""src/main.rs""#).is_ok());
        assert!(serde_json::from_str::<WorkspaceRelativePath>(r#""../secret""#).is_err());
        assert!(serde_json::from_str::<WorkspaceRelativePath>(r#""C:\\secret""#).is_err());
    }
}
