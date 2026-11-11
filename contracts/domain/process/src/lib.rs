//! Lapis が spawn したプロセスの所有関係と、ローカル計測結果の共通モデル。
//!
//! この crate は OS プロセスの一覧を扱わない。登録された root PID とその子孫だけが
//! `ProcessRoot` として表現される。実際の計測・終了は Host / Desktop のローカル実装が行う。

use std::fmt;

use editor_core::WorkspaceId;
use serde::{Deserialize, Serialize};

/// この計測結果を所有する実行環境。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessOrigin {
    Host,
    Desktop,
}

/// 管理下プロセスの種別。root にだけ付く。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessKind {
    Host,
    Desktop,
    Terminal,
    Agent,
    LanguageServer,
    TaskRun,
    Build,
    Git,
    Other,
}

impl ProcessKind {
    pub const fn is_self(self) -> bool {
        matches!(self, Self::Host | Self::Desktop)
    }
}

/// 所有元の種別。LSP のように個別 ID を持たないものは `id` を省略する。
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProcessOwner {
    pub kind: ProcessKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

impl ProcessOwner {
    pub fn new(kind: ProcessKind, id: Option<String>) -> Self {
        Self { kind, id }
    }

    pub fn terminal(id: impl Into<String>) -> Self {
        Self::new(ProcessKind::Terminal, Some(id.into()))
    }

    pub fn agent(id: impl Into<String>) -> Self {
        Self::new(ProcessKind::Agent, Some(id.into()))
    }

    pub fn task_run(id: impl Into<String>) -> Self {
        Self::new(ProcessKind::TaskRun, Some(id.into()))
    }

    pub fn language_server() -> Self {
        Self::new(ProcessKind::LanguageServer, None)
    }
}

/// プロセスの現在状態。root が観測できなくなった場合は `Exited`。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessState {
    Running,
    Exited,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// root またはその子孫の現在値。
pub struct ProcessNode {
    pub pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_pid: Option<u32>,
    pub name: String,
    /// root からの深さ。root 自身は 0。
    pub depth: u16,
    /// CPU 使用率の 100 分の 1 単位。float を wire 契約へ持ち込まない。
    pub cpu_centipercent: u32,
    pub memory_bytes: u64,
    pub started_at_ms: u64,
}

impl ProcessNode {
    pub fn cpu_percent(&self) -> f32 {
        self.cpu_centipercent as f32 / 100.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// root ツリー全体の定期サンプル。
pub struct ProcessSample {
    pub at_ms: u64,
    pub cpu_centipercent: u32,
    pub memory_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// Lapis が所有する root とその子孫。
pub struct ProcessRoot {
    pub root_id: ProcessRootId,
    pub origin: ProcessOrigin,
    pub kind: ProcessKind,
    pub owner: ProcessOwner,
    /// 登録時の PID。終了後も表示できるよう保持する。
    pub pid: u32,
    pub label: String,
    pub started_at_ms: u64,
    pub state: ProcessState,
    /// CPU 使用率の 100 分の 1 単位。float を wire 契約へ持ち込まない。
    pub cpu_centipercent: u32,
    pub memory_bytes: u64,
    /// 直近のサンプル（古い順）。計測間隔と保持数は実装が決める。
    pub samples: Vec<ProcessSample>,
    /// root 自身を含む現在のツリー。`depth` で表示順を決められる。
    pub nodes: Vec<ProcessNode>,
    /// Host / Desktop 自身は終了操作を出さない。
    pub terminable: bool,
}

impl ProcessRoot {
    pub fn cpu_percent(&self) -> f32 {
        self.cpu_centipercent as f32 / 100.0
    }

    /// 子孫を含めた総メモリ使用量。表示のたびに再計算しない。
    pub fn total_memory_bytes(&self) -> u64 {
        self.memory_bytes
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
/// ある時点の管理下プロセス一覧。
pub struct ProcessSnapshot {
    pub sampled_at_ms: u64,
    pub roots: Vec<ProcessRoot>,
}

impl ProcessSnapshot {
    pub fn roots_for(&self, origin: ProcessOrigin) -> impl Iterator<Item = &ProcessRoot> {
        self.roots.iter().filter(move |root| root.origin == origin)
    }
}

/// レジストリへ登録する root の指定。子孫は実装が PID から解決する。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessRootSpec {
    pub workspace: Option<WorkspaceId>,
    pub kind: ProcessKind,
    pub owner: ProcessOwner,
    pub pid: u32,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
/// Lapis のレジストリが発行する root の識別子。PID は再利用されるため ID に使わない。
pub struct ProcessRootId(String);

impl ProcessRootId {
    pub fn try_new(value: impl Into<String>) -> Result<Self, InvalidProcessRootId> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 128
            || !value
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':'))
        {
            return Err(InvalidProcessRootId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProcessRootId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidProcessRootId;

impl fmt::Display for InvalidProcessRootId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid process root id")
    }
}

impl std::error::Error for InvalidProcessRootId {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessErrorKind {
    /// 登録されていない root。
    NotFound,
    /// 呼び出し元の Workspace に属さない、または Host / Desktop 自身。
    NotOwned,
    /// OS に root が存在しない、または終了操作に失敗した。
    Io,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessError {
    kind: ProcessErrorKind,
    message: String,
}

impl ProcessError {
    pub fn new(kind: ProcessErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn kind(&self) -> ProcessErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ProcessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ProcessError {}

/// Host / Desktop が所有するプロセスレジストリ。
///
/// 登録された root PID とその子孫だけを計測し、終了操作も root 単位でしか受け付けない。
pub trait ProcessRegistry: Send + Sync {
    fn origin(&self) -> ProcessOrigin;

    fn register(&self, spec: ProcessRootSpec) -> Result<ProcessRootId, ProcessError>;

    /// 明示的な破棄。root が既に終了していても成功する。
    fn unregister(&self, root_id: &ProcessRootId);

    /// `workspace` を指定すると、その Workspace の root と Host / Desktop 自身を返す。
    fn snapshot(&self, workspace: Option<&WorkspaceId>) -> ProcessSnapshot;

    /// 所有する root とその子孫だけを終了する。
    fn terminate(
        &self,
        workspace: Option<&WorkspaceId>,
        root_id: &ProcessRootId,
    ) -> Result<(), ProcessError>;
}

/// Client から Host のレジストリへ到達する契約。Local / Remote で同じ意味にする。
pub trait ProcessConnection: Send + Sync {
    fn snapshot(&self, workspace: &WorkspaceId) -> Result<ProcessSnapshot, ProcessError>;

    fn terminate(
        &self,
        workspace: &WorkspaceId,
        root_id: &ProcessRootId,
    ) -> Result<(), ProcessError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root_id(value: &str) -> ProcessRootId {
        ProcessRootId::try_new(value).unwrap()
    }

    #[test]
    fn root_ids_reject_values_that_cannot_be_sent_on_the_wire() {
        assert!(ProcessRootId::try_new("process-1-2").is_ok());
        assert!(ProcessRootId::try_new("").is_err());
        assert!(ProcessRootId::try_new("process/1").is_err());
        assert!(ProcessRootId::try_new("process 1").is_err());
        assert!(ProcessRootId::try_new("x".repeat(129)).is_err());
    }

    #[test]
    fn snapshot_round_trips_without_internal_paths() {
        let snapshot = ProcessSnapshot {
            sampled_at_ms: 1_000,
            roots: vec![ProcessRoot {
                root_id: root_id("process-1"),
                origin: ProcessOrigin::Host,
                kind: ProcessKind::Terminal,
                owner: ProcessOwner::terminal("terminal-1"),
                pid: 42,
                label: "Terminal".into(),
                started_at_ms: 900,
                state: ProcessState::Running,
                cpu_centipercent: 150,
                memory_bytes: 1024,
                samples: vec![ProcessSample {
                    at_ms: 1_000,
                    cpu_centipercent: 150,
                    memory_bytes: 1024,
                }],
                nodes: vec![ProcessNode {
                    pid: 42,
                    parent_pid: None,
                    name: "pwsh".into(),
                    depth: 0,
                    cpu_centipercent: 150,
                    memory_bytes: 1024,
                    started_at_ms: 900,
                }],
                terminable: true,
            }],
        };
        let encoded = serde_json::to_value(&snapshot).unwrap();
        assert!(!encoded.to_string().contains("workspace_root"));
        assert_eq!(
            serde_json::from_value::<ProcessSnapshot>(encoded).unwrap(),
            snapshot
        );
    }

    #[test]
    fn owners_keep_kind_and_optional_id() {
        assert_eq!(
            ProcessOwner::task_run("execution-1"),
            ProcessOwner::new(ProcessKind::TaskRun, Some("execution-1".into()))
        );
        assert_eq!(ProcessOwner::language_server().id, None);
        assert!(ProcessKind::Host.is_self());
        assert!(!ProcessKind::Terminal.is_self());
    }
}
