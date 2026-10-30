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

struct Connections {
    host: BackendClient,
    clients: std::sync::Mutex<std::collections::HashMap<lapis_client_api::WorkspaceId, Client>>,
}
impl Connections {
    fn new(host: BackendClient) -> Self {
        Self {
            host,
            clients: Default::default(),
        }
    }
    fn connect(&self, workspace: lapis_client_api::WorkspaceId) -> Result<Client, String> {
        let mut clients = self
            .clients
            .lock()
            .map_err(|_| "Workspace接続を取得できません")?;
        if let Some(client) = clients.get(&workspace) {
            return Ok(client.clone());
        }
        let client = self
            .host
            .connect_workspace(workspace.clone())
            .map_err(|error| error.to_string())?;
        clients.insert(workspace, client.clone());
        Ok(client)
    }
}

fn client(host: &Connections, workspace: &WorkspaceId) -> Result<Client, String> {
    let workspace = lapis_client_api::WorkspaceId::try_new(workspace.as_str())
        .map_err(|error| error.to_string())?;
    host.connect(workspace)
}
fn execute(
    host: &Connections,
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
    host: Connections,
}
impl HostGit {
    pub fn new(host: BackendClient) -> Self {
        Self {
            host: Connections::new(host),
        }
    }
}
impl GitConnection for HostGit {
    fn repositories(
        &self,
        workspace: &WorkspaceId,
    ) -> Result<Vec<lapis_git::GitRepositorySnapshot>, GitError> {
        match execute(&self.host, workspace, Operation::GitRepositories).map_err(git_error)? {
            Outcome::GitRepositories(value) => Ok(value),
            _ => Err(git_error("Unexpected Git repositories")),
        }
    }
    fn repository_status(
        &self,
        workspace: &WorkspaceId,
        repository_id: &str,
    ) -> Result<GitSnapshot, GitError> {
        match execute(
            &self.host,
            workspace,
            Operation::RepositoryGitStatus {
                repository_id: repository_id.to_owned(),
            },
        )
        .map_err(git_error)?
        {
            Outcome::RepositoryGitStatus {
                repository_id: actual,
                status,
            } if actual == repository_id => Ok(status),
            _ => Err(git_error("Unexpected Git repository status")),
        }
    }
    fn repository_action(
        &self,
        workspace: &WorkspaceId,
        repository_id: &str,
        request: lapis_git::GitActionRequest,
    ) -> Result<lapis_git::GitActionResult, GitError> {
        match execute(
            &self.host,
            workspace,
            Operation::RepositoryGitAction {
                repository_id: repository_id.to_owned(),
                request,
            },
        )
        .map_err(git_error)?
        {
            Outcome::GitAction(result) => Ok(result),
            _ => Err(git_error("Unexpected Git repository operation")),
        }
    }

    fn open_worktree(
        &self,
        workspace: &WorkspaceId,
        worktree: &TaskId,
    ) -> Result<std::path::PathBuf, GitError> {
        let id = lapis_client_api::WorkspaceId::try_new(workspace.as_str()).map_err(git_error)?;
        self.host
            .host
            .open_worktree(id, worktree.clone())
            .map_err(git_error)
    }
    fn execute(
        &self,
        workspace: &WorkspaceId,
        worktree: Option<&TaskId>,
        request: lapis_git::GitActionRequest,
    ) -> Result<lapis_git::GitActionResult, GitError> {
        match execute(
            &self.host,
            workspace,
            Operation::GitAction {
                worktree: worktree.cloned(),
                request,
            },
        )
        .map_err(git_error)?
        {
            Outcome::GitAction(result) => Ok(result),
            _ => Err(git_error("Unexpected Git result")),
        }
    }
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
    host: Connections,
}
impl HostTasks {
    pub fn new(host: BackendClient) -> Self {
        Self {
            host: Connections::new(host),
        }
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
