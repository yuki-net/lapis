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

/// 開始応答を待つ間に保持する Terminal 数とイベント数の上限。
/// 他 Session の Terminal イベントが混ざっても増え続けないようにする。
/// 上限を超えた分は、対応付け後の再同期 snapshot で回復する。
const MAX_PENDING_TERMINALS: usize = 16;
const MAX_PENDING_EVENTS_PER_TERMINAL: usize = 64;

pub(crate) struct Replica {
    pub(crate) workspace: WorkspaceId,
    pub(crate) entries: Vec<FileEntry>,
    pub(crate) documents: HashMap<DocumentId, DocumentState>,
    pub(crate) by_path: HashMap<PathBuf, DocumentId>,
    pub(crate) terminals: HashMap<String, TerminalState>,
    /// 開始応答で local ID と Host ID が対応付く前に届いた Terminal イベント。
    /// 対応付け後に到着順で適用する。
    pub(crate) pending_terminal_events: HashMap<String, Vec<TerminalEvent>>,
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
            pending_terminal_events: HashMap::new(),
            workbench: WorkbenchReplica::default(),
            profiles,
            next: 1,
            next_terminal: 1,
            initial: None,
            session,
        }
    }

    /// Host から届いた Terminal イベントを、対応済みなら poll 用に積み、
    /// 開始応答待ちの Terminal があれば対応付けまで保持する。
    pub(crate) fn apply_terminal_event(&mut self, host: &ApiTerminalId, event: TerminalEvent) {
        if let Some(state) = self
            .terminals
            .values_mut()
            .find(|state| state.host.as_ref() == Some(host))
        {
            push_terminal_event(state, event);
            return;
        }
        // 開始応答待ちが無ければ、この Terminal は再同期の snapshot で作る。
        if !self.terminals.values().any(|state| state.host.is_none()) {
            return;
        }
        let known = self.pending_terminal_events.contains_key(host.as_str());
        if !known && self.pending_terminal_events.len() >= MAX_PENDING_TERMINALS {
            return;
        }
        let pending = self
            .pending_terminal_events
            .entry(host.as_str().to_owned())
            .or_default();
        if pending.len() < MAX_PENDING_EVENTS_PER_TERMINAL {
            pending.push(event);
        }
    }

    /// 開始応答で local ID と Host ID を対応付け、応答前のイベントを到着順に適用する。
    pub(crate) fn connect_terminal(
        &mut self,
        local: &TerminalId,
        host: ApiTerminalId,
        snapshot: TerminalSnapshot,
    ) {
        let pending = self.pending_terminal_events.remove(host.as_str());
        if let Some(state) = self.terminals.get_mut(local.as_str()) {
            state.host = Some(host);
            state.snapshot = Some(snapshot.clone());
            push_terminal_event(state, TerminalEvent::Snapshot(snapshot));
            for event in pending.into_iter().flatten() {
                push_terminal_event(state, event);
            }
        }
        if !self.terminals.values().any(|state| state.host.is_none()) {
            // 対応付け待ちが無くなったら、他 Session の Terminal 用に保持した分を捨てる。
            self.pending_terminal_events.clear();
        }
    }
}

fn push_terminal_event(state: &mut TerminalState, event: TerminalEvent) {
    state.finished |= matches!(
        &event,
        TerminalEvent::Exited { .. } | TerminalEvent::Failed(_)
    );
    // UI に届けた owner を覚え、再同期の snapshot と比較できるようにする。
    if let TerminalEvent::Owner { owner } = &event {
        state.owner = owner.clone();
    }
    state.events.push(event);
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
    /// UI 向けに最後に積んだ操作 Session。
    /// 再同期の snapshot と比べ、変わった場合だけ通知するために保持する。
    pub(crate) owner: Option<String>,
    pub(crate) events: Vec<TerminalEvent>,
    pub(crate) finished: bool,
}

/// owner を更新し、UI が最後に知った値と変わった場合だけ通知イベントを積む。
pub(crate) fn set_terminal_owner(state: &mut TerminalState, owner: Option<String>) -> bool {
    if state.owner == owner {
        return false;
    }
    push_terminal_event(state, TerminalEvent::Owner { owner });
    true
}

