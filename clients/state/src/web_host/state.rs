//! Web transport が保持するローカル複製と、wire DTO から UI モデルへの変換。
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use client_api::{
    DocumentEncoding, DocumentId as ApiDocumentId, DocumentSnapshot, Revision as ApiRevision,
    TerminalId as ApiTerminalId, TerminalSnapshot as WireTerminalSnapshot,
    TerminalStatus as WireTerminalStatus, WorkbenchSnapshot,
};
use document::{Document, DocumentCache, Encoding, ExternalChange, Revision};
use editor_core::{DocumentId, SessionId, WorkspaceId};
use terminal::{TerminalEvent, TerminalId, TerminalSnapshot, TerminalStatus};
use workbench::{VersionedViewState, WorkbenchReplica};
use workspace::FileEntry;

/// Web は Host の OS に依存しない仮想 root を使う。
pub const ROOT: &str = "/Lapis Host";

pub(crate) struct Replica {
    pub(crate) workspace: WorkspaceId,
    pub(crate) entries: Vec<FileEntry>,
    pub(crate) documents: HashMap<DocumentId, DocumentState>,
    pub(crate) by_path: HashMap<PathBuf, DocumentId>,
    pub(crate) terminals: HashMap<String, TerminalState>,
    pub(crate) workbench: WorkbenchReplica,
    pub(crate) profiles: terminal::TerminalProfiles,
    pub(crate) next: u64,
    pub(crate) next_terminal: u64,
    pub(crate) initial: Option<PathBuf>,
    /// この接続の Session。Host の owner と比較して編集可否を決める。
    pub(crate) session: Option<SessionId>,
}

impl Replica {
    pub(crate) fn new(
        workspace: WorkspaceId,
        entries: Vec<FileEntry>,
        session: Option<SessionId>,
        profiles: terminal::TerminalProfiles,
    ) -> Self {
        Self {
            workspace,
            entries,
            documents: HashMap::new(),
            by_path: HashMap::new(),
            terminals: HashMap::new(),
            workbench: WorkbenchReplica::default(),
            profiles,
            next: 1,
            next_terminal: 1,
            initial: None,
            session,
        }
    }
}

pub(crate) fn cache_of(
    local: &DocumentId,
    document: &Document,
    external_change: ExternalChange,
    owner: Option<&SessionId>,
    session: Option<&SessionId>,
) -> DocumentCache {
    DocumentCache {
        reused: true,
        id: local.clone(),
        document: document.clone(),
        can_undo: document.can_undo(),
        can_redo: document.can_redo(),
        external_change,
        owner: owner.cloned(),
        is_owner: owner
            .zip(session)
            .is_some_and(|(owner, session)| owner == session),
    }
}

/// wire の owner を UI 側の Session に変換する。
pub(crate) fn owner_of(snapshot: &DocumentSnapshot) -> Option<SessionId> {
    snapshot
        .owner
        .as_ref()
        .map(|owner| SessionId::new(owner.as_str()))
}

pub(crate) fn versioned_workbench(snapshot: WorkbenchSnapshot) -> Option<VersionedViewState> {
    let state = VersionedViewState {
        revision: snapshot.revision,
        view: serde_json::from_value(snapshot.view).ok(),
    };
    state.is_consistent().then_some(state)
}

pub(crate) struct DocumentState {
    pub(crate) host: Option<ApiDocumentId>,
    pub(crate) path: Option<PathBuf>,
    pub(crate) document: Document,
    pub(crate) acked: String,
    pub(crate) host_revision: ApiRevision,
    pub(crate) loaded: bool,
    pub(crate) external_change: ExternalChange,
    /// Host が通知した編集権の所有者。
    pub(crate) owner: Option<SessionId>,
}

pub(crate) struct TerminalState {
    pub(crate) host: Option<ApiTerminalId>,
    pub(crate) snapshot: Option<TerminalSnapshot>,
    pub(crate) events: Vec<TerminalEvent>,
    pub(crate) finished: bool,
}

pub(crate) fn local_id(replica: &mut Replica) -> DocumentId {
    let id = DocumentId::new(format!("web-{}", replica.next));
    replica.next += 1;
    id
}

pub(crate) fn document_from_snapshot(
    snapshot: &DocumentSnapshot,
    path: Option<PathBuf>,
) -> Document {
    Document::from_snapshot(
        path,
        snapshot.content.clone(),
        match snapshot.encoding {
            DocumentEncoding::Utf8 => Encoding::Utf8,
            DocumentEncoding::Utf8Bom => Encoding::Utf8Bom,
            DocumentEncoding::Utf16Le => Encoding::Utf16Le,
            DocumentEncoding::Utf16Be => Encoding::Utf16Be,
        },
        Revision::new(snapshot.revision.number()),
        snapshot.dirty,
    )
}

pub(crate) fn model_terminal_status(status: WireTerminalStatus) -> TerminalStatus {
    match status {
        WireTerminalStatus::Running => TerminalStatus::Running,
        WireTerminalStatus::Exited | WireTerminalStatus::Terminated => TerminalStatus::Exited,
        WireTerminalStatus::Failed => TerminalStatus::Failed,
        WireTerminalStatus::Disconnected => TerminalStatus::Disconnected,
    }
}

pub(crate) fn model_terminal_snapshot(
    local: &TerminalId,
    snapshot: &WireTerminalSnapshot,
) -> TerminalSnapshot {
    TerminalSnapshot {
        id: local.clone(),
        cwd: snapshot
            .cwd
            .as_ref()
            .map(|path| Path::new(ROOT).join(path.as_str()))
            .unwrap_or_default(),
        status: model_terminal_status(snapshot.status),
        revision: snapshot.revision,
        screen: snapshot.screen.clone(),
    }
}

pub(crate) fn diff_chars(previous: &str, next: &str) -> (u64, u64, String) {
    let previous_chars: Vec<char> = previous.chars().collect();
    let next_chars: Vec<char> = next.chars().collect();
    let mut start = 0;
    while start < previous_chars.len()
        && start < next_chars.len()
        && previous_chars[start] == next_chars[start]
    {
        start += 1;
    }
    let mut tail = 0;
    while tail < previous_chars.len().saturating_sub(start)
        && tail < next_chars.len().saturating_sub(start)
        && previous_chars[previous_chars.len() - 1 - tail]
            == next_chars[next_chars.len() - 1 - tail]
    {
        tail += 1;
    }
    let end = previous_chars.len() - tail;
    let replacement: String = next_chars[start..next_chars.len() - tail].iter().collect();
    (start as u64, end as u64, replacement)
}
