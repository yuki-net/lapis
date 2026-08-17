//! 言語定義と、パスから言語を判定する軽量なレジストリ。

mod languages;
mod registry;

pub use registry::LanguageRegistry;

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
