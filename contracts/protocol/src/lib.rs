//! DesktopとMobileが共有する、transport非依存のclient契約。
//!
//! # 命名規則
//!
//! wire の境界を越える message は役割を接尾辞で示す。
//!
//! - `XxxRequest` / `XxxResponse`: 1つの method の要求と応答
//! - `XxxSnapshot`: ある時点の状態の複製
//! - `XxxEvent`: 片方向の状態変化
//!
//! 上記のどれでもない値型（`FileTreeEntry` / `RevisionConflict` / `ClientKind` など）は
//! ドメインの名詞をそのまま使う。境界を越える message に `Result` / `Hello` のような
//! 役割の曖昧な名前を付けない。

mod connection;
mod credential_id;
mod document;
mod envelope;
mod error;
mod event;
mod files;
mod handshake;
mod ids;
mod operations;
mod remote_wire;
mod request;
mod response;
mod revision;
mod snapshot;
mod terminal;
mod version;
mod workbench;
mod workspace;

pub mod capability;
pub mod git;

pub use ::workspace::{InvalidWorkspacePath, WorkspaceRelativePath};
pub use capability::{CapabilityId, CapabilitySet, InvalidCapabilityId, TooManyCapabilities};
pub use connection::ConnectionState;
pub use credential_id::{CredentialId, InvalidCredentialId};
pub use document::{
    DocumentClaimRequest, DocumentCloseRequest, DocumentCloseResponse, DocumentCommandResponse,
    DocumentCreateRequest, DocumentCreateResponse, DocumentDiscardRequest, DocumentDiscardResponse,
    DocumentEditRequest, DocumentEditResponse, DocumentEncoding, DocumentExternalChange,
    DocumentHistoryRequest, DocumentHistoryResponse, DocumentInspectRequest, DocumentOpenRequest,
    DocumentOpenResponse, DocumentReleaseRequest, DocumentRelocateRequest, DocumentSaveAsRequest,
    DocumentSaveRequest, DocumentSaveResponse, DocumentSnapshot, DocumentTextEdit,
    DocumentTransaction, DocumentUntitledRequest, InvalidDocumentTextEdit,
    InvalidDocumentTransaction,
};
pub use envelope::{EventEnvelope, RequestEnvelope, ResponseEnvelope};
pub use error::{
    CREDENTIAL_EXPIRED, ErrorCode, FORBIDDEN, HOST_MISMATCH, INTERNAL, INVALID_PATH,
    INVALID_REQUEST, InvalidErrorCode, NOT_FOUND, PROTOCOL_ERROR, ProtocolError, RATE_LIMITED,
    REVISION_CONFLICT, RevisionConflict, UNAUTHORIZED, UNSUPPORTED, WORKSPACE_MISMATCH,
    WORKSPACE_UNAVAILABLE,
};
pub use event::EventBody;
pub use files::{
    CreateDirectoryRequest, CreateDirectoryResponse, FileTreeEntry, FileTreeKind, FileTreeRequest,
    FileTreeResponse,
};
pub use handshake::{ClientKind, HelloRequest, HelloResponse};
pub use ids::{
    ClientId, DocumentId, InvalidResourceId, RequestId, SessionId, TerminalId, WorkspaceId,
};
pub use operations::{WorkspaceOperation, WorkspaceOperationRequest, WorkspaceOperationResponse};
pub use remote_wire::{
    AuthMethod, AuthRequired, AuthenticateRequest, ClientMessage, InvalidSecretHex,
    OidcClientConfig, OidcRequest, PairRequest, PairedResponse, ServerMessage,
};
pub use request::RequestBody;
pub use response::ResponseBody;
pub use revision::{EventSequence, Revision};
pub use snapshot::{SnapshotReason, SnapshotRequest, SnapshotResponse, WorkspaceSnapshot};
pub use terminal::{
    TerminalCell, TerminalClaimRequest, TerminalColor, TerminalCommandResponse, TerminalCursor,
    TerminalDelta, TerminalInput, TerminalInputRequest, TerminalKey, TerminalKeyEvent,
    TerminalModes, TerminalModifiers, TerminalReleaseRequest, TerminalResizeRequest,
    TerminalRevision, TerminalRow, TerminalRowUpdate, TerminalScreen, TerminalScrollbackDelta,
    TerminalShell, TerminalSize, TerminalSnapshot, TerminalStartRequest, TerminalStartResponse,
    TerminalStatus, TerminalTerminateRequest,
};
pub use version::{
    CURRENT_PROTOCOL, InvalidProtocolRange, ProtocolRange, ProtocolVersion, VersionMismatch,
};
pub use workbench::{
    WorkbenchGetRequest, WorkbenchPutRequest, WorkbenchPutResponse, WorkbenchSnapshot,
    encode_workbench_view,
};
pub use workspace::{
    HostRegisterRequest, WorkspaceCloseRequest, WorkspaceCloseResponse, WorkspaceConnectRequest,
    WorkspaceConnectResponse, WorkspaceListRequest, WorkspaceListResponse, WorkspaceSummary,
};
