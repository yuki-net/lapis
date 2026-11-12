//! Workspace の view state（開いているタブとアクティブ文書）の Session 共有を、
//! 受信と UI が読む watch に分離して扱う。
//!
//! 正本は Host にあり、購読した Client は snapshot の後に最新の view 全体を受け取る。
//! 差分やマージは行わず、Host が受理した値だけを watch へ載せる。event は接続中の
//! 全 Session に配られるため、購読していない間と、権限で内容が伏せられた event は
//! 適用しない。
//!
//! 受信手段は platform で異なる（Desktop は背景 thread、Web は非同期の event pump）。
//! 適用可否と通知順の判断はここで共有し、受信だけを各 platform が持つ。

use std::collections::VecDeque;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc,
};
use std::time::Duration;

use editor_core::WorkspaceId;
use workbench::{
    SessionViewConnection, SessionViewEvent, SessionViewEvents, SessionViewSnapshot, WorkspaceView,
};

/// 別の Client が共有した view state を UI へ渡すための共有スロット。
///
/// `sequence` はこの Client 内での通知順で、未適用の判定に使う。Host の `revision` は
/// Workspace を切り替えると巻き戻り得るため、適用対象は Workspace と組で持つ。
#[derive(Clone, Default)]
pub struct SessionViewWatch {
    sequence: Arc<AtomicU64>,
    slot: Arc<Mutex<Option<(WorkspaceId, u64, WorkspaceView)>>>,
    listeners: Arc<Mutex<Vec<mpsc::Sender<u64>>>>,
    /// snapshot まで受け取った Workspace。参加が済む前に自分の値を送ると、
    /// 共有済みの状態を上書きしてしまうため、送信可否の判断に使う。
    synced: Arc<Mutex<Option<WorkspaceId>>>,
    /// 接続の切断などで、以降の変更を取りこぼした可能性。
    stale: Arc<AtomicBool>,
}

impl SessionViewWatch {
    pub fn new() -> Self {
        Self::default()
    }

    /// この Client 内で最後に載った順序。
    pub fn sequence(&self) -> u64 {
        self.sequence.load(Ordering::Acquire)
    }

    /// 新しい通知を受け取る。UI は blocking 受信を worker thread へ任せる。
    pub fn subscribe(&self) -> mpsc::Receiver<u64> {
        let (sender, receiver) = mpsc::channel();
        if let Ok(mut listeners) = self.listeners.lock() {
            listeners.push(sender);
        }
        receiver
    }

    /// 共有状態が最新でない可能性があるか。表示は最後に受信した値のまま。
    pub fn is_stale(&self) -> bool {
        self.stale.load(Ordering::Acquire)
    }

    /// 切断や再同期の失敗で、以降の変更を取りこぼした可能性を記録する。
    /// 次の snapshot・event まで解除しない。
    pub fn mark_stale(&self) {
        if !self.stale.swap(true, Ordering::AcqRel) {
            self.notify();
        }
    }

    /// まだ誰も共有していない snapshot（`view` が null）でも、参加は完了している。
    pub fn mark_synced(&self, workspace: WorkspaceId) {
        if let Ok(mut synced) = self.synced.lock() {
            *synced = Some(workspace);
        }
        if self.stale.swap(false, Ordering::AcqRel) {
            self.notify();
        }
    }

    /// 購読を解除したので、参加の完了を取り消す。再購読するまで送信させない。
    pub fn mark_unsynced(&self) {
        if let Ok(mut synced) = self.synced.lock() {
            *synced = None;
        }
    }

    /// この Workspace の snapshot を一度でも受け取ったか。
    pub fn is_synced(&self, workspace: &WorkspaceId) -> bool {
        self.synced
            .lock()
            .is_ok_and(|synced| synced.as_ref() == Some(workspace))
    }

    fn notify(&self) {
        let sequence = self.sequence();
        if let Ok(mut listeners) = self.listeners.lock() {
            listeners.retain(|listener| listener.send(sequence).is_ok());
        }
    }

