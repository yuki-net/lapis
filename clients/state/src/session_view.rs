//! Workspace の view state（開いているタブと表示位置）の Session 共有を、
//! 背景 thread と UI が読む watch に分離して扱う。
//!
//! 正本は Host にあり、購読した Client は snapshot の後に最新の view 全体を受け取る。
//! 差分やマージは行わず、Host が受理した値だけを watch へ載せる。event は接続中の
//! 全 Session に配られるため、購読していない間と、権限で内容が伏せられた event は
//! 適用しない。

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

    fn publish(&self, workspace: WorkspaceId, revision: u64, view: WorkspaceView) {
        let Ok(mut slot) = self.slot.lock() else {
            return;
        };
        *slot = Some((workspace, revision, view));
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
