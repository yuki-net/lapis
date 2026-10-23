//! DesktopとMobileが共有する、transport非依存のclient契約。

mod connection;
mod document;
mod envelope;
mod error;
mod event;
mod files;
mod handshake;
mod ids;
mod operations;
mod path;
mod request;
mod response;
mod revision;
mod snapshot;
mod terminal;
mod version;
mod workspace;

pub mod capability;

pub use capability::{CapabilityId, CapabilitySet, InvalidCapabilityId, TooManyCapabilities};
pub use connection::ConnectionState;
pub use document::{
    DocumentCloseRequest, DocumentCloseResponse, DocumentCreateRequest, DocumentCreateResponse,
    DocumentDiscardRequest, DocumentDiscardResponse, DocumentEditRequest, DocumentEditResponse,
    DocumentEncoding, DocumentExternalChange, DocumentHistoryRequest, DocumentHistoryResponse,
    DocumentInspectRequest, DocumentOpenRequest, DocumentOpenResponse, DocumentRelocateRequest,
    DocumentSaveAsRequest, DocumentSaveRequest, DocumentSaveResponse, DocumentSnapshot,
    DocumentTextEdit, DocumentTransaction, DocumentUntitledRequest, InvalidDocumentTextEdit,
    InvalidDocumentTransaction,
};
pub use envelope::{EventEnvelope, RequestEnvelope, ResponseEnvelope};
pub use error::{
    ErrorCode, FORBIDDEN, INTERNAL, INVALID_PATH, INVALID_REQUEST, InvalidErrorCode, NOT_FOUND,
    PROTOCOL_ERROR, ProtocolError, RATE_LIMITED, REVISION_CONFLICT, RevisionConflict, UNAUTHORIZED,
    UNSUPPORTED,
};
pub use event::EventBody;
pub use files::{
    CreateDirectoryRequest, CreateDirectoryResponse, FileTreeEntry, FileTreeKind, FileTreeRequest,
    FileTreeResponse,
};
pub use handshake::{ClientHello, ClientKind, ServerHello};
pub use ids::{
    ClientId, DocumentId, InvalidResourceId, RequestId, SessionId, TerminalId, WorkspaceId,
};
pub use operations::{WorkspaceOperation, WorkspaceOperationRequest, WorkspaceOperationResult};
pub use path::{InvalidWorkspacePath, WorkspaceRelativePath};
pub use request::RequestBody;
pub use response::ResponseBody;
pub use revision::{EventSequence, Revision};
pub use snapshot::{SnapshotReason, SnapshotRequest, SnapshotResponse, WorkspaceSnapshot};
pub use terminal::{
    TerminalCell, TerminalColor, TerminalCommandResponse, TerminalCursor, TerminalDelta,
    TerminalInput, TerminalInputRequest, TerminalKey, TerminalKeyEvent, TerminalModes,
    TerminalModifiers, TerminalResizeRequest, TerminalRevision, TerminalRow, TerminalRowUpdate,
    TerminalScreen, TerminalShell, TerminalSize, TerminalSnapshot, TerminalStartRequest,
    TerminalStartResponse, TerminalStatus, TerminalTerminateRequest,
};
pub use version::{
    CURRENT_PROTOCOL, InvalidProtocolRange, ProtocolRange, ProtocolVersion, VersionMismatch,
};
pub use workspace::{
    HostRegisterRequest, WorkspaceCloseRequest, WorkspaceCloseResponse, WorkspaceConnectRequest,
    WorkspaceConnectResponse, WorkspaceListRequest, WorkspaceListResponse, WorkspaceSummary,
};
