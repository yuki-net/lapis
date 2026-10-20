use crate::BackendClient;
use crate::client::Client;
use lapis_client_api::{
    RequestBody, ResponseBody, WorkspaceOperation as Operation, WorkspaceOperationRequest,
    WorkspaceOperationResult as Outcome, WorkspaceRelativePath,
};
use lapis_editor_core::{ExecutionId, TaskId, WorkspaceId};
use lapis_git::{
    GitConnection, GitDiffSnapshot, GitError, GitErrorKind, GitSnapshot, WorktreeSnapshot,
};
use lapis_task::{TaskConnection, TaskControl, TaskError, TaskSnapshot, TaskStart};

fn client(host: &BackendClient, workspace: &WorkspaceId) -> Result<Client, String> {
    let workspace = lapis_client_api::WorkspaceId::try_new(workspace.as_str())
        .map_err(|error| error.to_string())?;
    host.connect_workspace(workspace)
        .map_err(|error| error.to_string())
}
fn execute(
    host: &BackendClient,
    workspace: &WorkspaceId,
    operation: Operation,
) -> Result<Outcome, String> {
    let client = client(host, workspace)?;
    match client
        .request(RequestBody::WorkspaceExecute(WorkspaceOperationRequest {
            workspace_id: client.workspace_id().clone(),
            operation,
        }))
        .map_err(|error| error.to_string())?
    {
        ResponseBody::WorkspaceExecute(result) => Ok(result),
        ResponseBody::Error(error) => Err(error
            .detail
            .unwrap_or_else(|| error.code.as_str().to_owned())),
        _ => Err("Unexpected workspace response".to_owned()),
    }
}
fn git_error(error: impl std::fmt::Display) -> GitError {
    GitError::new(GitErrorKind::Io, error.to_string())
}
fn relative(path: &str) -> Result<WorkspaceRelativePath, GitError> {
    WorkspaceRelativePath::parse(path).map_err(git_error)
}

pub(super) struct HostGit {
    host: BackendClient,
}
impl HostGit {
    pub fn new(host: BackendClient) -> Self {
        Self { host }
    }
}
impl GitConnection for HostGit {
    fn status(
        &self,
        workspace: &WorkspaceId,
        worktree: Option<&TaskId>,
    ) -> Result<GitSnapshot, GitError> {
        match execute(
            &self.host,
            workspace,
            Operation::GitStatus {
                worktree: worktree.cloned(),
            },
        )
        .map_err(git_error)?
        {
            Outcome::GitStatus(value) => Ok(value),
            _ => Err(git_error("Unexpected Git status")),
        }
    }
    fn diff(
        &self,
        workspace: &WorkspaceId,
        worktree: Option<&TaskId>,
        path: &str,
    ) -> Result<GitDiffSnapshot, GitError> {
        match execute(
            &self.host,
            workspace,
            Operation::GitDiff {
                path: relative(path)?,
                worktree: worktree.cloned(),
            },
        )
        .map_err(git_error)?
        {
            Outcome::GitDiff(value) => Ok(value),
            _ => Err(git_error("Unexpected Git diff")),
        }
    }
    fn worktrees(&self, workspace: &WorkspaceId) -> Result<Vec<WorktreeSnapshot>, GitError> {
        match execute(&self.host, workspace, Operation::Worktrees).map_err(git_error)? {
            Outcome::Worktrees(value) => Ok(value),
            _ => Err(git_error("Unexpected worktrees")),
        }
    }
    fn create_worktree(&self, workspace: &WorkspaceId) -> Result<WorktreeSnapshot, GitError> {
        match execute(&self.host, workspace, Operation::CreateWorktree).map_err(git_error)? {
            Outcome::Worktree(value) => Ok(value),
            _ => Err(git_error("Unexpected worktree")),
        }
    }
    fn import_file(
        &self,
        workspace: &WorkspaceId,
        worktree: &TaskId,
        path: &str,
    ) -> Result<WorktreeSnapshot, GitError> {
        match execute(
            &self.host,
            workspace,
            Operation::ImportFile {
                worktree_id: worktree.clone(),
                path: relative(path)?,
            },
        )
        .map_err(git_error)?
        {
            Outcome::Worktree(value) => Ok(value),
            _ => Err(git_error("Unexpected worktree import")),
        }
    }
    fn discard_worktree(
        &self,
        workspace: &WorkspaceId,
        worktree: &TaskId,
    ) -> Result<WorktreeSnapshot, GitError> {
        match execute(
            &self.host,
            workspace,
            Operation::DiscardWorktree {
                worktree_id: worktree.clone(),
            },
        )
        .map_err(git_error)?
        {
            Outcome::Worktree(value) => Ok(value),
            _ => Err(git_error("Unexpected worktree discard")),
        }
    }
}

pub(super) struct HostTasks {
    host: BackendClient,
}
impl HostTasks {
    pub fn new(host: BackendClient) -> Self {
        Self { host }
    }
}
impl TaskConnection for HostTasks {
    fn load(&self, workspace: &WorkspaceId) -> Result<Vec<TaskSnapshot>, TaskError> {
        match execute(&self.host, workspace, Operation::Tasks).map_err(TaskError::new)? {
            Outcome::Tasks(value) => Ok(value),
            _ => Err(TaskError::new("Unexpected tasks")),
        }
    }
    fn start(&self, workspace: &WorkspaceId, input: TaskStart) -> Result<TaskSnapshot, TaskError> {
        match execute(&self.host, workspace, Operation::StartTask { input })
            .map_err(TaskError::new)?
        {
            Outcome::TaskStarted(value) => Ok(*value),
            _ => Err(TaskError::new("Unexpected task start")),
        }
    }
    fn control(
        &self,
        workspace: &WorkspaceId,
        execution: &ExecutionId,
        control: &TaskControl,
    ) -> Result<(), TaskError> {
        match execute(
            &self.host,
            workspace,
            Operation::ControlTask {
                execution_id: execution.clone(),
                control: control.clone(),
            },
        )
        .map_err(TaskError::new)?
        {
            Outcome::Completed => Ok(()),
            _ => Err(TaskError::new("Unexpected task control")),
        }
    }
}
