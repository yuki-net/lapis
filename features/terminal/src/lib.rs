//! PTY の実装を UI から隠すための Terminal 契約。
use std::{fmt, path::PathBuf};

use serde::{Deserialize, Serialize};

pub type TerminalOutputSequence = u64;

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

/// PTYから届いた一つのordered output chunk。
///
/// `data` は表示用文字列ではなく、PTYから読み取ったbytesをそのまま保持する。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalOutput {
    pub sequence: TerminalOutputSequence,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalEvent {
    Output(TerminalOutput),
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
    /// 再同期用に保持している、先頭から順序どおりに連結したraw output。
    pub output: Vec<u8>,
    /// `output` に反映済みの最後のoutput sequence。outputがなければ0。
    pub output_sequence: TerminalOutputSequence,
    /// 保持上限により過去のoutputが欠落している場合にtrue。
    pub output_truncated: bool,
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
            output: Vec::new(),
            output_sequence: 0,
            output_truncated: false,
        };
        assert_eq!(snapshot.id.as_str(), "terminal-1");
        assert_eq!(snapshot.columns, 120);
    }

    #[test]
    fn terminal_output_preserves_control_and_non_utf8_bytes() {
        let output = TerminalOutput {
            sequence: 7,
            data: vec![0x1b, b'[', b'3', b'1', b'm', 0xff, 0x00],
        };

        assert_eq!(output.sequence, 7);
        assert_eq!(output.data, [0x1b, b'[', b'3', b'1', b'm', 0xff, 0x00]);
    }

    #[test]
    fn terminal_output_and_snapshot_are_lossless_over_json_payloads() {
        let output = TerminalOutput {
            sequence: 9,
            data: vec![0x1b, b'[', b'2', b'J', 0xe3, 0x81, 0x82, 0xff],
        };
        let encoded = serde_json::to_vec(&output).unwrap();
        let decoded: TerminalOutput = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, output);

        let snapshot = TerminalSnapshot {
            id: TerminalId::new("terminal-1"),
            cwd: PathBuf::from("workspace"),
            status: TerminalStatus::Running,
            columns: 120,
            rows: 36,
            output: output.data.clone(),
            output_sequence: output.sequence,
            output_truncated: false,
        };
        let encoded = serde_json::to_vec(&snapshot).unwrap();
        let decoded: TerminalSnapshot = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, snapshot);
    }
}