    pub(crate) fn publish(&self, workspace: WorkspaceId, revision: u64, view: WorkspaceView) {
        let Ok(mut slot) = self.slot.lock() else {
            return;
        };
        *slot = Some((workspace.clone(), revision, view));
        drop(slot);
        self.mark_synced(workspace);
        // slot の更新後に順序を進める。順序を見た読み手は必ず新しい slot を読める。
        let sequence = self.sequence.fetch_add(1, Ordering::AcqRel) + 1;
        if let Ok(mut listeners) = self.listeners.lock() {
            listeners.retain(|listener| listener.send(sequence).is_ok());
        }
    }

    /// まだ適用していない view を `(sequence, revision, view)` で返す。
    ///
    /// 別の Workspace の値は適用対象外として捨てない。Workspace を戻したときに
    /// 最新値として使えるよう slot には残し、ここでは `None` を返す。
    pub fn take_if_changed(
        &self,
        workspace: &WorkspaceId,
        last: u64,
    ) -> Option<(u64, u64, WorkspaceView)> {
        let sequence = self.sequence();
        if sequence <= last {
            return None;
        }
        let slot = self.slot.lock().ok()?;
        let (owner, revision, view) = slot.as_ref()?;
        (owner == workspace).then(|| (sequence, *revision, view.clone()))
    }
}

/// Workspace 単位の購読で共有された view の適用判定。
///
/// `revision` は Host が受理した順で単調に増えるが、Workspace を切り替えると
/// 巻き戻り得るため、Workspace と組で保持する。
pub struct SessionViewCursor {
    workspace: WorkspaceId,
    /// 適用済み、または自分が受理させた revision。これ以下は再適用しない。
    applied: AtomicU64,
    /// 自分の送信が受理された revision。同じ revision の event は自分の更新の
    /// 巡回なので内容を再適用しない。
    local_revisions: Mutex<VecDeque<u64>>,
    /// 送信したが、まだ応答を受け取っていない自分の view。Host は応答より先に
    /// event を配り得るため、その event を revision ではなく値で照合する。
    /// 応答を受けた時点で revision の記録へ移す。
    pending: Mutex<VecDeque<WorkspaceView>>,
}

/// 自分の送信として覚えておく件数の上限。応答待ちが積み重なる目安。
const MAX_LOCAL_ENTRIES: usize = 16;

impl SessionViewCursor {
    pub fn new(workspace: WorkspaceId, revision: u64) -> Self {
        Self {
            workspace,
            applied: AtomicU64::new(revision),
            local_revisions: Mutex::new(VecDeque::new()),
            pending: Mutex::new(VecDeque::new()),
        }
    }

    pub fn workspace(&self) -> &WorkspaceId {
        &self.workspace
    }

    pub fn applied(&self) -> u64 {
        self.applied.load(Ordering::Acquire)
    }

    /// これから送る自分の view を記録する。送信より前に呼ぶ。応答より先に自分の
    /// 更新が event として届いても、値を照合して再適用しないため。
    pub fn begin_local(&self, view: &WorkspaceView) {
        if let Ok(mut pending) = self.pending.lock() {
            while pending.len() >= MAX_LOCAL_ENTRIES {
                pending.pop_front();
            }
            pending.push_back(view.clone());
        }
    }

