use crate::BackendClient;
use crate::client::Client;
use ::document::{
    Document, DocumentCache, DocumentConnection, DocumentError, DocumentSubscription, Encoding,
    ExternalChange, Revision,
};
use client_api as wire;
use editor_core::DocumentId;
use std::{
    collections::HashMap,
    ops::Range,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

/// Windowごとの購読へ変更通知を送る。満杯の購読は1件にまとめる。
#[derive(Default)]
struct DocumentChanges {
    senders: Mutex<Vec<mpsc::SyncSender<()>>>,
}

impl DocumentChanges {
    fn subscribe(&self) -> mpsc::Receiver<()> {
        let (sender, receiver) = mpsc::sync_channel(1);
        if let Ok(mut senders) = self.senders.lock() {
            senders.push(sender);
        }
        receiver
    }

    fn publish(&self) {
        let Ok(mut senders) = self.senders.lock() else {
            return;
        };
        senders.retain(|sender| {
            matches!(
                sender.try_send(()),
                Ok(()) | Err(mpsc::TrySendError::Full(()))
            )
        });
    }
}

struct HostDocumentSubscription {
    receiver: mpsc::Receiver<()>,
}

impl DocumentSubscription for HostDocumentSubscription {
    fn recv_timeout(&mut self, timeout: Duration) -> Result<bool, DocumentError> {
        match self.receiver.recv_timeout(timeout) {
            Ok(()) => Ok(true),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(false),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(DocumentError::io("Document購読が切断されました"))
            }
        }
    }
}

pub(super) struct HostDocuments {
    host: BackendClient,
    connections: Mutex<HashMap<wire::WorkspaceId, Client>>,
    resources: Mutex<HashMap<DocumentId, (Client, PathBuf)>>,
    changes: Arc<DocumentChanges>,
}

impl HostDocuments {
    pub(super) fn new(host: BackendClient) -> Self {
        Self {
            host,
            connections: Mutex::new(HashMap::new()),
            resources: Mutex::new(HashMap::new()),
            changes: Arc::new(DocumentChanges::default()),
        }
    }

    fn connection(&self, workspace: wire::WorkspaceId) -> Result<Client, DocumentError> {
        let mut connections = self
            .connections
            .lock()
            .map_err(|_| DocumentError::io("Document接続を取得できません"))?;
        if let Some(client) = connections.get(&workspace) {
            return Ok(client.clone());
        }
        let client = self
            .host
            .connect_workspace(workspace.clone())
            .map_err(|error| DocumentError::io(error.to_string()))?;
        connections.insert(workspace, client.clone());
        forward_document_events(client.clone(), Arc::downgrade(&self.changes));
        Ok(client)
    }
    fn connect(&self, root: &Path) -> Result<Client, DocumentError> {
        let workspace = self
            .host
            .register_workspace(root)
            .map_err(|error| DocumentError::io(error.to_string()))?;
        self.connection(workspace)
    }

    fn request(
        client: &Client,
        body: wire::RequestBody,
    ) -> Result<wire::ResponseBody, DocumentError> {
        match client
            .request(body)
            .map_err(|error| DocumentError::io(error.to_string()))?
        {
            wire::ResponseBody::Error(error) => {
                if error.code.as_str() == wire::REVISION_CONFLICT
                    || error.revision_conflict.is_some()
                {
                    Err(DocumentError::conflict(error.code.as_str()))
                } else if error.code.as_str() == "document_conflict" {
                    Err(DocumentError::conflict(
                        "Document が外部変更と競合しています",
                    ))
                } else {
                    Err(DocumentError::io(error.code.as_str()))
                }
            }
            response => Ok(response),
        }
    }

    fn resource(
        &self,
        id: &DocumentId,
    ) -> Result<(Client, PathBuf, wire::DocumentId), DocumentError> {
        let (client, root) = self
            .resources
            .lock()
            .map_err(|_| DocumentError::io("Document 接続 lock に失敗しました"))?
            .get(id)
            .cloned()
            .ok_or_else(|| DocumentError::io("Document 接続がありません"))?;
        let id = wire::DocumentId::try_new(id.as_str())
            .map_err(|_| DocumentError::io("Document ID が不正です"))?;
        Ok((client, root, id))
    }

