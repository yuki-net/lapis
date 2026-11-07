use crate::git::{GitDiffSnapshot, GitSnapshot, WorktreeSnapshot};
use crate::{WorkspaceId, WorkspaceRelativePath};
use editor_core::{ExecutionId, TaskId};
use serde::{Deserialize, Serialize};
use task::{TaskControl, TaskSnapshot, TaskStart};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// Workspace 操作の要求。
pub struct WorkspaceOperationRequest {
    pub workspace_id: WorkspaceId,
    pub operation: WorkspaceOperation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "operation",
    content = "arguments",
    rename_all = "snake_case",
    deny_unknown_fields
)]
/// Workspace に対する Git / Task 操作。
pub enum WorkspaceOperation {
    GitRepositories,
    RepositoryGitStatus {
        repository_id: String,
    },
    RepositoryGitAction {
        repository_id: String,
        request: crate::git::GitActionRequest,
    },
    GitAction {
        worktree: Option<TaskId>,
        request: crate::git::GitActionRequest,
    },
    GitStatus {
        worktree: Option<TaskId>,
    },
    GitDiff {
        path: WorkspaceRelativePath,
        worktree: Option<TaskId>,
    },
    Worktrees,
    CreateWorktree,
    ImportFile {
        worktree_id: TaskId,
        path: WorkspaceRelativePath,
    },
    DiscardWorktree {
        worktree_id: TaskId,
    },
    Tasks,
    StartTask {
        input: TaskStart,
    },
    ControlTask {
        execution_id: ExecutionId,
        control: TaskControl,
    },
}
impl WorkspaceOperation {
    pub const fn is_mutation(&self) -> bool {
        matches!(
            self,
            Self::GitAction { request, .. } | Self::RepositoryGitAction { request, .. } if request.action.is_mutation()
        ) || matches!(
            self,
            Self::CreateWorktree
                | Self::ImportFile { .. }
                | Self::DiscardWorktree { .. }
                | Self::StartTask { .. }
                | Self::ControlTask { .. }
        )
    }
    pub const fn required_capability(&self) -> &'static str {
        match self {
            Self::GitAction { request, .. } | Self::RepositoryGitAction { request, .. } => {
                if request.action.is_mutation() {
                    "workspace.git.write"
                } else {
                    "workspace.git.read"
                }
            }
            Self::GitRepositories
            | Self::RepositoryGitStatus { .. }
            | Self::GitStatus { .. }
            | Self::GitDiff { .. }
            | Self::Worktrees => "workspace.git.read",
            Self::CreateWorktree | Self::ImportFile { .. } | Self::DiscardWorktree { .. } => {
                "workspace.git.write"
            }
            Self::Tasks => "workspace.tasks.read",
            Self::StartTask { .. } => "workspace.tasks.start",
            Self::ControlTask { .. } => "workspace.tasks.control",
        }
    }
    pub const fn additional_required_capability(&self) -> Option<&'static str> {
        match self {
            Self::StartTask { input } if input.isolated => Some("workspace.git.write"),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", content = "value", rename_all = "snake_case")]
/// Workspace 操作の結果。
pub enum WorkspaceOperationResponse {
    GitRepositories(Vec<crate::git::GitRepositorySnapshot>),
    RepositoryGitStatus {
        repository_id: String,
        status: GitSnapshot,
    },
    GitAction(crate::git::GitActionResult),
    GitStatus(GitSnapshot),
    GitDiff(GitDiffSnapshot),
    Worktrees(Vec<WorktreeSnapshot>),
    Worktree(WorktreeSnapshot),
    Tasks(Vec<TaskSnapshot>),
    TaskStarted(Box<TaskSnapshot>),
    Completed,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{ChangeKind, GitFileSnapshot, WorktreeState};
    use crate::{RequestBody, RequestEnvelope, RequestId, ResponseBody, ResponseEnvelope};
    use editor_core::{ConversationId, WorkspaceId as CoreWorkspaceId};
    use serde_json::{Value, json};
    use task::{ExecutionSnapshot, ExecutionStatus, Task, TaskMode};

    fn input() -> TaskStart {
        TaskStart {
            prompt: "日本語\n確認".into(),
            mode: TaskMode::Plan,
            isolated: false,
            conversation_id: ConversationId::new("conversation-1"),
        }
    }
    fn task(status: ExecutionStatus) -> TaskSnapshot {
        TaskSnapshot {
            task: Task {
                id: TaskId::new("task-1"),
                conversation_id: ConversationId::new("conversation-1"),
                title: "確認".into(),
                prompt: "日本語".into(),
                created_at_ms: 42,
            },
            execution: ExecutionSnapshot {
                id: ExecutionId::new("execution-1"),
                task_id: TaskId::new("task-1"),
                workspace_id: CoreWorkspaceId::new("workspace-1"),
                worktree_id: Some(TaskId::new("task-1")),
                runner: "codex-app-server".into(),
                mode: TaskMode::Plan,
                status,
                started_at_ms: None,
                completed_at_ms: None,
                external_thread_id: None,
                failure: None,
            },
            events: vec![],
        }
    }
    fn worktree() -> WorktreeSnapshot {
        WorktreeSnapshot {
            workspace_id: CoreWorkspaceId::new("workspace-1"),
            worktree_id: TaskId::new("task-1"),
            base_commit: "abc".into(),
            state: WorktreeState::Active,
            conflict: None,
        }
    }
    fn assert_no_internal_paths(value: &Value) {
        match value {
            Value::Object(fields) => {
                for (key, value) in fields {
                    assert!(!matches!(
                        key.as_str(),
                        "root" | "workspace_root" | "repository_root"
                    ));
                    assert_no_internal_paths(value);
                }
            }
            Value::Array(items) => {
                for item in items {
                    assert_no_internal_paths(item);
                }
            }
            _ => {}
        }
    }
    #[test]
    fn every_operation_round_trips_through_the_common_request_envelope() {
        let path = WorkspaceRelativePath::parse("note.txt").unwrap();
        let mut cases = vec![
            WorkspaceOperation::GitRepositories,
            WorkspaceOperation::RepositoryGitStatus {
                repository_id: "path:frontend".into(),
            },
            WorkspaceOperation::RepositoryGitAction {
                repository_id: "path:frontend".into(),
                request: crate::git::GitActionRequest {
                    action: crate::git::GitAction::History,
                    expected_head: None,
                },
            },
            WorkspaceOperation::GitStatus { worktree: None },
            WorkspaceOperation::GitStatus {
                worktree: Some(TaskId::new("task-1")),
            },
            WorkspaceOperation::GitDiff {
                path: path.clone(),
                worktree: None,
            },
            WorkspaceOperation::Worktrees,
            WorkspaceOperation::CreateWorktree,
            WorkspaceOperation::ImportFile {
                worktree_id: TaskId::new("task-1"),
                path,
            },
            WorkspaceOperation::DiscardWorktree {
                worktree_id: TaskId::new("task-1"),
            },
            WorkspaceOperation::Tasks,
            WorkspaceOperation::StartTask { input: input() },
        ];
        for control in [
            TaskControl::Cancel,
            TaskControl::Approve,
            TaskControl::Decline,
            TaskControl::Reply {
                text: "回答".into(),
            },
        ] {
            cases.push(WorkspaceOperation::ControlTask {
                execution_id: ExecutionId::new("execution-1"),
                control,
            });
        }
        for operation in cases {
            let request = RequestEnvelope {
                request_id: RequestId::try_new("request-1").unwrap(),
                body: RequestBody::WorkspaceExecute(WorkspaceOperationRequest {
                    workspace_id: WorkspaceId::try_new("workspace-1").unwrap(),
                    operation,
                }),
            };
            let encoded = serde_json::to_value(&request).unwrap();
            assert_eq!(encoded["type"], "workspace.execute");
            assert_no_internal_paths(&encoded);
            assert_eq!(
                serde_json::from_value::<RequestEnvelope>(encoded).unwrap(),
                request
            );
        }
    }
    #[test]
    fn every_result_round_trips_without_internal_roots() {
        let cases = [
            WorkspaceOperationResponse::GitStatus(GitSnapshot {
                workspace_id: CoreWorkspaceId::new("workspace-1"),
                worktree_id: None,
                revision: 1,
                branch: "main".into(),
                head: "abc".into(),
                files: vec![GitFileSnapshot {
                    path: "note.txt".into(),
                    old_path: None,
                    kind: ChangeKind::Modified,
                    staged: false,
                }],
            }),
            WorkspaceOperationResponse::GitDiff(GitDiffSnapshot {
                path: "note.txt".into(),
                additions: 1,
                deletions: 0,
                patch: "+新規\n".into(),
                binary: false,
            }),
            WorkspaceOperationResponse::Worktrees(vec![worktree()]),
            WorkspaceOperationResponse::Worktree(worktree()),
            WorkspaceOperationResponse::Tasks(vec![task(ExecutionStatus::Queued)]),
            WorkspaceOperationResponse::TaskStarted(Box::new(task(ExecutionStatus::Queued))),
            WorkspaceOperationResponse::Completed,
        ];
        for result in cases {
            let response = ResponseEnvelope {
                request_id: RequestId::try_new("request-1").unwrap(),
                body: ResponseBody::WorkspaceExecute(result),
            };
            let encoded = serde_json::to_value(&response).unwrap();
            assert_eq!(encoded["type"], "workspace.execute");
            assert_no_internal_paths(&encoded);
            assert_eq!(
                serde_json::from_value::<ResponseEnvelope>(encoded).unwrap(),
                response
            );
        }
    }
    #[test]
    fn task_start_rejects_client_records_ids_and_roots() {
        for extra in [
            "record",
            "workspace_root",
            "task_id",
            "execution_id",
            "status",
        ] {
            let mut encoded = serde_json::to_value(input()).unwrap();
            encoded[extra] = json!("client-value");
            assert!(serde_json::from_value::<TaskStart>(encoded).is_err());
        }
        assert!(
            serde_json::from_value::<WorkspaceOperation>(
                json!({"operation":"start_task", "arguments":{"record":{}}})
            )
            .is_err()
        );
    }
    #[test]
    fn canonical_statuses_and_isolated_capabilities_are_preserved() {
        for (status, tag) in [
            (ExecutionStatus::Queued, "queued"),
            (ExecutionStatus::Running, "running"),
            (ExecutionStatus::WaitingForInput, "waiting_for_input"),
            (ExecutionStatus::WaitingForApproval, "waiting_for_approval"),
            (ExecutionStatus::Succeeded, "succeeded"),
            (ExecutionStatus::Failed, "failed"),
            (ExecutionStatus::Cancelled, "cancelled"),
        ] {
            let snapshot = task(status);
            let encoded = serde_json::to_value(&snapshot).unwrap();
            assert_eq!(encoded["execution"]["status"], tag);
            assert_eq!(
                serde_json::from_value::<TaskSnapshot>(encoded).unwrap(),
                snapshot
            );
        }
        let mut start = input();
        assert_eq!(
            WorkspaceOperation::StartTask {
                input: start.clone()
            }
            .additional_required_capability(),
            None
        );
        start.isolated = true;
        let operation = WorkspaceOperation::StartTask { input: start };
        assert_eq!(operation.required_capability(), "workspace.tasks.start");
        assert_eq!(
            operation.additional_required_capability(),
            Some("workspace.git.write")
        );
    }
}
