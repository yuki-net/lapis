//! Terminal 専用 wire。
//!
//! Terminal の型は `protocol/types/terminal` を正本とし、ここでは Host と Client の間で
//! 往復する要求・応答の形だけを定義する。要求は Workspace と Workspace 相対 path
//! だけを運び、Host の絶対 path を wire に載せない。応答の snapshot も Host 側で
//! Workspace 相対 path へ直してから送る。

use serde::{Deserialize, Serialize};
use terminal::{
    TerminalEvent, TerminalId, TerminalInput, TerminalProfileId, TerminalProfiles, TerminalSnapshot,
};

use crate::{WorkspaceId, WorkspaceRelativePath};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "kebab-case")]
/// 1 つの Terminal 操作。workspace は認証済み接続の Workspace と一致させる。
pub enum TerminalRequest {
    #[serde(rename = "start")]
    Start {
        workspace_id: WorkspaceId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<WorkspaceRelativePath>,
        columns: u16,
        rows: u16,
    },
    #[serde(rename = "start-profile")]
    StartProfile {
        workspace_id: WorkspaceId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<WorkspaceRelativePath>,
        columns: u16,
        rows: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile: Option<TerminalProfileId>,
    },
    #[serde(rename = "profiles")]
    Profiles {
        workspace_id: WorkspaceId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<WorkspaceRelativePath>,
    },
    #[serde(rename = "list")]
    List {
        workspace_id: WorkspaceId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<WorkspaceRelativePath>,
    },
    #[serde(rename = "attach")]
    Attach {
        workspace_id: WorkspaceId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<WorkspaceRelativePath>,
        id: TerminalId,
    },
    #[serde(rename = "input")]
    Input {
        workspace_id: WorkspaceId,
        id: TerminalId,
        input: TerminalInput,
    },
    #[serde(rename = "resize")]
    Resize {
        workspace_id: WorkspaceId,
        id: TerminalId,
        columns: u16,
        rows: u16,
    },
    #[serde(rename = "poll")]
    Poll {
        workspace_id: WorkspaceId,
        id: TerminalId,
    },
    #[serde(rename = "terminate")]
    Terminate {
        workspace_id: WorkspaceId,
        id: TerminalId,
    },
    #[serde(rename = "claim")]
    Claim {
        workspace_id: WorkspaceId,
        id: TerminalId,
    },
    #[serde(rename = "release")]
    Release {
        workspace_id: WorkspaceId,
        id: TerminalId,
    },
}

impl TerminalRequest {
    /// 認証済み接続と照合する Workspace。
    pub const fn workspace_id(&self) -> &WorkspaceId {
        match self {
            Self::Start { workspace_id, .. }
            | Self::StartProfile { workspace_id, .. }
            | Self::Profiles { workspace_id, .. }
            | Self::List { workspace_id, .. }
            | Self::Attach { workspace_id, .. }
            | Self::Input { workspace_id, .. }
            | Self::Resize { workspace_id, .. }
            | Self::Poll { workspace_id, .. }
            | Self::Terminate { workspace_id, .. }
            | Self::Claim { workspace_id, .. }
            | Self::Release { workspace_id, .. } => workspace_id,
        }
    }

    /// 要求された Workspace 相対の作業ディレクトリ。`None` は Workspace root。
    pub fn cwd(&self) -> Option<&WorkspaceRelativePath> {
        match self {
            Self::Start { cwd, .. }
            | Self::StartProfile { cwd, .. }
            | Self::Profiles { cwd, .. }
            | Self::List { cwd, .. }
            | Self::Attach { cwd, .. } => cwd.as_ref(),
            _ => None,
        }
    }

