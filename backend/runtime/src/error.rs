use std::{error::Error, fmt};

use lapis_client_api::RevisionConflict;
use lapis_document::DocumentError;
use lapis_terminal::TerminalError;

use crate::PathSecurityError;

#[derive(Debug)]
pub enum BackendStateError {
    WorkspaceNotFound,
    DuplicateWorkspace,
    WorkspaceDenied,
    WorkspaceNotConnected,
    DocumentNotFound,
    DocumentNotAttached,
    TerminalNotFound,
    TerminalNotAttached,
    TerminalNotRunning,
    InvalidTerminalSize,
    RevisionConflict(RevisionConflict),
    Path(PathSecurityError),
    Document(DocumentError),
    Terminal(TerminalError),
    CounterOverflow,
    Unsupported,
}

impl fmt::Display for BackendStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WorkspaceNotFound => formatter.write_str("workspace was not found"),
            Self::DuplicateWorkspace => formatter.write_str("workspace is already registered"),
            Self::WorkspaceDenied => formatter.write_str("workspace is not granted to session"),
            Self::WorkspaceNotConnected => formatter.write_str("workspace is not connected"),
            Self::DocumentNotFound => formatter.write_str("document was not found"),
            Self::DocumentNotAttached => formatter.write_str("document is not attached to session"),
            Self::TerminalNotFound => formatter.write_str("terminal was not found"),
            Self::TerminalNotAttached => formatter.write_str("terminal is not attached to session"),
            Self::TerminalNotRunning => formatter.write_str("terminal is not running"),
            Self::InvalidTerminalSize => {
                formatter.write_str("terminal size must be greater than zero")
            }
            Self::RevisionConflict(_) => formatter.write_str("document revision conflicts"),
            Self::Path(error) => error.fmt(formatter),
            Self::Document(error) => error.fmt(formatter),
            Self::Terminal(error) => error.fmt(formatter),
            Self::CounterOverflow => formatter.write_str("backend resource counter overflowed"),
            Self::Unsupported => formatter.write_str("request is not implemented by this service"),
        }
    }
}

impl Error for BackendStateError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Path(error) => Some(error),
            Self::Document(error) => Some(error),
            Self::Terminal(error) => Some(error),
            _ => None,
        }
    }
}

impl From<PathSecurityError> for BackendStateError {
    fn from(error: PathSecurityError) -> Self {
        Self::Path(error)
    }
}

impl From<DocumentError> for BackendStateError {
    fn from(error: DocumentError) -> Self {
        Self::Document(error)
    }
}

impl From<TerminalError> for BackendStateError {
    fn from(error: TerminalError) -> Self {
        Self::Terminal(error)
    }
}
