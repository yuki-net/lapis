//! 言語定義と、パスから言語を判定する軽量なレジストリ。

use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LanguageId(String);

impl LanguageId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for LanguageId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LanguageDefinition {
    pub id: LanguageId,
    pub extensions: Vec<String>,
}

impl LanguageDefinition {
    pub fn new(
        id: impl Into<LanguageId>,
        extensions: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            id: id.into(),
            extensions: extensions.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct LanguageRegistry {
    definitions: Vec<LanguageDefinition>,
}

impl LanguageRegistry {
    pub fn bundled() -> Self {
        Self {
            definitions: vec![
                LanguageDefinition::new("rust", ["rs"]),
                LanguageDefinition::new("markdown", ["md", "markdown"]),
            ],
        }
    }

    pub fn register(&mut self, definition: LanguageDefinition) -> Result<(), LanguageId> {
        if self
            .definitions
            .iter()
            .any(|current| current.id == definition.id)
        {
            return Err(definition.id);
        }
        self.definitions.push(definition);
        Ok(())
    }

    pub fn detect_path(&self, path: &Path) -> Option<LanguageId> {
        let extension = path.extension()?.to_str()?;
        self.definitions
            .iter()
            .find(|definition| {
                definition
                    .extensions
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(extension))
            })
            .map(|definition| definition.id.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_registry_detects_rust_and_markdown() {
        let registry = LanguageRegistry::bundled();
        assert_eq!(
            registry.detect_path(Path::new("src/lib.rs")),
            Some(LanguageId::new("rust"))
        );
        assert_eq!(
            registry.detect_path(Path::new("README.MD")),
            Some(LanguageId::new("markdown"))
        );
        assert_eq!(registry.detect_path(Path::new("LICENSE")), None);
    }
}