    /// 直前の送信が失敗したため、その記録だけを捨てる。
    pub fn abort_local(&self) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.pop_back();
        }
    }

    /// 自分の送信が受理された revision を、適用済みとして扱う。
    /// 応答の時点で pending から外し、同じ revision の event は再適用しない。
    pub fn accept_local(&self, revision: u64) {
        self.applied.fetch_max(revision, Ordering::AcqRel);
        self.abort_local();
        if let Ok(mut revisions) = self.local_revisions.lock() {
            while revisions.len() >= MAX_LOCAL_ENTRIES {
                revisions.pop_front();
            }
            if !revisions.contains(&revision) {
                revisions.push_back(revision);
            }
        }
    }

    /// event を適用対象として受け入れる。重複と巻き戻りは `None` で弾く。
    /// `view` が `None` の event は権限で伏せられているため内容は適用しない。
    /// 自分の送信が戻ってきた event も内容を再適用しない。
    pub fn accept_event(
        &self,
        revision: u64,
        view: Option<WorkspaceView>,
    ) -> Option<(u64, WorkspaceView)> {
        if revision <= self.applied.load(Ordering::Acquire) {
            return None;
        }
        self.applied.store(revision, Ordering::Release);
        let view = view?;
        if self.is_local(revision, &view) {
            return None;
        }
        Some((revision, view))
    }

    /// 自分の更新か。応答待ちの値と一致すれば自分の更新とみなし、記録を消費する。
    /// 応答済みの revision と一致する場合も自分の更新。
    fn is_local(&self, revision: u64, view: &WorkspaceView) -> bool {
        let accepted = self.local_revisions.lock().is_ok_and(|mut revisions| {
            match revisions.iter().position(|&own| own == revision) {
                Some(index) => {
                    revisions.remove(index);
                    true
                }
                None => false,
            }
        });
        if accepted {
            return true;
        }
        self.pending.lock().is_ok_and(|mut pending| {
            match pending.iter().position(|own| own == view) {
                Some(index) => {
                    pending.remove(index);
                    true
                }
                None => false,
            }
        })
    }
}

/// Host の view state 購読を、platform を問わず同じ規則で適用するレプリカ。
///
/// Web は接続の event pump から直接 event を受け、snapshot は要求の応答で得る。
/// 適用可否と通知順の判定は `SessionViewCursor` に任せ、ここでは購読の開始・
/// 解除と watch への反映だけを行う。
pub struct SessionViewReplica {
    watch: SessionViewWatch,
    cursor: Option<SessionViewCursor>,
}

impl SessionViewReplica {
    pub fn new(watch: SessionViewWatch) -> Self {
        Self {
            watch,
            cursor: None,
        }
    }

    pub fn watch(&self) -> &SessionViewWatch {
        &self.watch
    }

    pub fn workspace(&self) -> Option<&WorkspaceId> {
        self.cursor.as_ref().map(SessionViewCursor::workspace)
    }

    /// 購読応答の snapshot で参加を開始する。`view` が null でも参加は完了する。
    /// 適用済みより古い snapshot は、再接続前の応答が遅れて届いた場合などに
    /// 表示を巻き戻すため適用しない。
    pub fn subscribe_snapshot(
        &mut self,
        workspace: WorkspaceId,
        revision: u64,
        view: Option<WorkspaceView>,
    ) {
        if let Some(cursor) = self.cursor.as_ref()
            && cursor.workspace() == &workspace
            && revision < cursor.applied()
        {
            return;
        }
        if self.workspace() != Some(&workspace) {
            self.cursor = Some(SessionViewCursor::new(workspace.clone(), revision));
        } else if let Some(cursor) = &self.cursor {
            cursor.accept_local(revision);
        }
        if let Some(view) = view {
            self.watch.publish(workspace, revision, view);
        } else {
            self.watch.mark_synced(workspace);
        }
    }

    /// `workspace.view_changed` を適用する。`view` が null の event は内容を持たない。
    pub fn apply_event(
        &mut self,
        workspace: &WorkspaceId,
        revision: u64,
        view: Option<WorkspaceView>,
    ) {
        let Some(cursor) = self.cursor.as_ref() else {
            // 購読前の event は snapshot で追いつくため適用しない。
            return;
        };
        if cursor.workspace() != workspace {
            return;
        }
        if let Some((revision, view)) = cursor.accept_event(revision, view) {
            self.watch.publish(workspace.clone(), revision, view);
        }
    }

