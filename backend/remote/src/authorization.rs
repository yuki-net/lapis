use std::{error::Error, fmt};

use lapis_client_api::{CapabilityId, CapabilitySet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionGrant {
    session_id: String,
    workspace_id: String,
    capabilities: CapabilitySet,
}

impl SessionGrant {
    pub fn new(
        session_id: impl Into<String>,
        workspace_id: impl Into<String>,
        capabilities: CapabilitySet,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            workspace_id: workspace_id.into(),
            capabilities,
        }
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn require_workspace(&self, workspace_id: &str) -> Result<(), AuthorizationError> {
        if self.workspace_id == workspace_id {
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
        let files = CapabilityId::new(capability::FILES_READ);
        let terminal = CapabilityId::new(capability::TERMINAL_START);
        let grant = SessionGrant::new(
            "session-1",
            "workspace-1",
            CapabilitySet::new([files.clone()]),
        );

        assert_eq!(grant.require_workspace("workspace-1"), Ok(()));
        assert_eq!(
            grant.require_workspace("workspace-2"),
            Err(AuthorizationError::WorkspaceDenied)
        );
        assert_eq!(grant.require_capability(&files), Ok(()));
        assert!(matches!(
            grant.require_capability(&terminal),
            Err(AuthorizationError::CapabilityDenied(_))
        ));
    }
}
