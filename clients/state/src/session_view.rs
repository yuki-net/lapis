//! Workspace の view state（開いているタブとアクティブ文書）の Session 共有。
//!
//! 正本は Host にあり、購読した Client は snapshot の後に最新の view 全体を受け取る。
//! 差分やマージは行わず、Host が受理した値だけを watch へ載せる。event は接続中の
//! 全 Session に配られるため、購読していない間と、権限で内容が伏せられた event は
//! 適用しない。
//!
//! 受信手段は platform で異なる（Desktop は背景 thread、Web は非同期の event pump）。
//! 適用可否と通知順の判断はここで共有し、受信だけを各 platform が持つ。

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc,
};

use editor_core::WorkspaceId;
use workbench::WorkspaceView;

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

    /// この Workspace の snapshot を一度でも受け取ったか。
    pub fn is_synced(&self, workspace: &WorkspaceId) -> bool {
        self.synced
            .lock()
            .is_ok_and(|synced| synced.as_ref() == Some(workspace))
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

    fn notify(&self) {
        let sequence = self.sequence();
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
/// 巻き戻り得るため、Workspace と組で保持する。`atomic` なので、背景 thread と
/// UI thread の双方から同じ規則で更新できる。
pub struct SessionViewCursor {
    workspace: WorkspaceId,
    /// 適用済み、または自分が受理させた revision。これ以下は再適用しない。
    applied: AtomicU64,
    /// 送信中、または受理させた自分の view。自分の更新が event として戻ってきた
    /// 場合に再適用しないための照合値。送信より前に記録するので、応答と event の
    /// 到着順に依存しない。
    local: Mutex<Option<WorkspaceView>>,
}

impl SessionViewCursor {
    pub fn new(workspace: WorkspaceId, revision: u64) -> Self {
        Self {
            workspace,
            applied: AtomicU64::new(revision),
            local: Mutex::new(None),
        }
    }

    pub fn workspace(&self) -> &WorkspaceId {
        &self.workspace
    }

    pub fn applied(&self) -> u64 {
        self.applied.load(Ordering::Acquire)
    }

    /// これから送る自分の view を記録する。送信より前に呼ぶ。
    pub fn begin_local(&self, view: &WorkspaceView) {
        if let Ok(mut local) = self.local.lock() {
            *local = Some(view.clone());
        }
    }

    /// 送信が失敗したため、記録した自分の view を捨てる。
    pub fn abort_local(&self) {
        if let Ok(mut local) = self.local.lock() {
            *local = None;
        }
    }

    /// 自分の送信が受理された revision を、適用済みとして扱う。
    pub fn accept_local(&self, revision: u64) {
        self.applied.fetch_max(revision, Ordering::AcqRel);
        self.abort_local();
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
        if self.is_local(&view) {
            return None;
        }
        Some((revision, view))
    }

    fn is_local(&self, view: &WorkspaceView) -> bool {
        self.local
            .lock()
            .is_ok_and(|local| local.as_ref() == Some(view))
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
    pub fn subscribe_snapshot(
        &mut self,
        workspace: WorkspaceId,
        revision: u64,
        view: Option<WorkspaceView>,
    ) {
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

    /// 自分の送信を記録する。応答と event の到着順に依存せず、自分の更新を
    /// 再適用しないために、送信より前に呼ぶ。
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
        self.watch.mark_stale();
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod desktop {
    use super::*;
    use std::time::Duration;

    use workbench::{SessionViewConnection, SessionViewEvent, SessionViewEvents};

    /// Workspace 単位の購読 session。Workspace を閉じるまで 1 つだけ持つ。
    pub struct SessionViewSession {
        inner: Arc<Inner>,
    }

    struct Inner {
        connection: Arc<dyn SessionViewConnection>,
        cursor: SessionViewCursor,
        watch: SessionViewWatch,
        stopped: AtomicBool,
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
                cursor: SessionViewCursor::new(workspace, snapshot.revision),
                watch,
                stopped: AtomicBool::new(false),
            });
            publish_snapshot(&inner, &snapshot);
            let worker = inner.clone();
            std::thread::spawn(move || run_events(worker, events));
            Ok(Self { inner })
        }

        pub fn workspace(&self) -> &WorkspaceId {
            self.inner.cursor.workspace()
        }

        /// 自分の view を送る。受理した revision を適用済みとして扱い、
        /// 同じ値が event で返ってきても再度適用しない。
        pub fn publish(&self, view: WorkspaceView) -> Result<u64, String> {
            // 応答と event の到着順に依存しないよう、送信より前に自分の値を記録する。
            self.inner.cursor.begin_local(&view);
            let revision = match self
                .inner
                .connection
                .update(self.inner.cursor.workspace(), view)
            {
                Ok(revision) => revision,
                Err(error) => {
                    self.inner.cursor.abort_local();
                    return Err(error);
                }
            };
            self.inner.cursor.accept_local(revision);
            Ok(revision)
        }

        /// バックグラウンド復帰などで snapshot を取り直して再同期する。
        pub fn resync(&self) -> Result<u64, String> {
            let snapshot = self
                .inner
                .connection
                .subscribe(self.inner.cursor.workspace())?;
            self.inner.cursor.accept_local(snapshot.revision);
            publish_snapshot(&self.inner, &snapshot);
            Ok(snapshot.revision)
        }

        /// 購読を終了する。以後の delta は Host から届かない。
        pub fn close(&self) {
            if self.inner.stopped.swap(true, Ordering::AcqRel) {
                return;
            }
            if let Err(error) = self
                .inner
                .connection
                .unsubscribe(self.inner.cursor.workspace())
            {
                eprintln!("Workspace view の購読解除に失敗しました: {error}");
            }
        }
    }

    impl Drop for Inner {
        fn drop(&mut self) {
            self.stopped.store(true, Ordering::Release);
        }
    }

    fn publish_snapshot(inner: &Inner, snapshot: &workbench::SessionViewSnapshot) {
        if let Some(view) = &snapshot.view {
            inner.watch.publish(
                inner.cursor.workspace().clone(),
                snapshot.revision,
                view.clone(),
            );
        } else {
            inner.watch.mark_synced(inner.cursor.workspace().clone());
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
                            // 表示は最後の値のまま残し、古い可能性を UI へ知らせる。
                            inner.watch.mark_stale();
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
            match inner.connection.events(inner.cursor.workspace()) {
                Ok(replacement) => {
                    events = replacement;
                    backoff = Duration::from_secs(1);
                    if let Err(error) = inner.resubscribe() {
                        inner.watch.mark_stale();
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
            if let Some((revision, view)) = self.cursor.accept_event(event.revision, event.view) {
                self.watch
                    .publish(self.cursor.workspace().clone(), revision, view);
            }
        }

        fn resubscribe(&self) -> Result<(), String> {
            let snapshot = self.connection.subscribe(self.cursor.workspace())?;
            self.cursor.accept_local(snapshot.revision);
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
}

#[cfg(not(target_arch = "wasm32"))]
pub use desktop::{SessionViewSession, SessionViewSync};

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
}