    /// 送信する自分の view を記録する。応答より先に自分の更新が event として
    /// 届いても再適用しないため、送信より前に呼ぶ。
    pub fn begin_local(&self, view: &WorkspaceView) {
        if let Some(cursor) = &self.cursor {
            cursor.begin_local(view);
        }
    }

    /// 送信が失敗したとき、記録した自分の view を捨てる。
    pub fn abort_local(&self) {
        if let Some(cursor) = &self.cursor {
            cursor.abort_local();
        }
    }

    /// 自分の送信が受理された revision を、適用済みとして扱う。
    pub fn accept_local(&self, revision: u64) {
        if let Some(cursor) = &self.cursor {
            cursor.accept_local(revision);
        }
    }

    /// 購読を解除した状態へ戻す。表示は古い可能性があるものとして残す。
    pub fn reset(&mut self) {
        self.cursor = None;
        // 再購読の snapshot を確認するまで送信させない。
        self.watch.mark_unsynced();
        self.watch.mark_stale();
    }
}

/// Workspace 単位の購読 session。Workspace を閉じるまで 1 つだけ持つ。
pub struct SessionViewSession {
    inner: Arc<Inner>,
}

struct Inner {
    connection: Arc<dyn SessionViewConnection>,
    workspace: WorkspaceId,
    watch: SessionViewWatch,
    stopped: AtomicBool,
    /// 適用済み、または自分が受理させた revision。これ以下は再適用しない。
    applied: AtomicU64,
    /// 自分の更新の受理 revision を確定するまで、event の適用を待たせる。
    ///
    /// Host は自分の更新の echo を、更新の応答より先に配ることがある。待たせないと
    /// echo が先に適用され、自分の watch に自分の更新が戻って再送ループになる。
    update_gate: Mutex<()>,
}

impl SessionViewSession {
    /// 購読を開始し、snapshot を watch へ載せる。
    ///
    /// 先に event の受け口を作ってから snapshot を取ることで、購読開始と
    /// snapshot 取得の間の更新を取りこぼさない。
    pub fn start(
        connection: Arc<dyn SessionViewConnection>,
        workspace: WorkspaceId,
        watch: SessionViewWatch,
    ) -> Result<Self, String> {
        let events = connection.events(&workspace)?;
        let snapshot = connection.subscribe(&workspace)?;
        let inner = Arc::new(Inner {
            connection,
            workspace,
            watch,
            stopped: AtomicBool::new(false),
            applied: AtomicU64::new(snapshot.revision),
            update_gate: Mutex::new(()),
        });
        publish_snapshot(&inner, &snapshot);
        let worker = inner.clone();
        std::thread::spawn(move || run_events(worker, events));
        Ok(Self { inner })
    }

    pub fn workspace(&self) -> &WorkspaceId {
        &self.inner.workspace
    }

    /// 自分の view を送る。受理した revision を適用済みとして扱い、
    /// 同じ値が event で返ってきても再度適用しない。
    pub fn publish(&self, view: WorkspaceView) -> Result<u64, String> {
        // echo の適用と受理 revision の確定を排他し、自分の更新が自分の watch へ
        // 戻らないようにする。
        let _gate = lock(&self.inner.update_gate);
        let revision = self.inner.connection.update(&self.inner.workspace, view)?;
        self.inner.applied.fetch_max(revision, Ordering::AcqRel);
        Ok(revision)
    }

    /// スリープ復帰などで snapshot を取り直して再同期する。
    pub fn resync(&self) -> Result<u64, String> {
        let snapshot = self.inner.connection.subscribe(&self.inner.workspace)?;
        self.inner
            .applied
            .fetch_max(snapshot.revision, Ordering::AcqRel);
        publish_snapshot(&self.inner, &snapshot);
        Ok(snapshot.revision)
    }

    /// 購読を終了する。以後の delta は Host から届かない。
    pub fn close(&self) {
        if self.inner.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Err(error) = self.inner.connection.unsubscribe(&self.inner.workspace) {
            eprintln!("Workspace view の購読解除に失敗しました: {error}");
        }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
    }
}

