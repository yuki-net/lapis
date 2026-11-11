//! Workspace ごとに保持するクライアント複製と、離れていた間の差分の反映。
//!
//! 保持するのはページの生存中だけであり、永続化はしない。
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use std::collections::VecDeque;

use client_api::{
    DocumentExternalChange, DocumentSnapshot, TerminalSnapshot as WireTerminalSnapshot,
    WorkspaceSnapshot,
};
use document::ExternalChange;
use editor_core::WorkspaceId;
use terminal::TerminalId;

use crate::web_replica::{
    DocumentState, Replica, TerminalState, document_from_snapshot, model_terminal_snapshot,
    owner_of,
};

/// 切替で離れた Workspace の複製。
pub(crate) struct CachedWorkspace {
    pub(crate) replica: Replica,
}

/// Workspace ごとの複製を古い順に保持し、上限を超えた分を破棄する。
///
/// 破棄した Workspace へ戻るときは、保持が無いため snapshot から作り直す。
pub(crate) struct WorkspaceCache {
    entries: VecDeque<CachedWorkspace>,
    limit: usize,
}

impl WorkspaceCache {
    pub(crate) const fn new(limit: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            limit: if limit == 0 { 1 } else { limit },
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// 保持済みの複製を取り出す。取り出した Workspace は再度 insert するまで保持されない。
    pub(crate) fn take(&mut self, workspace: &WorkspaceId) -> Option<CachedWorkspace> {
        let index = self
            .entries
            .iter()
            .position(|entry| &entry.replica.workspace == workspace)?;
        self.entries.remove(index)
    }

    /// 複製を保持する。同じ Workspace の古い保持は置き換える。
    ///
    /// 上限を超えた場合は最も古い保持を返す。
    pub(crate) fn insert(&mut self, workspace: CachedWorkspace) -> Option<CachedWorkspace> {
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.replica.workspace == workspace.replica.workspace)
        {
            self.entries.remove(index);
        }
        self.entries.push_back(workspace);
        if self.entries.len() > self.limit {
            self.entries.pop_front()
        } else {
            None
        }
    }
}

/// `snapshot.resync` で生じた変更の種類。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SnapshotDelta {
    /// `poll` や再描画で表示を更新する必要がある。
    pub(crate) display: bool,
    /// Editor が購読している文書状態に変更がある。
    pub(crate) documents: bool,
}

/// `snapshot.resync` の結果を保持済みの複製へ反映する。
///
/// 文書は Host の内容へ置き換え、Terminal は snapshot を次の `poll` で返す。
pub(crate) fn apply_snapshot(replica: &mut Replica, snapshot: &WorkspaceSnapshot) -> SnapshotDelta {
    let mut delta = SnapshotDelta::default();
    for document in &snapshot.documents {
        let changed = apply_document(replica, document);
        delta.display |= changed;
        delta.documents |= changed;
    }
    for terminal in &snapshot.terminals {
        delta.display |= apply_terminal(replica, terminal);
    }
    delta
}

fn apply_document(replica: &mut Replica, snapshot: &DocumentSnapshot) -> bool {
    let Some(state) = replica
        .documents
        .values_mut()
        .find(|state| state.host.as_ref() == Some(&snapshot.document_id))
    else {
        return false;
    };
    let owner = owner_of(snapshot);
    // 未送信のローカル変更がある場合は、snapshot で上書きしない。
    if state.document.content_for_save() != state.acked {
        // Host 側の revision と owner がローカル編集の基準から動いていれば、
        // 上書きせずに競合として記録する。ローカル本文は復旧できるよう保持する。
        let conflict = snapshot.revision != state.host_revision || snapshot.content != state.acked;
        // 競合していなくても、外部変更の状態は Snapshot から復旧する。
        // 競合している場合は Host の本文で置き換えていないことを Modified で示す。
        let external_change = if conflict {
            match snapshot.external_change {
                DocumentExternalChange::Unchanged => ExternalChange::Modified,
                change => model_external_change(change),
            }
        } else {
            model_external_change(snapshot.external_change)
        };
        let mut changed = state.external_change != external_change;
        state.external_change = external_change;
        changed |= state.owner != owner;
        state.owner = owner;
        // host_revision と acked はローカル編集の基準なので、競合中は更新しない。
        return changed;
    }
    let document = document_from_snapshot(snapshot, state.path.clone());
    let external_change = model_external_change(snapshot.external_change);
    let changed = state.document != document
        || state.host_revision != snapshot.revision
        || state.owner != owner
        || state.external_change != external_change;
    state.document = document;
    state.acked = snapshot.content.clone();
    state.host_revision = snapshot.revision;
    state.loaded = true;
    state.external_change = external_change;
    state.owner = owner;
    changed
}