    /// 対象の Terminal。開始・一覧・profile 照会は対象を持たない。
    pub fn terminal_id(&self) -> Option<&TerminalId> {
        match self {
            Self::Attach { id, .. }
            | Self::Input { id, .. }
            | Self::Resize { id, .. }
            | Self::Poll { id, .. }
            | Self::Terminate { id, .. }
            | Self::Claim { id, .. }
            | Self::Release { id, .. } => Some(id),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
/// `TerminalRequest` 1 つに対する応答。
pub enum TerminalResponse {
    Started { id: TerminalId },
    Profiles { profiles: TerminalProfiles },
    Sessions { sessions: Vec<TerminalSnapshot> },
    Attached { snapshot: TerminalSnapshot },
    Events { events: Vec<TerminalEvent> },
    Completed,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use terminal::{TerminalDelta, TerminalScreen, TerminalSize, TerminalStatus};

    fn workspace() -> WorkspaceId {
        WorkspaceId::try_new("workspace-1").unwrap()
    }

    fn snapshot() -> TerminalSnapshot {
        TerminalSnapshot {
            id: TerminalId::new("terminal-1"),
            cwd: Path::new("src").to_owned(),
            status: TerminalStatus::Running,
            revision: 3,
            screen: TerminalScreen::empty(TerminalSize::new(80, 24)),
        }
    }

    #[test]
    fn start_profile_request_keeps_paths_workspace_relative() {
        let request = TerminalRequest::StartProfile {
            workspace_id: workspace(),
            cwd: Some(WorkspaceRelativePath::parse("src/画面").unwrap()),
            columns: 80,
            rows: 24,
            profile: Some(TerminalProfileId::new("cmd")),
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["operation"], "start-profile");
        assert_eq!(json["cwd"], "src/画面");
        assert!(
            !serde_json::to_string(&request).unwrap().contains(":\\"),
            "absolute host paths must not reach the wire"
        );
        assert_eq!(
            serde_json::from_value::<TerminalRequest>(json).unwrap(),
            request
        );
    }

    #[test]
    fn terminal_request_rejects_escaping_and_absolute_paths() {
        for cwd in [r#""../secret""#, r#""C:\\workspace""#, r#""/etc""#] {
            let value = serde_json::json!({
                "operation": "start",
                "workspace_id": "workspace-1",
                "cwd": serde_json::from_str::<serde_json::Value>(cwd).unwrap(),
                "columns": 80,
                "rows": 24,
            });
            assert!(
                serde_json::from_value::<TerminalRequest>(value).is_err(),
                "accepted cwd {cwd}"
            );
        }
    }

    #[test]
    fn terminal_responses_round_trip_contract_models() {
        let screens = TerminalResponse::Sessions {
            sessions: vec![snapshot()],
        };
        let encoded = serde_json::to_string(&screens).unwrap();
        assert!(encoded.contains(r#""cwd":"src""#));
        assert_eq!(
            serde_json::from_str::<TerminalResponse>(&encoded).unwrap(),
            screens
        );

        let events = TerminalResponse::Events {
            events: vec![TerminalEvent::Delta(TerminalDelta {
                revision: 1,
                size: TerminalSize::new(80, 24),
                changed_rows: Vec::new(),
                cursor: Default::default(),
                modes: Default::default(),
                scrollback: None,
            })],
        };
        assert_eq!(
            serde_json::from_str::<TerminalResponse>(&serde_json::to_string(&events).unwrap())
                .unwrap(),
            events
        );
    }

    #[test]
    fn every_request_carries_its_workspace() {
        let workspace_id = workspace();
        let requests = [
            TerminalRequest::Start {
                workspace_id: workspace_id.clone(),
                cwd: None,
                columns: 80,
                rows: 24,
            },
            TerminalRequest::StartProfile {
                workspace_id: workspace_id.clone(),
                cwd: None,
                columns: 80,
                rows: 24,
                profile: None,
            },
            TerminalRequest::Profiles {
                workspace_id: workspace_id.clone(),
                cwd: None,
            },
            TerminalRequest::List {
                workspace_id: workspace_id.clone(),
                cwd: None,
            },
            TerminalRequest::Attach {
                workspace_id: workspace_id.clone(),
                cwd: None,
                id: TerminalId::new("terminal-1"),
            },
            TerminalRequest::Input {
                workspace_id: workspace_id.clone(),
                id: TerminalId::new("terminal-1"),
                input: TerminalInput::Text("ls".to_owned()),
            },
            TerminalRequest::Resize {
                workspace_id: workspace_id.clone(),
                id: TerminalId::new("terminal-1"),
                columns: 100,
                rows: 30,
            },
            TerminalRequest::Poll {
                workspace_id: workspace_id.clone(),
                id: TerminalId::new("terminal-1"),
            },
            TerminalRequest::Terminate {
                workspace_id: workspace_id.clone(),
                id: TerminalId::new("terminal-1"),
            },
            TerminalRequest::Claim {
                workspace_id: workspace_id.clone(),
                id: TerminalId::new("terminal-1"),
            },
            TerminalRequest::Release {
                workspace_id: workspace_id.clone(),
                id: TerminalId::new("terminal-1"),
            },
        ];
        for request in requests {
            assert_eq!(request.workspace_id(), &workspace_id);
            assert_eq!(
                serde_json::from_str::<TerminalRequest>(&serde_json::to_string(&request).unwrap())
                    .unwrap(),
                request
            );
        }
    }

    #[test]
    fn unset_cwd_is_omitted_instead_of_duplicated() {
        let request = TerminalRequest::List {
            workspace_id: workspace(),
            cwd: None,
        };
        let encoded = serde_json::to_string(&request).unwrap();
        assert_eq!(
            encoded,
            r#"{"operation":"list","workspace_id":"workspace-1"}"#
        );
    }
}