/// Host snapshot の owner を状態へ反映し、UI への通知が必要かを返す。
///
/// `discarded_owner` は、置き換え前の `events` に未 poll の Owner があったか。
/// その場合は値が同じでも UI へ届いていないため、通知し直す。
pub(crate) fn resync_terminal_owner(
    state: &mut TerminalState,
    owner: Option<String>,
    discarded_owner: bool,
) -> bool {
    if !discarded_owner && state.owner == owner {
        return false;
    }
    push_terminal_event(state, TerminalEvent::Owner { owner });
    true
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

/// Host snapshot から、接続直後に使うローカル Terminal 状態を作る。
pub(crate) fn terminal_state(local: &TerminalId, snapshot: &WireTerminalSnapshot) -> TerminalState {
    let modeled = model_terminal_snapshot(local, snapshot);
    let mut state = TerminalState {
        host: Some(snapshot.terminal_id.clone()),
        snapshot: Some(modeled.clone()),
        owner: None,
        events: vec![TerminalEvent::Snapshot(modeled)],
        finished: false,
    };
    // 初期 snapshot の owner も UI が知る必要がある。
    set_terminal_owner(
        &mut state,
        snapshot
            .owner
            .as_ref()
            .map(|owner| owner.as_str().to_owned()),
    );
    state
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

#[cfg(test)]
mod tests {
    use std::{
        path::Path,
        sync::{Arc, Mutex},
    };

    use client_api::{
        EventSequence, TerminalSnapshot as WireTerminalSnapshot, WorkspaceId as ApiWorkspaceId,
        WorkspaceSnapshot, WorkspaceSummary,
    };
    use terminal::{
        TerminalBackend, TerminalDelta, TerminalError, TerminalEvent, TerminalId, TerminalInput,
        TerminalRow, TerminalRowUpdate, TerminalScreen, TerminalSize, TerminalSnapshot,
        TerminalStatus,
    };

    use super::*;
    use crate::{TerminalSession, web_cache::apply_snapshot};

    fn replica() -> Replica {
        Replica::new(
            WorkspaceId::new("workspace-a"),
            Vec::new(),
            None,
            Default::default(),
        )
    }

    fn local_with_pending_start(replica: &mut Replica) -> TerminalId {
        let local = TerminalId::new("web-terminal-1");
        replica.next_terminal = 2;
        replica.terminals.insert(
            local.as_str().to_owned(),
            TerminalState {
                host: None,
                snapshot: None,
                owner: None,
                events: Vec::new(),
                finished: false,
            },
        );
        local
    }

    fn host(id: &str) -> ApiTerminalId {
        ApiTerminalId::try_new(id.to_owned()).unwrap()
    }

    fn row(columns: u16, text: &str) -> TerminalRow {
        let mut row = TerminalRow::empty(columns);
        for (column, character) in text.chars().take(columns as usize).enumerate() {
            row.cells[column].text = character.to_string();
        }
        row
    }

    fn delta(revision: u64, text: &str) -> TerminalDelta {
        TerminalDelta {
            revision,
            size: TerminalSize::new(80, 24),
            changed_rows: vec![TerminalRowUpdate {
                row: 0,
                value: row(80, text),
            }],
            cursor: Default::default(),
            modes: Default::default(),
            scrollback: None,
        }
    }

    fn local_snapshot(id: &TerminalId, revision: u64, text: &str) -> TerminalSnapshot {
        let mut screen = TerminalScreen::empty(TerminalSize::new(80, 24));
        screen.rows[0] = row(80, text);
        TerminalSnapshot {
            id: id.clone(),
            cwd: std::path::PathBuf::new(),
            status: TerminalStatus::Running,
            revision,
            screen,
        }
    }

    fn wire_snapshot(revision: u64, text: &str) -> WireTerminalSnapshot {
        let mut screen = TerminalScreen::empty(TerminalSize::new(80, 24));
        screen.rows[0] = row(80, text);
        WireTerminalSnapshot {
            terminal_id: host("terminal-1"),
            workspace_id: ApiWorkspaceId::try_new("workspace-a").unwrap(),
            cwd: None,
            status: client_api::TerminalStatus::Running,
            revision,
            screen,
            owner: None,
        }
    }

    fn owned_wire_snapshot(revision: u64, text: &str, owner: &str) -> WireTerminalSnapshot {
        let mut snapshot = wire_snapshot(revision, text);
        snapshot.owner = Some(client_api::SessionId::try_new(owner.to_owned()).unwrap());
        snapshot
    }

    #[test]
    fn terminal_events_before_the_start_response_are_replayed_after_association() {
        let mut replica = replica();
        let local = local_with_pending_start(&mut replica);
        let host = host("terminal-1");
        replica.apply_terminal_event(&host, TerminalEvent::Delta(delta(1, "prompt")));
        replica.apply_terminal_event(
            &host,
            TerminalEvent::Owner {
                owner: Some("session-a".to_owned()),
            },
        );

        replica.connect_terminal(&local, host.clone(), local_snapshot(&local, 0, ""));

        let state = replica.terminals.get(local.as_str()).unwrap();
        assert_eq!(state.host.as_ref(), Some(&host));
        assert_eq!(state.snapshot.as_ref().unwrap().revision, 0);
        assert!(
            matches!(&state.events[0], TerminalEvent::Snapshot(snapshot) if snapshot.revision == 0)
        );
        assert!(matches!(&state.events[1], TerminalEvent::Delta(delta) if delta.revision == 1));
        assert!(matches!(
            &state.events[2],
            TerminalEvent::Owner { owner: Some(owner) } if owner == "session-a"
        ));
        assert!(replica.pending_terminal_events.is_empty());
    }

    #[test]
    fn terminal_events_without_a_pending_start_are_not_retained() {
        let mut replica = replica();
        replica.apply_terminal_event(
            &host("terminal-1"),
            TerminalEvent::Delta(delta(1, "other session")),
        );

        assert!(replica.pending_terminal_events.is_empty());
    }

    #[test]
    fn associated_terminal_events_are_appended_and_mark_the_end() {
        let mut replica = replica();
        let local = local_with_pending_start(&mut replica);
        let host = host("terminal-1");
        replica.connect_terminal(&local, host.clone(), local_snapshot(&local, 0, ""));

        replica.apply_terminal_event(&host, TerminalEvent::Delta(delta(1, "done")));
        replica.apply_terminal_event(&host, TerminalEvent::Exited { code: Some(0) });

        let state = replica.terminals.get(local.as_str()).unwrap();
        assert!(state.finished);
        assert!(matches!(
            &state.events[1],
            TerminalEvent::Delta(delta) if delta.revision == 1
        ));
        assert!(matches!(
            &state.events[2],
            TerminalEvent::Exited { code: Some(0) }
        ));
    }

    /// 修正前の WebBackend と同じ同期契約（キャッシュを返し、再同期を要求する）を持つ backend。
    struct WebLikeBackend {
        inner: Mutex<WebLikeState>,
    }

    struct WebLikeState {
        replica: Replica,
        resync_requests: usize,
    }

    impl WebLikeBackend {
        fn new() -> Self {
            Self {
                inner: Mutex::new(WebLikeState {
                    replica: replica(),
                    resync_requests: 0,
                }),
            }
        }

        fn deliver(&self, host: &ApiTerminalId, event: TerminalEvent) {
            self.inner
                .lock()
                .unwrap()
                .replica
                .apply_terminal_event(host, event);
        }

        fn apply_start_response(
            &self,
            local: &TerminalId,
            host: ApiTerminalId,
            snapshot: TerminalSnapshot,
        ) {
            self.inner
                .lock()
                .unwrap()
                .replica
                .connect_terminal(local, host, snapshot);
        }

        fn apply_host_snapshot(&self, terminal: WireTerminalSnapshot) {
            let mut inner = self.inner.lock().unwrap();
            apply_snapshot(
                &mut inner.replica,
                &WorkspaceSnapshot {
                    event_watermark: EventSequence::new(1),
                    workspace: WorkspaceSummary {
                        workspace_id: ApiWorkspaceId::try_new("workspace-a").unwrap(),
                        name: "Workspace A".to_owned(),
                    },
                    documents: Vec::new(),
                    terminals: vec![terminal],
                    git_repositories: Vec::new(),
                },
            );
        }

        fn resync_requests(&self) -> usize {
            self.inner.lock().unwrap().resync_requests
        }
    }

    impl TerminalBackend for WebLikeBackend {
        fn start(
            &self,
            _cwd: &Path,
            _columns: u16,
            _rows: u16,
        ) -> Result<TerminalId, TerminalError> {
            let local = TerminalId::new("web-terminal-1");
            self.inner.lock().unwrap().replica.terminals.insert(
                local.as_str().to_owned(),
                TerminalState {
                    host: None,
                    snapshot: None,
                    owner: None,
                    events: Vec::new(),
                    finished: false,
                },
            );
            Ok(local)
        }

        fn attach(&self, _cwd: &Path, id: &TerminalId) -> Result<TerminalSnapshot, TerminalError> {
            let mut inner = self.inner.lock().unwrap();
            let snapshot = inner
                .replica
                .terminals
                .get(id.as_str())
                .and_then(|state| state.snapshot.clone())
                .ok_or_else(|| TerminalError::new("Terminalがありません"))?;
            // WebBackend::attach と同じく、キャッシュを返しつつ Host の snapshot を取り直す。
            inner.resync_requests += 1;
            Ok(snapshot)
        }

        fn input(&self, _id: &TerminalId, _input: &TerminalInput) -> Result<(), TerminalError> {
            Ok(())
        }

        fn resize(&self, _id: &TerminalId, _columns: u16, _rows: u16) -> Result<(), TerminalError> {
            Ok(())
        }

        fn poll(&self, id: &TerminalId) -> Result<Vec<TerminalEvent>, TerminalError> {
            let mut inner = self.inner.lock().unwrap();
            let state = inner
                .replica
                .terminals
                .get_mut(id.as_str())
                .ok_or_else(|| TerminalError::new("Terminalがありません"))?;
            Ok(std::mem::take(&mut state.events))
        }

        fn terminate(&self, _id: &TerminalId) -> Result<(), TerminalError> {
            Ok(())
        }
    }

    #[test]
    fn start_response_replays_the_first_delta_instead_of_leaving_the_screen_blank() {
        let backend = Arc::new(WebLikeBackend::new());
        let mut session = TerminalSession::new(backend.clone());
        let local = session.start(Path::new("repo"), 80, 24).unwrap();
        assert_eq!(session.terminals()[0].revision, 0);

        let host = host("terminal-1");
        // Host 応答前に revision 1 が届く。対応付け前なので保留される。
        backend.deliver(&host, TerminalEvent::Delta(delta(1, "prompt")));
        backend.apply_start_response(&local, host, local_snapshot(&local, 0, ""));

        assert!(session.refresh().unwrap());
        assert_eq!(session.terminals()[0].revision, 1);
        assert!(
            session.terminals()[0].screen.rows[0]
                .plain_text()
                .starts_with("prompt")
        );
        assert_eq!(backend.resync_requests(), 0);
    }

    #[test]
    fn delta_gap_recovers_from_the_host_snapshot() {
        let backend = Arc::new(WebLikeBackend::new());
        let mut session = TerminalSession::new(backend.clone());
        let local = session.start(Path::new("repo"), 80, 24).unwrap();

        let host = host("terminal-1");
        // revision 1 は失われたものとして、開始応答と revision 2 だけが届く。
        backend.apply_start_response(&local, host.clone(), local_snapshot(&local, 0, ""));
        backend.deliver(&host, TerminalEvent::Delta(delta(2, "prompt$ ")));

        // 欠番を検出して attach に入る。キャッシュは revision 0 のまま。
        assert!(session.refresh().unwrap());
        assert_eq!(session.terminals()[0].revision, 0);
        assert_eq!(backend.resync_requests(), 1);

        // attach が要求した再同期で Host の現在の snapshot が反映される。
        backend.apply_host_snapshot(wire_snapshot(2, "prompt$ "));
        assert!(session.refresh().unwrap());
        assert_eq!(session.terminals()[0].revision, 2);
        assert!(
            session.terminals()[0].screen.rows[0]
                .plain_text()
                .starts_with("prompt$ ")
        );
    }

    #[test]
    fn resync_restores_the_owner_that_the_pending_limit_dropped() {
        let backend = Arc::new(WebLikeBackend::new());
        let mut session = TerminalSession::new(backend.clone());
        let local = session.start(Path::new("repo"), 80, 24).unwrap();
        let host = host("terminal-1");
        // 保留が上限に達して terminal.owner を取りこぼしたまま、対応付けだけが済む。
        backend.apply_start_response(&local, host.clone(), local_snapshot(&local, 0, ""));
        assert!(session.refresh().unwrap());
        assert_eq!(session.owner(&local), None);

        // delta 欠番で attach に入り、再同期 snapshot から owner を復元する。
        backend.deliver(&host, TerminalEvent::Delta(delta(3, "prompt$ ")));
        assert!(session.refresh().unwrap());
        assert_eq!(backend.resync_requests(), 1);
        backend.apply_host_snapshot(owned_wire_snapshot(3, "prompt$ ", "session-b"));

        // 画面差分が無くても、owner の復元だけで反映する。
        assert!(session.refresh().unwrap());
        assert_eq!(session.owner(&local), Some("session-b"));
    }
}