/// `document.inspect` の結果を状態へ反映する。
///
/// `preserve_local` が true で未送信のローカル変更がある場合は本文を保持し、
/// Host 側の変更を外部変更（競合）として記録する。false の場合は Host を正本とする。
pub(crate) fn apply_inspection(
    state: &mut DocumentState,
    snapshot: &DocumentSnapshot,
    preserve_local: bool,
) -> bool {
    let owner = owner_of(snapshot);
    if preserve_local && state.document.content_for_save() != state.acked {
        let mut changed = state.external_change != ExternalChange::Modified;
        state.external_change = ExternalChange::Modified;
        changed |= state.owner != owner;
        state.owner = owner;
        // acked と host_revision はローカル編集の基準なので更新しない。
        return changed;
    }
    let document = document_from_snapshot(snapshot, state.path.clone());
    let changed = state.document != document
        || state.acked != snapshot.content
        || state.host_revision != snapshot.revision
        || state.owner != owner
        || state.external_change != ExternalChange::Modified;
    state.document = document;
    state.acked = snapshot.content.clone();
    state.host_revision = snapshot.revision;
    state.loaded = true;
    state.external_change = ExternalChange::Modified;
    state.owner = owner;
    changed
}

fn apply_terminal(replica: &mut Replica, terminal: &WireTerminalSnapshot) -> bool {
    if let Some(state) = replica
        .terminals
        .values_mut()
        .find(|state| state.host.as_ref() == Some(&terminal.terminal_id))
    {
        let local = state
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.id.clone())
            .unwrap_or_else(|| TerminalId::new(terminal.terminal_id.as_str()));
        let snapshot = model_terminal_snapshot(&local, terminal);
        let changed = state.snapshot.as_ref() != Some(&snapshot);
        state.snapshot = Some(snapshot.clone());
        state.events.clear();
        state
            .events
            .push(terminal::TerminalEvent::Snapshot(snapshot));
        state.finished = false;
        return changed;
    }
    let local = TerminalId::new(format!("web-terminal-{}", replica.next_terminal));
    replica.next_terminal += 1;
    let snapshot = model_terminal_snapshot(&local, terminal);
    replica.terminals.insert(
        local.as_str().to_owned(),
        TerminalState {
            host: Some(terminal.terminal_id.clone()),
            snapshot: Some(snapshot.clone()),
            events: vec![terminal::TerminalEvent::Snapshot(snapshot)],
            finished: false,
        },
    );
    true
}

fn model_external_change(change: DocumentExternalChange) -> ExternalChange {
    match change {
        DocumentExternalChange::Unchanged => ExternalChange::Unchanged,
        DocumentExternalChange::Modified => ExternalChange::Modified,
        DocumentExternalChange::Deleted => ExternalChange::Deleted,
    }
}

