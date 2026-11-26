//! ACP（Agent Client Protocol）client の拡張境界。
//!
//! 外部ハーネスの起動・protocol の送受信・`TaskEvent` への変換だけを持ち、
//! Task の正規状態や Workspace の判断には触れない。対応する ACP の範囲は
//! 安定面の最小セット（`initialize` / `session/new` / `session/prompt` /
//! `session/update` / `session/request_permission` / `session/cancel`）に限る。

mod harness;
mod map;
mod session;

pub use harness::opencode_spec;
pub use map::UpdateMapper;
pub use session::{AcpAgentSpec, AcpError, AcpUpdate, run_acp_session};

pub use task::ExecutionStatus;
