//! LSP の JSON-RPC 実装詳細を UI から隠す契約。
use std::{
    fmt,
    path::{Path, PathBuf},
};

use lapis_document::Revision;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LspPosition {
    pub line: u32,
    pub utf16_column: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LspRange {
    pub start: LspPosition,
    pub end: LspPosition,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub path: PathBuf,
    pub range: LspRange,
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub revision: Revision,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionItem {
    pub label: String,
    pub detail: Option<String>,
    pub insert_text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefinitionTarget {
    pub path: PathBuf,
    pub range: LspRange,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LspError(String);
impl LspError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}
impl fmt::Display for LspError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for LspError {}

pub trait LanguageServerBackend: Send + Sync {
    fn start(&self, workspace: &Path) -> Result<(), LspError>;
    fn did_open(&self, path: &Path, text: &str, revision: Revision) -> Result<(), LspError>;
    fn did_change(&self, path: &Path, text: &str, revision: Revision) -> Result<(), LspError>;
    fn diagnostics(&self) -> Result<Vec<Diagnostic>, LspError>;
    fn completion(
        &self,
        path: &Path,
        position: LspPosition,
        revision: Revision,
    ) -> Result<Vec<CompletionItem>, LspError>;
    fn definition(
        &self,
        path: &Path,
        position: LspPosition,
        revision: Revision,
    ) -> Result<Option<DefinitionTarget>, LspError>;
    fn shutdown(&self) -> Result<(), LspError>;
}