/// 保持済みの複製を、変更の有無を問わず1つの文書状態として組み立てる。
///
/// テストと、保持を破棄した場合の snapshot 復帰の境界をそろえるために使う。
#[cfg(test)]
pub(crate) fn cached_document<'a>(
    replica: &'a Replica,
    host: &client_api::DocumentId,
) -> Option<&'a crate::web_replica::DocumentState> {
    replica
        .documents
        .values()
        .find(|state| state.host.as_ref() == Some(host))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use client_api::{
        DocumentEncoding, DocumentId as ApiDocumentId, EventSequence, Revision as ApiRevision,
        SessionId as ApiSessionId, TerminalId as ApiTerminalId, TerminalSnapshot, TerminalStatus,
        WorkspaceId as ApiWorkspaceId, WorkspaceSummary,
    };
    use document::{Document, Encoding, Revision};
    use editor_core::{DocumentId, SessionId, WorkspaceId};
    use terminal::{TerminalScreen, TerminalSize};

    use super::*;
    use crate::web_replica::{DocumentState, ROOT};

    fn workspace(value: &str) -> WorkspaceId {
        WorkspaceId::new(value)
    }

    fn cached(id: &WorkspaceId, text: &str) -> CachedWorkspace {
        let mut replica = Replica::new(
            id.clone(),
            Vec::new(),
            Some(SessionId::new("session")),
            Default::default(),
        );
        replica.next = 7;
        replica.next_terminal = 4;
        replica.documents.insert(
            DocumentId::new("web-1"),
            DocumentState {
                host: Some(ApiDocumentId::try_new("document-1").unwrap()),
                path: Some(PathBuf::from(ROOT).join("README.md")),
                document: Document::from_snapshot(
                    Some(PathBuf::from(ROOT).join("README.md")),
                    text.to_owned(),
                    Encoding::Utf8,
                    Revision::new(1),
                    false,
                ),
                acked: text.to_owned(),
                host_revision: ApiRevision::new(1),
                loaded: true,
                external_change: ExternalChange::Unchanged,
                owner: None,
            },
        );
        CachedWorkspace { replica }
    }

    fn wire_document(content: &str) -> DocumentSnapshot {
        DocumentSnapshot {
            document_id: ApiDocumentId::try_new("document-1").unwrap(),
            workspace_id: ApiWorkspaceId::try_new("workspace-a").unwrap(),
            path: Some(
                workspace::WorkspaceRelativePath::from_path(std::path::Path::new("README.md"))
                    .unwrap(),
            ),
            content: content.to_owned(),
            encoding: DocumentEncoding::Utf8,
            revision: ApiRevision::new(2),
            dirty: false,
            can_undo: false,
            can_redo: false,
            external_change: DocumentExternalChange::Unchanged,
            owner: None,
        }
    }

    fn wire_terminal(revision: u64) -> TerminalSnapshot {
        TerminalSnapshot {
            terminal_id: ApiTerminalId::try_new("terminal-1").unwrap(),
            workspace_id: ApiWorkspaceId::try_new("workspace-a").unwrap(),
            cwd: None,
            status: TerminalStatus::Running,
            revision,
            screen: TerminalScreen::empty(TerminalSize::new(80, 24)),
            owner: None,
        }
    }

    fn snapshot(
        documents: Vec<DocumentSnapshot>,
        terminals: Vec<TerminalSnapshot>,
    ) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            event_watermark: EventSequence::new(3),
            workspace: WorkspaceSummary {
                workspace_id: ApiWorkspaceId::try_new("workspace-a").unwrap(),
                name: "Workspace A".to_owned(),
            },
            documents,
            terminals,
            git_repositories: Vec::new(),
        }
    }

    #[test]
    fn restored_replica_is_reused_and_catches_up_with_delta_resync() {
        let workspace = workspace("workspace-a");
        let mut cache = WorkspaceCache::new(4);
        cache.insert(cached(&workspace, "before"));

        let restored = cache.take(&workspace).expect("保持した複製を復元できる");
        assert!(cache.take(&workspace).is_none());
        let mut replica = restored.replica;
        // 復元直後は保持していた表示状態がそのまま使える。
        assert_eq!(
            cached_document(&replica, &ApiDocumentId::try_new("document-1").unwrap())
                .unwrap()
                .document
                .content_for_save(),
            "before"
        );
        assert_eq!(replica.next, 7);
        assert_eq!(replica.next_terminal, 4);

        // 離れていた間の差分だけを snapshot.resync で反映する。
        let terminal = wire_terminal(5);
        replica.terminals.insert(
            "web-terminal-1".to_owned(),
            TerminalState {
                host: Some(ApiTerminalId::try_new("terminal-1").unwrap()),
                snapshot: None,
                events: Vec::new(),
                finished: false,
            },
        );
        let changed = apply_snapshot(
            &mut replica,
            &snapshot(vec![wire_document("after")], vec![terminal]),
        );

        assert!(changed.display);
        assert!(changed.documents);
        let state =
            cached_document(&replica, &ApiDocumentId::try_new("document-1").unwrap()).unwrap();
        assert_eq!(state.document.content_for_save(), "after");
        assert_eq!(state.acked, "after");
        assert_eq!(state.host_revision, ApiRevision::new(2));
        // 既存の Terminal は新しい snapshot を次の poll で返す。
        let terminal_state = replica.terminals.get("web-terminal-1").unwrap();
        assert_eq!(terminal_state.snapshot.as_ref().unwrap().revision, 5);
        assert_eq!(
            terminal_state.events,
            vec![terminal::TerminalEvent::Snapshot(
                terminal_state.snapshot.clone().unwrap()
            )]
        );
        // 復元で使った識別子の採番は維持する。
        assert_eq!(replica.next, 7);
        assert_eq!(replica.next_terminal, 4);
    }

    #[test]
    fn resync_adds_terminals_that_appeared_while_away() {
        let workspace = workspace("workspace-a");
        let mut replica = cached(&workspace, "text").replica;

        let delta = apply_snapshot(&mut replica, &snapshot(Vec::new(), vec![wire_terminal(2)]));

        // 離れている間に始まった Terminal も、戻ったときの resync で見える。
        assert!(delta.display);
        assert!(
            !delta.documents,
            "Terminal だけの変更で Editor を再同期しない"
        );
        assert_eq!(replica.terminals.len(), 1);
        let state = replica.terminals.values().next().unwrap();
        assert_eq!(
            state.host,
            Some(ApiTerminalId::try_new("terminal-1").unwrap())
        );
        assert_eq!(
            state.snapshot.as_ref().unwrap().id.as_str(),
            "web-terminal-4"
        );
        assert_eq!(replica.next_terminal, 5);
    }

    #[test]
    fn resync_notifies_when_only_the_external_change_differs() {
        let workspace = workspace("workspace-a");
        let mut replica = cached(&workspace, "text").replica;
        let mut remote = wire_document("text");
        remote.revision = ApiRevision::new(1);
        remote.external_change = DocumentExternalChange::Modified;

        let delta = apply_snapshot(&mut replica, &snapshot(vec![remote], Vec::new()));

        // 本文・revision・owner が同じでも、外部変更の状態は Editor に通知する。
        assert!(delta.documents);
        let state =
            cached_document(&replica, &ApiDocumentId::try_new("document-1").unwrap()).unwrap();
        assert_eq!(state.document.content_for_save(), "text");
        assert_eq!(state.host_revision, ApiRevision::new(1));
        assert_eq!(state.external_change, ExternalChange::Modified);
    }

    #[test]
    fn resync_restores_external_change_for_unsent_local_changes() {
        let workspace = workspace("workspace-a");
        let mut replica = cached(&workspace, "before").replica;
        {
            let state = replica.documents.values_mut().next().unwrap();
            state.document = Document::from_snapshot(
                state.path.clone(),
                "local edit".to_owned(),
                Encoding::Utf8,
                Revision::new(2),
                true,
            );
        }
        let mut remote = wire_document("before");
        remote.revision = ApiRevision::new(1);
        remote.external_change = DocumentExternalChange::Modified;

        let delta = apply_snapshot(&mut replica, &snapshot(vec![remote], Vec::new()));

        assert!(delta.documents);
        let state =
            cached_document(&replica, &ApiDocumentId::try_new("document-1").unwrap()).unwrap();
        // 本文と revision が編集基準と同じでも、外部変更の状態は Snapshot から反映する。
        assert_eq!(state.document.content_for_save(), "local edit");
        assert_eq!(state.acked, "before");
        assert_eq!(state.host_revision, ApiRevision::new(1));
        assert_eq!(state.external_change, ExternalChange::Modified);
    }

    #[test]
    fn resync_keeps_unsent_local_changes_and_records_conflict() {
        let workspace = workspace("workspace-a");
        let mut replica = cached(&workspace, "before").replica;
        {
            let state = replica.documents.values_mut().next().unwrap();
            state.document = Document::from_snapshot(
                state.path.clone(),
                "local edit".to_owned(),
                Encoding::Utf8,
                Revision::new(2),
                true,
            );
        }

        let delta = apply_snapshot(
            &mut replica,
            &snapshot(vec![wire_document("remote")], Vec::new()),
        );

        assert!(delta.documents, "競合した文書は Editor にも通知する");
        let state =
            cached_document(&replica, &ApiDocumentId::try_new("document-1").unwrap()).unwrap();
        // Host の本文で上書きせず、未送信のローカル編集を復旧できる形で残す。
        assert_eq!(state.document.content_for_save(), "local edit");
        assert_eq!(state.acked, "before");
        // 古い revision を保存の基準として保持し、競合を外部変更として記録する。
        assert_eq!(state.host_revision, ApiRevision::new(1));
        assert_eq!(state.external_change, ExternalChange::Modified);
    }

    #[test]
    fn resync_records_the_host_owner_for_unsent_local_changes() {
        let workspace = workspace("workspace-a");
        let mut replica = cached(&workspace, "before").replica;
        {
            let state = replica.documents.values_mut().next().unwrap();
            state.document = Document::from_snapshot(
                state.path.clone(),
                "local edit".to_owned(),
                Encoding::Utf8,
                Revision::new(2),
                true,
            );
        }
        let mut remote = wire_document("remote");
        remote.owner = Some(ApiSessionId::try_new("session-2").unwrap());

        apply_snapshot(&mut replica, &snapshot(vec![remote], Vec::new()));

        let state =
            cached_document(&replica, &ApiDocumentId::try_new("document-1").unwrap()).unwrap();
        // Host 側で編集権が移っていても、ローカル編集を失わずに owner を記録する。
        assert_eq!(state.document.content_for_save(), "local edit");
        assert_eq!(state.owner, Some(SessionId::new("session-2")));
        assert_eq!(state.external_change, ExternalChange::Modified);
    }

    #[test]
    fn inspection_preserves_unsent_local_changes_and_records_conflict() {
        let workspace = workspace("workspace-a");
        let mut replica = cached(&workspace, "before").replica;
        let state = replica.documents.values_mut().next().unwrap();
        state.document = Document::from_snapshot(
            state.path.clone(),
            "local edit".to_owned(),
            Encoding::Utf8,
            Revision::new(2),
            true,
        );

        assert!(apply_inspection(state, &wire_document("remote"), true));

        assert_eq!(state.document.content_for_save(), "local edit");
        assert_eq!(state.acked, "before");
        assert_eq!(state.host_revision, ApiRevision::new(1));
        assert_eq!(state.external_change, ExternalChange::Modified);
    }

    #[test]
    fn inspection_replaces_a_clean_document_with_host_content() {
        let workspace = workspace("workspace-a");
        let mut replica = cached(&workspace, "before").replica;
        let state = replica.documents.values_mut().next().unwrap();

        assert!(apply_inspection(state, &wire_document("remote"), true));

        assert_eq!(state.document.content_for_save(), "remote");
        assert_eq!(state.acked, "remote");
        assert_eq!(state.host_revision, ApiRevision::new(2));
        assert_eq!(state.external_change, ExternalChange::Modified);
    }

    #[test]
    fn forced_inspection_replaces_unsent_local_changes() {
        let workspace = workspace("workspace-a");
        let mut replica = cached(&workspace, "before").replica;
        let state = replica.documents.values_mut().next().unwrap();
        state.document = Document::from_snapshot(
            state.path.clone(),
            "local edit".to_owned(),
            Encoding::Utf8,
            Revision::new(2),
            true,
        );

        assert!(apply_inspection(state, &wire_document("remote"), false));

        // reload / discard は Host を正本としてローカル編集を置き換える。
        assert_eq!(state.document.content_for_save(), "remote");
        assert_eq!(state.acked, "remote");
        assert_eq!(state.host_revision, ApiRevision::new(2));
        assert_eq!(state.external_change, ExternalChange::Modified);
    }

    #[test]
    fn cache_evicts_the_oldest_workspace_beyond_the_limit() {
        let first = workspace("workspace-a");
        let second = workspace("workspace-b");
        let third = workspace("workspace-c");
        let mut cache = WorkspaceCache::new(2);

        assert!(cache.insert(cached(&first, "a")).is_none());
        assert!(cache.insert(cached(&second, "b")).is_none());
        let evicted = cache
            .insert(cached(&third, "c"))
            .expect("上限超過を破棄する");

        assert_eq!(evicted.replica.workspace, first);
        assert_eq!(cache.len(), 2);
        assert!(
            cache.take(&first).is_none(),
            "破棄した Workspace は snapshot 復帰になる"
        );
        assert!(cache.take(&second).is_some());
        assert!(cache.take(&third).is_some());
    }

    #[test]
    fn rollback_can_return_a_restored_replica_without_losing_the_other_entries() {
        let first = workspace("workspace-a");
        let second = workspace("workspace-b");
        let mut cache = WorkspaceCache::new(4);

        cache.insert(cached(&first, "a"));
        let restored = cache.take(&second).unwrap_or_else(|| cached(&second, "b"));
        // rollback では切替先をそのまま戻す。
        cache.insert(restored);

        assert_eq!(cache.len(), 2);
        assert_eq!(
            cached_document(
                &cache.take(&second).unwrap().replica,
                &ApiDocumentId::try_new("document-1").unwrap()
            )
            .unwrap()
            .document
            .content_for_save(),
            "b"
        );
        assert!(cache.take(&first).is_some());
    }
}