/// 毒された lock でも中身を読めるようにし、view の共有を止めない。
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn publish_snapshot(inner: &Inner, snapshot: &SessionViewSnapshot) {
    if let Some(view) = &snapshot.view {
        inner
            .watch
            .publish(inner.workspace.clone(), snapshot.revision, view.clone());
    }
}

fn run_events(inner: Arc<Inner>, mut events: Box<dyn SessionViewEvents>) {
    let mut backoff = Duration::from_secs(1);
    loop {
        let broken = loop {
            if inner.stopped.load(Ordering::Acquire) {
                return;
            }
            // 購読解除を速やかに反映するため、待機は短く区切る。event は購読中だけ
            // 届くが、Host 側の解除とこの thread の停止は非同期にずれる。
            match events.next(Duration::from_secs(1)) {
                Ok(Some(event)) => {
                    // close 後は、配信境界の遅れで届いた event も適用しない。
                    if !inner.stopped.load(Ordering::Acquire) {
                        inner.apply_event(event);
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    if !inner.stopped.load(Ordering::Acquire) {
                        eprintln!("Workspace view の購読が切断されました: {error}");
                    }
                    break true;
                }
            }
        };
        if !broken {
            continue;
        }
        // 切断後は snapshot で再同期し、購読を張り直す。待機はこの thread 内だけ。
        match inner.connection.events(&inner.workspace) {
            Ok(replacement) => {
                events = replacement;
                backoff = Duration::from_secs(1);
                if let Err(error) = inner.resubscribe() {
                    eprintln!("Workspace view を再取得できません: {error}");
                }
            }
            Err(error) => {
                eprintln!("Workspace view の購読を再開できません: {error}");
                if !sleep_unless_stopped(&inner, backoff) {
                    return;
                }
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
        }
    }
}

impl Inner {
    fn apply_event(&self, event: SessionViewEvent) {
        // 自分の更新の echo は受理 revision の確定より先に届き得る。確定前に適用すると
        // 自分の watch へ自分の更新が戻るため、確定まで待たせる。
        let _gate = lock(&self.update_gate);
        // gate を待つ間に close が完了していれば、終了済みの購読から watch を更新しない。
        if self.stopped.load(Ordering::Acquire) {
            return;
        }
        if event.revision <= self.applied.load(Ordering::Acquire) {
            return;
        }
        self.applied.store(event.revision, Ordering::Release);
        // 権限で伏せられた event は内容を持たないため適用しない。
        if let Some(view) = event.view {
            self.watch
                .publish(self.workspace.clone(), event.revision, view);
        }
    }

    fn resubscribe(&self) -> Result<(), String> {
        let snapshot = self.connection.subscribe(&self.workspace)?;
        self.applied.fetch_max(snapshot.revision, Ordering::AcqRel);
        publish_snapshot(self, &snapshot);
        Ok(())
    }
}

fn sleep_unless_stopped(inner: &Inner, duration: Duration) -> bool {
    let step = Duration::from_millis(100);
    let mut slept = Duration::ZERO;
    while slept < duration {
        if inner.stopped.load(Ordering::Acquire) {
            return false;
        }
        std::thread::sleep(step);
        slept += step;
    }
    !inner.stopped.load(Ordering::Acquire)
}

/// 現在の Workspace に対する購読 session を所有し、開き直しと公開を仲介する。
pub struct SessionViewSync {
    connection: Arc<dyn SessionViewConnection>,
    watch: SessionViewWatch,
    session: Mutex<Option<SessionViewSession>>,
}
impl SessionViewSync {
    pub fn new(connection: Arc<dyn SessionViewConnection>, watch: SessionViewWatch) -> Self {
        Self {
            connection,
            watch,
            session: Mutex::new(None),
        }
    }

    pub fn watch(&self) -> &SessionViewWatch {
        &self.watch
    }

    /// Workspace を開いたときに購読する。同じ Workspace なら何もしない。
    pub fn open(&self, workspace: &WorkspaceId) -> Result<(), String> {
        let mut session = self
            .session
            .lock()
            .map_err(|_| "Workspace view の lock に失敗しました".to_owned())?;
        if session
            .as_ref()
            .is_some_and(|current| current.workspace() == workspace)
        {
            return Ok(());
        }
        if let Some(current) = session.take() {
            current.close();
        }
        *session = Some(SessionViewSession::start(
            self.connection.clone(),
            workspace.clone(),
            self.watch.clone(),
        )?);
        Ok(())
    }

    /// Workspace を閉じたときや終了時に購読を解除する。
    pub fn close(&self) {
        let Ok(mut session) = self.session.lock() else {
            return;
        };
        if let Some(current) = session.take() {
            current.close();
        }
    }

    pub fn publish(&self, view: WorkspaceView) -> Result<u64, String> {
        let session = self
            .session
            .lock()
            .map_err(|_| "Workspace view の lock に失敗しました".to_owned())?;
        session
            .as_ref()
            .ok_or_else(|| "Workspace view を購読していません".to_owned())?
            .publish(view)
    }

    pub fn resync(&self) -> Result<u64, String> {
        let session = self
            .session
            .lock()
            .map_err(|_| "Workspace view の lock に失敗しました".to_owned())?;
        session
            .as_ref()
            .ok_or_else(|| "Workspace view を購読していません".to_owned())?
            .resync()
    }
}

#[cfg(test)]
mod tests {
    include!("session_view_tests.rs");
}

#[cfg(test)]
mod replica_tests {
    use super::*;

    fn view(active: &str) -> WorkspaceView {
        WorkspaceView {
            open_documents: vec![workbench::WorkspaceDocumentView {
                document_id: Some(editor_core::DocumentId::new(active)),
                ..Default::default()
            }],
            active_path: None,
            active_document_id: Some(editor_core::DocumentId::new(active)),
        }
    }

    fn active(view: &WorkspaceView) -> Option<&str> {
        view.active_document_id.as_ref().map(|id| id.as_str())
    }

    #[test]
    fn snapshot_is_applied_then_deltas_replace_it() {
        let mut session = SessionViewReplica::new(SessionViewWatch::new());
        let workspace = WorkspaceId::new("workspace-1");
        session.subscribe_snapshot(workspace.clone(), 1, Some(view("document-a")));
        let (sequence, revision, applied) = session
            .watch()
            .take_if_changed(&workspace, 0)
            .expect("snapshot が適用できる");
        assert_eq!(revision, 1);
        assert_eq!(active(&applied), Some("document-a"));

        session.apply_event(&workspace, 2, Some(view("document-b")));
        let (_, revision, applied) = session
            .watch()
            .take_if_changed(&workspace, sequence)
            .expect("delta が適用できる");
        assert_eq!(revision, 2);
        assert_eq!(active(&applied), Some("document-b"));
    }

    #[test]
    fn own_revision_and_older_events_are_not_reapplied() {
        let mut session = SessionViewReplica::new(SessionViewWatch::new());
        let workspace = WorkspaceId::new("workspace-1");
        session.subscribe_snapshot(workspace.clone(), 0, None);
        // 自分の送信が revision 3 で受理された。
        session.accept_local(3);
        // 同じ revision と古い revision の event は適用しない。自己ループを作らない。
        session.apply_event(&workspace, 3, Some(view("document-a")));
        session.apply_event(&workspace, 2, Some(view("document-b")));
        assert!(session.watch().take_if_changed(&workspace, 0).is_none());

        // 新しい revision だけが適用される。
        session.apply_event(&workspace, 4, Some(view("document-c")));
        let (_, revision, applied) = session
            .watch()
            .take_if_changed(&workspace, 0)
            .expect("新しい revision を適用する");
        assert_eq!(revision, 4);
        assert_eq!(active(&applied), Some("document-c"));
    }

    #[test]
    fn null_view_events_advance_revision_without_applying() {
        let mut session = SessionViewReplica::new(SessionViewWatch::new());
        let workspace = WorkspaceId::new("workspace-1");
        session.subscribe_snapshot(workspace.clone(), 1, Some(view("document-a")));
        let (last, _, _) = session.watch().take_if_changed(&workspace, 0).unwrap();
        // 権限で伏せられた event は連番だけ進む。
        session.apply_event(&workspace, 2, None);
        assert!(session.watch().take_if_changed(&workspace, last).is_none());
        // 連番は進んでいるため、同じ revision の内容付き event は適用しない。
        session.apply_event(&workspace, 2, Some(view("document-b")));
        assert!(session.watch().take_if_changed(&workspace, last).is_none());
        session.apply_event(&workspace, 3, Some(view("document-c")));
        let (_, revision, applied) = session.watch().take_if_changed(&workspace, last).unwrap();
        assert_eq!(revision, 3);
        assert_eq!(active(&applied), Some("document-c"));
    }

    #[test]
    fn empty_snapshot_marks_the_workspace_synced_without_a_view() {
        let mut session = SessionViewReplica::new(SessionViewWatch::new());
        let workspace = WorkspaceId::new("workspace-1");
        session.subscribe_snapshot(workspace.clone(), 0, None);
        assert!(session.watch().is_synced(&workspace));
        assert!(session.watch().take_if_changed(&workspace, 0).is_none());
    }

    #[test]
    fn events_before_subscription_are_ignored() {
        let mut session = SessionViewReplica::new(SessionViewWatch::new());
        let workspace = WorkspaceId::new("workspace-1");
        session.apply_event(&workspace, 5, Some(view("document-a")));
        assert!(session.watch().take_if_changed(&workspace, 0).is_none());
    }

    #[test]
    fn reset_marks_the_view_stale_and_stops_applying_events() {
        let mut session = SessionViewReplica::new(SessionViewWatch::new());
        let workspace = WorkspaceId::new("workspace-1");
        session.subscribe_snapshot(workspace.clone(), 1, Some(view("document-a")));
        assert!(!session.watch().is_stale());
        session.reset();
        assert!(session.workspace().is_none());
        assert!(session.watch().is_stale());
        // 解除後の event は適用しない。
        session.apply_event(&workspace, 2, Some(view("document-b")));
        let (_, revision, _) = session.watch().take_if_changed(&workspace, 0).unwrap();
        assert_eq!(revision, 1);
    }

    #[test]
    fn another_workspace_event_is_ignored_while_subscribed() {
        let mut session = SessionViewReplica::new(SessionViewWatch::new());
        let workspace = WorkspaceId::new("workspace-1");
        session.subscribe_snapshot(workspace.clone(), 0, None);
        let other = WorkspaceId::new("workspace-2");
        session.apply_event(&other, 9, Some(view("document-a")));
        assert!(session.watch().take_if_changed(&other, 0).is_none());
        assert!(session.watch().take_if_changed(&workspace, 0).is_none());
    }

    /// Host は受理した view を、応答より先に event として配り得る。
    /// 送信前に記録した自分の値と一致する event は、応答の前後を問わず適用しない。
    #[test]
    fn own_echo_is_not_applied_whether_it_arrives_before_or_after_the_response() {
        for response_first in [true, false] {
            let mut session = SessionViewReplica::new(SessionViewWatch::new());
            let workspace = WorkspaceId::new("workspace-1");
            session.subscribe_snapshot(workspace.clone(), 0, None);

            // publish と同じ順序で、送信より前に自分の値を記録する。
            let local = view("document-a");
            session.begin_local(&local);
            if response_first {
                session.accept_local(1);
                session.apply_event(&workspace, 1, Some(local.clone()));
            } else {
                // 応答より先に自分の更新が event として戻る。
                session.apply_event(&workspace, 1, Some(local.clone()));
                session.accept_local(1);
            }
            assert!(
                session.watch().take_if_changed(&workspace, 0).is_none(),
                "自分の更新を再適用しました (response_first={response_first})"
            );

            // 別 Client の値は同じ経路で適用される。
            session.apply_event(&workspace, 2, Some(view("document-b")));
            let (_, revision, applied) = session
                .watch()
                .take_if_changed(&workspace, 0)
                .expect("他人の更新は適用する");
            assert_eq!(revision, 2);
            assert_eq!(active(&applied), Some("document-b"));
        }
    }

    #[test]
    fn a_failed_send_stops_suppressing_later_events() {
        let mut session = SessionViewReplica::new(SessionViewWatch::new());
        let workspace = WorkspaceId::new("workspace-1");
        session.subscribe_snapshot(workspace.clone(), 0, None);
        session.begin_local(&view("document-a"));
        // 送信できなかったので記録を捨てる。
        session.abort_local();
        // 同じ内容が他 Client から届いた場合は適用する。
        session.apply_event(&workspace, 1, Some(view("document-a")));
        assert!(
            session.watch().take_if_changed(&workspace, 0).is_some(),
            "送信に失敗した値を他人の更新として捨てました"
        );
    }

    /// 応答を待たずに次の送信が積み重なっても、先の送信の event を捨てない。
    #[test]
    fn a_pending_send_is_kept_when_the_next_send_starts() {
        let mut session = SessionViewReplica::new(SessionViewWatch::new());
        let workspace = WorkspaceId::new("workspace-1");
        session.subscribe_snapshot(workspace.clone(), 0, None);
        // A を送信し、応答前に B を送信する。
        let first = view("document-a");
        let second = view("document-b");
        session.begin_local(&first);
        session.begin_local(&second);
        // 応答より先に A の event が戻っても、自分の更新として捨てる。
        session.apply_event(&workspace, 1, Some(first.clone()));
        assert!(
            session.watch().take_if_changed(&workspace, 0).is_none(),
            "先の送信を自分の更新として扱いませんでした"
        );
        // B の応答より先に B の event も捨てる。
        session.apply_event(&workspace, 2, Some(second.clone()));
        assert!(session.watch().take_if_changed(&workspace, 0).is_none());
        // 他 Client の値は適用する。
        session.apply_event(&workspace, 3, Some(view("document-c")));
        assert!(session.watch().take_if_changed(&workspace, 0).is_some());
    }

    #[test]
    fn an_older_snapshot_does_not_rewind_the_applied_view() {
        let mut session = SessionViewReplica::new(SessionViewWatch::new());
        let workspace = WorkspaceId::new("workspace-1");
        session.subscribe_snapshot(workspace.clone(), 2, Some(view("document-b")));
        let (last, _, _) = session.watch().take_if_changed(&workspace, 0).unwrap();
        // 再接続前の古い snapshot が遅れて届いても巻き戻さない。
        session.subscribe_snapshot(workspace.clone(), 1, Some(view("document-a")));
        assert!(session.watch().take_if_changed(&workspace, last).is_none());

        // 新しい snapshot は適用する。
        session.subscribe_snapshot(workspace.clone(), 3, Some(view("document-c")));
        let (_, revision, _) = session.watch().take_if_changed(&workspace, last).unwrap();
        assert_eq!(revision, 3);
    }

    #[test]
    fn reset_requires_a_new_snapshot_before_publishing() {
        let mut session = SessionViewReplica::new(SessionViewWatch::new());
        let workspace = WorkspaceId::new("workspace-1");
        session.subscribe_snapshot(workspace.clone(), 1, Some(view("document-a")));
        assert!(session.watch().is_synced(&workspace));
        session.reset();
        // 購読解除後は、再購読の snapshot を確認するまで送信しない。
        assert!(!session.watch().is_synced(&workspace));
        session.subscribe_snapshot(workspace.clone(), 2, None);
        assert!(session.watch().is_synced(&workspace));
    }
}
