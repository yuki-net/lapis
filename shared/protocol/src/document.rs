use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

use crate::{DocumentId, Revision, WorkspaceId, WorkspaceRelativePath};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentEncoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentSnapshot {
    pub document_id: DocumentId,
    pub workspace_id: WorkspaceId,
    pub path: WorkspaceRelativePath,
    pub content: String,
    pub encoding: DocumentEncoding,
    pub revision: Revision,
    pub dirty: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DocumentTextEdit {
    start_char: u64,
    end_char: u64,
    replacement: String,
}

impl DocumentTextEdit {
    pub fn try_new(
        start_char: u64,
        end_char: u64,
        replacement: impl Into<String>,
    ) -> Result<Self, InvalidDocumentTextEdit> {
        if start_char > end_char {
            return Err(InvalidDocumentTextEdit);
        }
        Ok(Self {
            start_char,
            end_char,
            replacement: replacement.into(),
        })
    }

    pub const fn start_char(&self) -> u64 {
        self.start_char
    }

    pub const fn end_char(&self) -> u64 {
        self.end_char
    }

    pub fn replacement(&self) -> &str {
        &self.replacement
    }
}

#[derive(Deserialize)]
struct UncheckedDocumentTextEdit {
    start_char: u64,
    end_char: u64,
    replacement: String,
}

impl<'de> Deserialize<'de> for DocumentTextEdit {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = UncheckedDocumentTextEdit::deserialize(deserializer)?;
        Self::try_new(value.start_char, value.end_char, value.replacement)
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidDocumentTextEdit;

impl fmt::Display for InvalidDocumentTextEdit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("document edit start must not exceed end")
    }
}

impl Error for InvalidDocumentTextEdit {}

const MAX_EDITS_PER_TRANSACTION: usize = 128;

/// `base_revision`のUnicode scalar indexを基準に、start昇順で表す編集群。
/// rangeは重複不可、隣接可。backendは末尾から適用してindex shiftを避ける。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct DocumentTransaction(Vec<DocumentTextEdit>);

impl DocumentTransaction {
    pub fn try_new(edits: Vec<DocumentTextEdit>) -> Result<Self, InvalidDocumentTransaction> {
        if edits.is_empty() || edits.len() > MAX_EDITS_PER_TRANSACTION {
            return Err(InvalidDocumentTransaction);
        }
        if edits
            .iter()
            .any(|edit| edit.start_char == edit.end_char && edit.replacement.is_empty())
        {
            return Err(InvalidDocumentTransaction);
        }
        if edits.windows(2).any(|pair| {
            pair[0].start_char >= pair[1].start_char || pair[0].end_char > pair[1].start_char
        }) {
            return Err(InvalidDocumentTransaction);
        }
        Ok(Self(edits))
    }

    pub fn edits(&self) -> &[DocumentTextEdit] {
        &self.0
    }
}

impl<'de> Deserialize<'de> for DocumentTransaction {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::try_new(Vec::<DocumentTextEdit>::deserialize(deserializer)?)
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidDocumentTransaction;

impl fmt::Display for InvalidDocumentTransaction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .write_str("document transaction must contain 1 to 128 ordered, non-overlapping edits")
    }
}

impl Error for InvalidDocumentTransaction {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentOpenRequest {
    pub workspace_id: WorkspaceId,
    pub path: WorkspaceRelativePath,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentCreateRequest {
    pub workspace_id: WorkspaceId,
    pub path: WorkspaceRelativePath,
    pub encoding: DocumentEncoding,
    #[serde(default)]
    pub content: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentCreateResponse {
    pub document: DocumentSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentOpenResponse {
    pub document: DocumentSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentEditRequest {
    pub document_id: DocumentId,
    pub base_revision: Revision,
    pub transaction: DocumentTransaction,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentEditResponse {
    pub document_id: DocumentId,
    pub revision: Revision,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentSaveRequest {
    pub document_id: DocumentId,
    pub base_revision: Revision,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentSaveResponse {
    pub document_id: DocumentId,
    pub revision: Revision,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentHistoryRequest {
    pub document_id: DocumentId,
    pub base_revision: Revision,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentHistoryResponse {
    pub document: DocumentSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentCloseRequest {
    pub document_id: DocumentId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentCloseResponse {
    pub document_id: DocumentId,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_edit_rejects_reversed_range_during_deserialization() {
        assert!(DocumentTextEdit::try_new(2, 1, "x").is_err());
        assert!(
            serde_json::from_str::<DocumentTextEdit>(
                r#"{"start_char":2,"end_char":1,"replacement":"x"}"#
            )
            .is_err()
        );
    }

    #[test]
    fn transaction_requires_ordered_non_overlapping_non_empty_edits() {
        let valid = DocumentTransaction::try_new(vec![
            DocumentTextEdit::try_new(0, 1, "a").unwrap(),
            DocumentTextEdit::try_new(1, 2, "b").unwrap(),
        ])
        .unwrap();
        assert_eq!(valid.edits().len(), 2);

        assert!(DocumentTransaction::try_new(Vec::new()).is_err());
        assert!(
            DocumentTransaction::try_new(vec![DocumentTextEdit::try_new(1, 1, "").unwrap()])
                .is_err()
        );
        assert!(
            DocumentTransaction::try_new(vec![
                DocumentTextEdit::try_new(0, 2, "a").unwrap(),
                DocumentTextEdit::try_new(1, 3, "b").unwrap(),
            ])
            .is_err()
        );
    }
}