    fn cache(
        &self,
        client: Client,
        root: PathBuf,
        snapshot: wire::DocumentSnapshot,
    ) -> Result<DocumentCache, DocumentError> {
        let id = DocumentId::new(snapshot.document_id.as_str());
        let owner = snapshot
            .owner
            .as_ref()
            .map(|owner| editor_core::SessionId::new(owner.as_str()));
        let is_owner = owner.as_ref().is_some_and(|owner| {
            client
                .session_id()
                .is_some_and(|session| session.as_str() == owner.as_str())
        });
        let path = snapshot.path.map(|path| root.join(path.as_str()));
        let document = Document::from_snapshot(
            path,
            snapshot.content,
            match snapshot.encoding {
                wire::DocumentEncoding::Utf8 => Encoding::Utf8,
                wire::DocumentEncoding::Utf8Bom => Encoding::Utf8Bom,
                wire::DocumentEncoding::Utf16Le => Encoding::Utf16Le,
                wire::DocumentEncoding::Utf16Be => Encoding::Utf16Be,
            },
            Revision::new(snapshot.revision.value()),
            snapshot.dirty,
        );
        self.resources
            .lock()
            .map_err(|_| DocumentError::io("Document 接続 lock に失敗しました"))?
            .insert(id.clone(), (client, root));
        Ok(DocumentCache {
            reused: true,
            id,
            document,
            can_undo: snapshot.can_undo,
            can_redo: snapshot.can_redo,
            external_change: match snapshot.external_change {
                wire::DocumentExternalChange::Unchanged => ExternalChange::Unchanged,
                wire::DocumentExternalChange::Modified => ExternalChange::Modified,
                wire::DocumentExternalChange::Deleted => ExternalChange::Deleted,
            },
            owner,
            is_owner,
        })
    }

    fn inspect(&self, id: &DocumentId) -> Result<DocumentCache, DocumentError> {
        let (client, root, document_id) = self.resource(id)?;
        let response = Self::request(
            &client,
            wire::RequestBody::DocumentInspect(wire::DocumentInspectRequest { document_id }),
        )?;
        let wire::ResponseBody::DocumentInspect(response) = response else {
            return Err(DocumentError::io("Document 応答が不正です"));
        };
        self.cache(client, root, response.document)
    }
}

/// Host の Document / ファイル変更 event を Window の再同期通知へ変換する。
///
/// 購読が切れたら再同期を促し、少し待って購読し直す。
/// `HostDocuments` が破棄されたら `Weak` の upgrade に失敗して終了する。
/// 購読イベントの受信待ちタイムアウト。破棄確認の間隔を兼ねる。
const DOCUMENT_EVENT_POLL: Duration = Duration::from_millis(250);

/// 転送するイベントの種類。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Relay {
    /// 再同期を促す。
    Notify,
    /// 再同期を促して購読を張り直す。
    Resync,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RelayPoll {
    Event(Relay),
    Idle,
    Closed,
}

/// Host イベントを再同期通知へ変換する。対象外は `None`。
fn relay_event(body: &wire::EventBody) -> Option<Relay> {
    match body {
        wire::EventBody::DocumentEdited { .. }
        | wire::EventBody::DocumentSaved { .. }
        | wire::EventBody::DocumentReplaced { .. }
        | wire::EventBody::DocumentClosed { .. }
        | wire::EventBody::DocumentOwner { .. }
        | wire::EventBody::WorkspaceChanged { .. }
        | wire::EventBody::FilesChanged { .. } => Some(Relay::Notify),
        wire::EventBody::SnapshotRequired { .. } => Some(Relay::Resync),
        _ => None,
    }
}

/// 購読を開始した直後に再同期を促してから、イベントを転送する。
fn relay_subscription(
    changes: &std::sync::Weak<DocumentChanges>,
    mut next: impl FnMut() -> RelayPoll,
) {
    // 購読開始前の変更を回収する。
    publish_changes(changes);
    relay_events(changes, &mut next);
}

