use serde::{Deserialize, Serialize};

use crate::{DocumentId, Revision};

pub const INVALID_REQUEST: &str = "invalid_request";
pub const UNAUTHORIZED: &str = "unauthorized";
pub const FORBIDDEN: &str = "forbidden";
pub const NOT_FOUND: &str = "not_found";
pub const REVISION_CONFLICT: &str = "revision_conflict";
pub const INVALID_PATH: &str = "invalid_path";
pub const UNSUPPORTED: &str = "unsupported";
pub const PROTOCOL_ERROR: &str = "protocol_error";
pub const RATE_LIMITED: &str = "rate_limited";
pub const INTERNAL: &str = "internal";
/// credential の有効期限が切れている。
pub const CREDENTIAL_EXPIRED: &str = "credential_expired";
/// 固定された Workspace と接続先が一致しない。
pub const WORKSPACE_MISMATCH: &str = "workspace_mismatch";
/// 指定された Workspace が Host に登録されていない。
pub const WORKSPACE_UNAVAILABLE: &str = "workspace_unavailable";
/// credential が別の Host のもの。
pub const HOST_MISMATCH: &str = "host_mismatch";

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
/// 安定した機械可読の error code。未知の code も保持する。
pub struct ErrorCode(String);

impl ErrorCode {
    pub fn try_new(value: impl Into<String>) -> Result<Self, InvalidErrorCode> {
        let value = value.into();
        if value.trim().is_empty() || value.len() > 128 {
            return Err(InvalidErrorCode);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn revision_conflict() -> Self {
        Self(REVISION_CONFLICT.to_owned())
    }
}

impl<'de> Deserialize<'de> for ErrorCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::try_new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// error code が空、または長すぎる。
pub struct InvalidErrorCode;

impl std::fmt::Display for InvalidErrorCode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("error code must contain 1 to 128 bytes")
    }
}

impl std::error::Error for InvalidErrorCode {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// 編集要求の base revision が現在値と一致しない。
pub struct RevisionConflict {
    pub document_id: DocumentId,
    pub expected: Revision,
    pub actual: Revision,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// Request 失敗の共通表現。
pub struct ProtocolError {
    pub code: ErrorCode,
    /// Transportログへそのまま出してよいとは限らない診断情報。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default)]
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision_conflict: Option<RevisionConflict>,
}

impl ProtocolError {
    pub fn new(code: ErrorCode) -> Self {
        Self {
            code,
            detail: None,
            retryable: false,
            revision_conflict: None,
        }
    }

    pub fn revision_conflict(conflict: RevisionConflict) -> Self {
        Self {
            code: ErrorCode::revision_conflict(),
            detail: None,
            retryable: false,
            revision_conflict: Some(conflict),
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_code_is_stable_and_unknown_codes_round_trip() {
        let error = ProtocolError::new(ErrorCode::try_new("future_error").unwrap());
        let json = serde_json::to_string(&error).unwrap();

        assert!(json.contains(r#""code":"future_error""#));
        assert_eq!(serde_json::from_str::<ProtocolError>(&json).unwrap(), error);
        assert!(serde_json::from_str::<ErrorCode>(r#""""#).is_err());
    }

    #[test]
    fn revision_conflict_keeps_both_revisions() {
        let error = ProtocolError::revision_conflict(RevisionConflict {
            document_id: DocumentId::try_new("doc-1").unwrap(),
            expected: Revision::new(3),
            actual: Revision::new(4),
        });

        assert_eq!(error.code.as_str(), REVISION_CONFLICT);
        assert_eq!(error.revision_conflict.unwrap().actual, Revision::new(4));
    }
}
