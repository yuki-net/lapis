//! PTY の実装を UI から隠すための Terminal 契約。
use std::{fmt, path::PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalId(String);

impl TerminalId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalStatus {
    Running,
    Exited,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalEvent {
    Output(String),
    Exited { code: Option<i32> },
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSnapshot {
    pub id: TerminalId,
    pub cwd: PathBuf,
    pub status: TerminalStatus,
    pub columns: u16,
    pub rows: u16,
    pub output: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalError(String);

impl TerminalError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for TerminalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for TerminalError {}

pub trait TerminalBackend: Send + Sync {
    fn start(
        &self,
        cwd: &std::path::Path,
        columns: u16,
        rows: u16,
    ) -> Result<TerminalId, TerminalError>;
    fn input(&self, id: &TerminalId, bytes: &[u8]) -> Result<(), TerminalError>;
    fn resize(&self, id: &TerminalId, columns: u16, rows: u16) -> Result<(), TerminalError>;
    fn poll(&self, id: &TerminalId) -> Result<Vec<TerminalEvent>, TerminalError>;
    fn terminate(&self, id: &TerminalId) -> Result<(), TerminalError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_snapshot_retains_lifecycle_and_size() {
        let snapshot = TerminalSnapshot {
            id: TerminalId::new("terminal-1"),
            cwd: PathBuf::from("workspace"),
            status: TerminalStatus::Running,
            columns: 120,
            rows: 36,
            output: String::new(),
        };
        assert_eq!(snapshot.id.as_str(), "terminal-1");
        assert_eq!(snapshot.columns, 120);
    }
}