/// 購読が続く間、イベントを通知へ変換する。
///
/// 受信待ち中も `Weak` の破棄を確認できるよう、呼び出し側は timeout 付きで待つ。
fn relay_events(changes: &std::sync::Weak<DocumentChanges>, next: &mut impl FnMut() -> RelayPoll) {
    loop {
        if changes.upgrade().is_none() {
            return;
        }
        match next() {
            RelayPoll::Event(Relay::Notify) => publish_changes(changes),
            RelayPoll::Event(Relay::Resync) => {
                publish_changes(changes);
                return;
            }
            RelayPoll::Idle => {}
            RelayPoll::Closed => return,
        }
    }
}

/// 購読区間の受信処理。`Open` は購読開始、`None` は購読失敗を表す。
type RelayPoller = Box<dyn FnMut() -> RelayPoll>;

/// 1 回の購読区間を処理する。
///
/// 購読に成功したら開始直後に再同期を促し、区間が終わったら再度促す。
fn relay_once(
    changes: &std::sync::Weak<DocumentChanges>,
    open: &mut impl FnMut() -> Option<RelayPoller>,
) {
    if let Some(mut next) = open() {
        relay_subscription(changes, &mut next);
    }
    publish_changes(changes);
}

fn forward_document_events(client: Client, changes: std::sync::Weak<DocumentChanges>) {
    std::thread::spawn(move || {
        loop {
            // 受信待ちの間に強参照を保持しない。破棄済みなら即座に終了する。
            if changes.upgrade().is_none() {
                return;
            }
            relay_once(&changes, &mut || {
                client.subscribe().ok().map(|receiver| {
                    Box::new(move || match receiver.recv_timeout(DOCUMENT_EVENT_POLL) {
                        Ok(event) => match relay_event(&event.body) {
                            Some(relay) => RelayPoll::Event(relay),
                            None => RelayPoll::Idle,
                        },
                        Err(mpsc::RecvTimeoutError::Timeout) => RelayPoll::Idle,
                        Err(mpsc::RecvTimeoutError::Disconnected) => RelayPoll::Closed,
                    }) as RelayPoller
                })
            });
            if changes.upgrade().is_none() {
                return;
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });
}

/// 破棄済みなら何もしない。強参照は通知の間だけ保持する。
fn publish_changes(changes: &std::sync::Weak<DocumentChanges>) {
    if let Some(changes) = changes.upgrade() {
        changes.publish();
    }
}

impl DocumentConnection for HostDocuments {
    fn fork(&self) -> std::sync::Arc<dyn DocumentConnection> {
        std::sync::Arc::new(Self::new(self.host.clone()))
    }
    fn workspace(&self, root: &Path) -> Result<editor_core::WorkspaceId, DocumentError> {
        self.host
            .register_workspace(root)
            .map(|id| editor_core::WorkspaceId::new(id.as_str()))
            .map_err(|error| DocumentError::io(error.to_string()))
    }
    fn create(&self, root: Option<&Path>) -> Result<DocumentCache, DocumentError> {
        let root = match root {
            Some(root) => root.to_owned(),
            None => self
                .host
                .default_root()
                .map_err(|error| DocumentError::io(error.to_string()))?,
        };
        let root = self
            .host
            .normalize_path(&root)
            .map_err(|error| DocumentError::io(error.to_string()))?;
        let client = self
            .connect(&root)
            .map_err(|error| DocumentError::io(error.to_string()))?;
        let response = Self::request(
            &client,
            wire::RequestBody::DocumentUntitled(wire::DocumentUntitledRequest {
                workspace_id: client.workspace_id().clone(),
            }),
        )?;
        let wire::ResponseBody::DocumentUntitled(response) = response else {
            return Err(DocumentError::io("Document 応答が不正です"));
        };
        let mut cache = self.cache(client, root, response.document)?;
        cache.reused = false;
        Ok(cache)
    }

    fn open(&self, path: &Path) -> Result<DocumentCache, DocumentError> {
        let path = self
            .host
            .normalize_file(path)
            .map_err(|error| DocumentError::io(error.to_string()))?;
        let parent = path
            .parent()
            .ok_or_else(|| DocumentError::io("親ディレクトリがありません"))?;
        let (root, _) = self
            .host
            .directory(parent)
            .map_err(|error| DocumentError::io(error.to_string()))?;
        let client = self.connect(&root)?;
        let relative = relative(&root, &path)?;
        let response = Self::request(
            &client,
            wire::RequestBody::DocumentOpen(wire::DocumentOpenRequest {
                workspace_id: client.workspace_id().clone(),
                path: relative,
            }),
        )?;
        let wire::ResponseBody::DocumentOpen(response) = response else {
            return Err(DocumentError::io("Document 応答が不正です"));
        };
        let mut cache = self.cache(client, root, response.document)?;
        cache.reused = response.reused;
        Ok(cache)
    }

    fn attach(&self, id: &DocumentId) -> Result<DocumentCache, DocumentError> {
        if self.resource(id).is_ok()
            && let Ok(cache) = self.inspect(id)
        {
            return Ok(cache);
        }
        let mut roots = self
            .host
            .roots()
            .map_err(|error| DocumentError::io(error.to_string()))?;
        roots.sort_by(|left, right| left.0.cmp(&right.0));
        for (root, workspace_id) in roots {
            let Ok(client) = self.connection(workspace_id) else {
                continue;
            };
            let document_id = wire::DocumentId::try_new(id.as_str())
                .map_err(|_| DocumentError::io("Document ID が不正です"))?;
            if let Ok(wire::ResponseBody::DocumentInspect(response)) = Self::request(
                &client,
                wire::RequestBody::DocumentInspect(wire::DocumentInspectRequest { document_id }),
            ) {
                return self.cache(client, root, response.document);
            }
        }
        Err(DocumentError::io("Document がありません"))
    }

    fn claim(&self, id: &DocumentId) -> Result<DocumentCache, DocumentError> {
        let (client, _, document_id) = self.resource(id)?;
        Self::request(
            &client,
            wire::RequestBody::DocumentClaim(wire::DocumentClaimRequest { document_id }),
        )?;
        self.inspect(id)
    }

    fn release(&self, id: &DocumentId) -> Result<DocumentCache, DocumentError> {
        let (client, _, document_id) = self.resource(id)?;
        Self::request(
            &client,
            wire::RequestBody::DocumentRelease(wire::DocumentReleaseRequest { document_id }),
        )?;
        self.inspect(id)
    }

    fn edit(
        &self,
        id: &DocumentId,
        revision: Revision,
        range: Range<usize>,
        replacement: &str,
    ) -> Result<DocumentCache, DocumentError> {
        let (client, _, document_id) = self.resource(id)?;
        let edit =
            wire::DocumentTextEdit::try_new(range.start as u64, range.end as u64, replacement)
                .map_err(|error| DocumentError::io(error.to_string()))?;
        let transaction = wire::DocumentTransaction::try_new(vec![edit])
            .map_err(|error| DocumentError::io(error.to_string()))?;
        Self::request(
            &client,
            wire::RequestBody::DocumentEdit(wire::DocumentEditRequest {
                document_id,
                base_revision: wire::Revision::new(revision.number()),
                transaction,
            }),
        )?;
        self.inspect(id)
    }

    fn history(
        &self,
        id: &DocumentId,
        revision: Revision,
        undo: bool,
    ) -> Result<DocumentCache, DocumentError> {
        let (client, _, document_id) = self.resource(id)?;
        let request = wire::DocumentHistoryRequest {
            document_id,
            base_revision: wire::Revision::new(revision.number()),
        };
        Self::request(
            &client,
            if undo {
                wire::RequestBody::DocumentUndo(request)
            } else {
                wire::RequestBody::DocumentRedo(request)
            },
        )?;
        self.inspect(id)
    }

    fn save(
        &self,
        id: &DocumentId,
        revision: Revision,
        path: Option<PathBuf>,
    ) -> Result<DocumentCache, DocumentError> {
        let (client, root, document_id) = self.resource(id)?;
        let base_revision = wire::Revision::new(revision.number());
        let request = if let Some(path) = path {
            let path = self
                .host
                .normalize_file(&path)
                .map_err(|error| DocumentError::io(error.to_string()))?;
            if !path.starts_with(&root) {
                let parent = path
                    .parent()
                    .ok_or_else(|| DocumentError::io("親ディレクトリがありません"))?;
                let (destination, _) = self
                    .host
                    .connect_directory(parent)
                    .map_err(|error| DocumentError::io(error.to_string()))?;
                let destination_root = self
                    .host
                    .root_for(destination.workspace_id())
                    .map_err(|error| DocumentError::io(error.to_string()))?;
                self.host
                    .relocate_document(
                        client.workspace_id().clone(),
                        destination.workspace_id().clone(),
                        document_id.clone(),
                        base_revision,
                        relative(&destination_root, &path)?,
                    )
                    .map_err(|error| match error {
                        backend_state::BackendServiceError::State(
                            backend_state::BackendStateError::Document(error),
                        ) => error,
                        backend_state::BackendServiceError::State(
                            backend_state::BackendStateError::RevisionConflict(_),
                        ) => DocumentError::conflict("Document revision が競合しています"),
                        backend_state::BackendServiceError::Rejected(problem)
                            if problem.revision_conflict.is_some()
                                || problem.code.as_str() == "document_conflict" =>
                        {
                            DocumentError::conflict("保存先またはDocument revisionが競合しています")
                        }
                        error => DocumentError::io(error.to_string()),
                    })?;
                return self.attach(id);
            }
            wire::RequestBody::DocumentSaveAs(wire::DocumentSaveAsRequest {
                document_id,
                base_revision,
                path: relative(&root, &path)?,
            })
        } else {
            wire::RequestBody::DocumentSave(wire::DocumentSaveRequest {
                document_id,
                base_revision,
            })
        };
        Self::request(&client, request)?;
        self.inspect(id)
    }

    fn reload(&self, id: &DocumentId, revision: Revision) -> Result<DocumentCache, DocumentError> {
        let (client, _, document_id) = self.resource(id)?;
        Self::request(
            &client,
            wire::RequestBody::DocumentReload(wire::DocumentHistoryRequest {
                document_id,
                base_revision: wire::Revision::new(revision.number()),
            }),
        )?;
        self.inspect(id)
    }

    fn discard(&self, id: &DocumentId, revision: Revision) -> Result<DocumentCache, DocumentError> {
        let (client, root, document_id) = self.resource(id)?;
        let response = Self::request(
            &client,
            wire::RequestBody::DocumentDiscard(wire::DocumentDiscardRequest {
                document_id,
                base_revision: wire::Revision::new(revision.number()),
            }),
        )?;
        let wire::ResponseBody::DocumentDiscard(response) = response else {
            return Err(DocumentError::io("Document 応答が不正です"));
        };
        self.cache(client, root, response.document)
    }

    fn close(&self, id: &DocumentId) -> Result<(), DocumentError> {
        self.attach(id)?;
        let (client, _, document_id) = self.resource(id)?;
        Self::request(
            &client,
            wire::RequestBody::DocumentClose(wire::DocumentCloseRequest { document_id }),
        )?;
        self.resources
            .lock()
            .map_err(|_| DocumentError::io("Document 接続 lock に失敗しました"))?
            .remove(id);
        Ok(())
    }

    fn subscribe(&self) -> Option<Box<dyn DocumentSubscription>> {
        Some(Box::new(HostDocumentSubscription {
            receiver: self.changes.subscribe(),
        }))
    }
}

fn relative(root: &Path, path: &Path) -> Result<wire::WorkspaceRelativePath, DocumentError> {
    wire::WorkspaceRelativePath::from_path(
        path.strip_prefix(root)
            .map_err(|_| DocumentError::io("保存先は Workspace 内に指定してください"))?,
    )
    .map_err(|error| DocumentError::io(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_notifies_all_subscribers_and_coalesces_when_full() {
        let changes = DocumentChanges::default();
        let first = changes.subscribe();
        let second = changes.subscribe();
        changes.publish();
        assert!(first.recv_timeout(Duration::from_millis(100)).is_ok());
        assert!(second.recv_timeout(Duration::from_millis(100)).is_ok());
        // 満杯の購読は 1 件にまとめ、切断扱いにしない。
        changes.publish();
        changes.publish();
        assert!(first.recv_timeout(Duration::from_millis(100)).is_ok());
        changes.publish();
        assert!(first.recv_timeout(Duration::from_millis(100)).is_ok());
    }

    #[test]
    fn publish_via_weak_does_not_retain_the_sink() {
        let changes = std::sync::Arc::new(DocumentChanges::default());
        let weak = std::sync::Arc::downgrade(&changes);
        publish_changes(&weak);
        assert_eq!(std::sync::Arc::strong_count(&changes), 1);
        drop(changes);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn dropped_subscribers_are_removed_on_publish() {
        let changes = DocumentChanges::default();
        let receiver = changes.subscribe();
        drop(receiver);
        changes.publish();
        assert!(changes.senders.lock().unwrap().is_empty());
    }

    #[test]
    fn subscription_start_requests_a_snapshot() {
        let changes = std::sync::Arc::new(DocumentChanges::default());
        let receiver = changes.subscribe();
        let weak = std::sync::Arc::downgrade(&changes);
        relay_subscription(&weak, || RelayPoll::Closed);
        assert!(
            receiver.recv_timeout(Duration::from_millis(100)).is_ok(),
            "購読開始時に再同期を促す"
        );
    }

    #[test]
    fn resync_notifies_and_stops() {
        let changes = std::sync::Arc::new(DocumentChanges::default());
        let receiver = changes.subscribe();
        let weak = std::sync::Arc::downgrade(&changes);
        let mut steps = vec![RelayPoll::Event(Relay::Resync)].into_iter();
        relay_events(&weak, &mut || {
            steps.next().unwrap_or_else(|| panic!("resync で終了する"))
        });
        assert!(
            receiver.recv_timeout(Duration::from_millis(100)).is_ok(),
            "resync でも再同期を促す"
        );
        assert!(receiver.try_recv().is_err(), "通知は 1 回にまとめる");
    }

    #[test]
    fn resync_resubscribes_and_notifies_after_success() {
        let changes = std::sync::Arc::new(DocumentChanges::default());
        let weak = std::sync::Arc::downgrade(&changes);
        let opens = std::cell::Cell::new(0);
        let mut open = || {
            opens.set(opens.get() + 1);
            if opens.get() == 1 {
                let mut steps = vec![RelayPoll::Event(Relay::Resync)].into_iter();
                Some(
                    Box::new(move || steps.next().unwrap_or_else(|| panic!("resync で終了する")))
                        as RelayPoller,
                )
            } else {
                // 再購読。購読直後の通知を受け取ってから終了する。
                let receiver = changes.subscribe();
                Some(Box::new(move || {
                    assert!(receiver.try_recv().is_ok(), "再購読成功直後に再同期を促す");
                    RelayPoll::Closed
                }) as RelayPoller)
            }
        };
        // 1 回目は resync で購読区間が終わる。
        relay_once(&weak, &mut open);
        assert_eq!(opens.get(), 1);
        // 2 回目は再購読し、購読中の通知を検証する。
        relay_once(&weak, &mut open);
        assert_eq!(opens.get(), 2, "resync 後に購読し直す");
    }

    #[test]
    fn relay_loop_stops_when_the_owner_is_dropped_while_waiting() {
        let changes = std::sync::Arc::new(DocumentChanges::default());
        let weak = std::sync::Arc::downgrade(&changes);
        let mut owner = Some(changes);
        let calls = std::cell::Cell::new(0);
        relay_events(&weak, &mut || {
            calls.set(calls.get() + 1);
            if calls.get() == 1 {
                // 受信待ち中に相当するタイミングで破棄する。
                drop(owner.take());
            }
            RelayPoll::Idle
        });
        // 受信待ち中に破棄したら、次の確認でループを抜ける（追加の待機をしない）。
        assert_eq!(calls.get(), 1, "破棄後の再待機は行わない");
        assert!(weak.upgrade().is_none());
    }
}
