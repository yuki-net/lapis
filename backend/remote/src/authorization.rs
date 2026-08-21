use std::{error::Error, fmt};

use lapis_client_api::{CapabilityId, CapabilitySet, RequestBody, SessionId, WorkspaceId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionGrant {
    session_id: SessionId,
    workspace_id: WorkspaceId,
    capabilities: CapabilitySet,
}

impl SessionGrant {
    pub fn new(
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

    pub fn require_request(&self, request: &RequestBody) -> Result<(), AuthorizationError> {
        let capability = CapabilityId::try_new(request.required_capability())
            .expect("built-in request capability must be valid");
        self.require_capability(&capability)
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
    fn rejects_other_workspace_and_missing_capability() {
        let files = CapabilityId::try_new(capability::FILES_READ).unwrap();
        let terminal = CapabilityId::try_new(capability::TERMINAL_START).unwrap();
        let grant = SessionGrant::new(
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
            size: lapis_client_api::TerminalSize {
                columns: 80,
                rows: 24,
            },
        });
        assert_eq!(grant.require_request(&file_tree), Ok(()));
        assert!(matches!(
            grant.require_request(&terminal_start),
            Err(AuthorizationError::CapabilityDenied(_))
        ));
    }
}
