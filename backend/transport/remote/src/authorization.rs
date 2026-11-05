use std::{error::Error, fmt};

use lapis_client_api::{CapabilityId, CapabilitySet, RequestBody, SessionId, WorkspaceId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionGrant {
    session_id: SessionId,
    workspace_id: WorkspaceId,
    capabilities: CapabilitySet,
}

impl SessionGrant {
    pub(crate) fn from_authenticated(
        session_id: SessionId,
        workspace_id: WorkspaceId,
        capabilities: CapabilitySet,
    ) -> Self {
        Self {
            session_id,
            workspace_id,
            capabilities,
        }
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn workspace_id(&self) -> &WorkspaceId {
        &self.workspace_id
    }

    pub fn capabilities(&self) -> &CapabilitySet {
        &self.capabilities
    }

    pub fn require_workspace(&self, workspace_id: &WorkspaceId) -> Result<(), AuthorizationError> {
        if &self.workspace_id == workspace_id {
            Ok(())
        } else {
            Err(AuthorizationError::WorkspaceDenied)
        }
    }

    pub fn require_capability(&self, capability: &CapabilityId) -> Result<(), AuthorizationError> {
        if self.capabilities.contains(capability) {
            Ok(())
        } else {
            Err(AuthorizationError::CapabilityDenied(
                capability.as_str().to_owned(),
            ))
        }
    }

    pub(crate) fn require_request(&self, request: &RequestBody) -> Result<(), AuthorizationError> {
        let capability = CapabilityId::try_new(request.required_capability())
            .expect("built-in request capability must be valid");
        let owner_registration = matches!(request, RequestBody::HostRegister(_))
            && self
                .capabilities
                .iter()
                .any(|value| value.as_str() == lapis_client_api::capability::HOST_MANAGE);
        if !owner_registration {
            self.require_capability(&capability)?;
        }
        if matches!(request, RequestBody::WorkbenchPut(_)) {
            let read = CapabilityId::try_new(lapis_client_api::capability::WORKSPACE_SYNC)
                .expect("built-in capability must be valid");
            self.require_capability(&read)?;
        }
        if let RequestBody::WorkspaceExecute(request) = request
            && let Some(additional) = request.operation.additional_required_capability()
        {
            let capability = CapabilityId::try_new(additional)
                .expect("built-in request capability must be valid");
            self.require_capability(&capability)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthorizationError {
    WorkspaceDenied,
    CapabilityDenied(String),
}

impl fmt::Display for AuthorizationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WorkspaceDenied => {
                formatter.write_str("workspace is not granted to this session")
            }
            Self::CapabilityDenied(capability) => {
                write!(formatter, "capability is not granted: {capability}")
            }
        }
    }
}

impl Error for AuthorizationError {}

#[cfg(test)]
mod tests {
    use super::*;
    use lapis_client_api::capability;

    #[test]
    fn workspace_registration_is_separate_from_other_host_management() {
        let workspace = WorkspaceId::try_new("workspace-1").unwrap();
        let make_grant = |capabilities: &[&str]| {
            SessionGrant::from_authenticated(
                SessionId::try_new("session-1").unwrap(),
                workspace.clone(),
                CapabilitySet::try_new(
                    capabilities
                        .iter()
                        .map(|value| CapabilityId::try_new(*value).unwrap()),
                )
                .unwrap(),
            )
        };
        let register = RequestBody::HostRegister(lapis_client_api::HostRegisterRequest {
            root: "absolute".into(),
        });
        let worktree = RequestBody::HostWorktree(lapis_editor_core::TaskId::new("task-1"));
        let user = make_grant(&[capability::WORKSPACES_REGISTER]);
        assert!(user.require_request(&register).is_ok());
        assert!(user.require_request(&worktree).is_err());
        let owner = make_grant(&[capability::HOST_MANAGE]);
        assert!(owner.require_request(&register).is_ok());
        assert!(owner.require_request(&worktree).is_ok());
        assert!(make_grant(&[]).require_request(&register).is_err());
    }

    #[test]
    fn workbench_write_also_requires_read_access() {
        let workspace = WorkspaceId::try_new("workspace-1").unwrap();
        let grant = SessionGrant::from_authenticated(
            SessionId::try_new("session-1").unwrap(),
            workspace.clone(),
            CapabilitySet::try_new([CapabilityId::try_new(capability::WORKBENCH_WRITE).unwrap()])
                .unwrap(),
        );
        let request = RequestBody::WorkbenchPut(lapis_client_api::WorkbenchPutRequest {
            workspace_id: workspace,
            expected_revision: 0,
            view: serde_json::json!({}),
        });
        assert_eq!(
            grant.require_request(&request),
            Err(AuthorizationError::CapabilityDenied(
                capability::WORKSPACE_SYNC.into()
            ))
        );
    }

    #[test]
    fn isolated_task_start_requires_both_task_start_and_git_write() {
        let workspace = WorkspaceId::try_new("workspace-1").unwrap();
        let request = |isolated| {
            RequestBody::WorkspaceExecute(lapis_client_api::WorkspaceOperationRequest {
                workspace_id: workspace.clone(),
                operation: lapis_client_api::WorkspaceOperation::StartTask {
                    input: lapis_task::TaskStart {
                        prompt: "テスト".into(),
                        mode: lapis_task::TaskMode::Default,
                        isolated,
                        conversation_id: lapis_editor_core::ConversationId::new("conversation-1"),
                    },
                },
            })
        };
        for (capabilities, allowed, missing) in [
            (vec![], false, "workspace.tasks.start"),
            (vec!["workspace.tasks.start"], false, "workspace.git.write"),
            (vec!["workspace.git.write"], false, "workspace.tasks.start"),
            (
                vec!["workspace.tasks.start", "workspace.git.read"],
                false,
                "workspace.git.write",
            ),
            (
                vec!["workspace.tasks.start", "workspace.git.write"],
                true,
                "",
            ),
        ] {
            let grant = SessionGrant::from_authenticated(
                SessionId::try_new("session-1").unwrap(),
                workspace.clone(),
                CapabilitySet::try_new(
                    capabilities
                        .into_iter()
                        .map(|value| CapabilityId::try_new(value).unwrap()),
                )
                .unwrap(),
            );
            let result = grant.require_request(&request(true));
            if allowed {
                assert_eq!(result, Ok(()));
            } else {
                assert_eq!(
                    result,
                    Err(AuthorizationError::CapabilityDenied(missing.into()))
                );
            }
        }
        let start_only = SessionGrant::from_authenticated(
            SessionId::try_new("session-1").unwrap(),
            workspace.clone(),
            CapabilitySet::try_new([CapabilityId::try_new("workspace.tasks.start").unwrap()])
                .unwrap(),
        );
        assert_eq!(start_only.require_request(&request(false)), Ok(()));
    }

    #[test]
    fn rejects_other_workspace_and_missing_capability() {
        let files = CapabilityId::try_new(capability::FILES_READ).unwrap();
        let terminal = CapabilityId::try_new(capability::TERMINAL_START).unwrap();
        let grant = SessionGrant::from_authenticated(
            SessionId::try_new("session-1").unwrap(),
            WorkspaceId::try_new("workspace-1").unwrap(),
            CapabilitySet::try_new([files.clone()]).unwrap(),
        );

        assert_eq!(
            grant.require_workspace(&WorkspaceId::try_new("workspace-1").unwrap()),
            Ok(())
        );
        assert_eq!(
            grant.require_workspace(&WorkspaceId::try_new("workspace-2").unwrap()),
            Err(AuthorizationError::WorkspaceDenied)
        );
        assert_eq!(grant.require_capability(&files), Ok(()));
        assert!(matches!(
            grant.require_capability(&terminal),
            Err(AuthorizationError::CapabilityDenied(_))
        ));

        let file_tree = RequestBody::FileTree(lapis_client_api::FileTreeRequest {
            workspace_id: WorkspaceId::try_new("workspace-1").unwrap(),
            path: None,
        });
        let terminal_start = RequestBody::TerminalStart(lapis_client_api::TerminalStartRequest {
            workspace_id: WorkspaceId::try_new("workspace-1").unwrap(),
            cwd: None,
            command: None,
            shell: Default::default(),
            size: lapis_client_api::TerminalSize {
                columns: 80,
                rows: 24,
            },
        });
        assert_eq!(grant.require_request(&file_tree), Ok(()));
        let create_directory =
            RequestBody::CreateDirectory(lapis_client_api::CreateDirectoryRequest {
                workspace_id: WorkspaceId::try_new("workspace-1").unwrap(),
                path: lapis_client_api::WorkspaceRelativePath::parse("new-folder").unwrap(),
            });
        assert!(matches!(
            grant.require_request(&create_directory),
            Err(AuthorizationError::CapabilityDenied(_))
        ));
        let write_grant = SessionGrant::from_authenticated(
            SessionId::try_new("writer").unwrap(),
            WorkspaceId::try_new("workspace-1").unwrap(),
            CapabilitySet::try_new([CapabilityId::try_new(
                lapis_client_api::capability::FILES_WRITE,
            )
            .unwrap()])
            .unwrap(),
        );
        assert_eq!(write_grant.require_request(&create_directory), Ok(()));
        assert!(matches!(
            grant.require_request(&terminal_start),
            Err(AuthorizationError::CapabilityDenied(_))
        ));
    }
}
