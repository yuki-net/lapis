use super::*;
use std::sync::Mutex;

/// 1 つの Workspace の view を保持する、Host 相当の共有先。
struct SharedView {
    state: Mutex<SharedState>,
}

struct SharedState {
    revision: u64,
    view: Option<WorkspaceView>,
    listeners: Vec<mpsc::Sender<SessionViewEvent>>,
    subscribed: usize,
    fail_update: bool,
}

impl SharedView {
    fn new() -> Self {
        Self {
            state: Mutex::new(SharedState {
                revision: 0,
                view: None,
                listeners: Vec::new(),
                subscribed: 0,
                fail_update: false,
            }),
        }
    }

    fn view(active: &str) -> WorkspaceView {
        WorkspaceView {
            open_documents: vec![workbench::WorkspaceDocumentView {
                document_id: Some(editor_core::DocumentId::new(active)),
                path: Some(workspace::WorkspaceRelativePath::parse("src/main.rs").unwrap()),
                ..Default::default()
            }],
            active_path: Some(workspace::WorkspaceRelativePath::parse("src/main.rs").unwrap()),
            active_document_id: Some(editor_core::DocumentId::new(active)),
        }
    }

    fn revision(&self) -> u64 {
        self.state.lock().unwrap().revision
    }

    fn set_fail_update(&self, fail: bool) {
        self.state.lock().unwrap().fail_update = fail;
    }
}

impl SessionViewConnection for SharedView {
    fn subscribe(&self, _: &WorkspaceId) -> Result<SessionViewSnapshot, String> {
        let mut state = self.state.lock().unwrap();
        state.subscribed += 1;
        Ok(SessionViewSnapshot {
            revision: state.revision,
            view: state.view.clone(),
        })
    }

    fn unsubscribe(&self, _: &WorkspaceId) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        state.subscribed = state.subscribed.saturating_sub(1);
        // 実際の Host でも購読解除後も接続中の Session へ event は届く。
        // 適用可否は client 側が購読状態で判断する。
        Ok(())
    }

    fn update(&self, _: &WorkspaceId, view: WorkspaceView) -> Result<u64, String> {
        let mut state = self.state.lock().unwrap();
        if state.fail_update {
            return Err("offline".into());
        }
        state.revision += 1;
        state.view = Some(view.clone());
        let revision = state.revision;
        let event = SessionViewEvent {
            revision,
            view: Some(view),
        };
        state
            .listeners
            .retain(|listener| listener.send(event.clone()).is_ok());
        Ok(revision)
    }

    fn events(&self, _: &WorkspaceId) -> Result<Box<dyn SessionViewEvents>, String> {
        let (sender, receiver) = mpsc::channel();
        self.state.lock().unwrap().listeners.push(sender);
        Ok(Box::new(SharedEvents { receiver }))
    }
}

struct SharedEvents {
    receiver: mpsc::Receiver<SessionViewEvent>,
}

impl SessionViewEvents for SharedEvents {
    fn next(&mut self, timeout: Duration) -> Result<Option<SessionViewEvent>, String> {
        match self.receiver.recv_timeout(timeout) {
            Ok(event) => Ok(Some(event)),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err("closed".into()),
        }
    }
}

fn wait_for(
    watch: &SessionViewWatch,
    workspace: &WorkspaceId,
    last: u64,
) -> (u64, WorkspaceView) {
    for _ in 0..200 {
        if let Some((sequence, _, view)) = watch.take_if_changed(workspace, last) {
            return (sequence, view);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("共有 view が届きません");
}

#[test]
fn update_is_applied_to_other_subscribers_as_the_latest_value() {
    let workspace = WorkspaceId::new("workspace-1");
    let shared = Arc::new(SharedView::new());
    let writer_watch = SessionViewWatch::new();
    let reader_watch = SessionViewWatch::new();
    let writer = SessionViewSession::start(shared.clone(), workspace.clone(), writer_watch.clone())
        .unwrap();
    let reader = SessionViewSession::start(shared.clone(), workspace.clone(), reader_watch.clone())
        .unwrap();
    writer
        .publish(SharedView::view("document-a"))
        .expect("update が受理されません");
    let (_, view) = wait_for(&reader_watch, &workspace, 0);
    assert_eq!(
        view.active_document_id,
        Some(editor_core::DocumentId::new("document-a"))
    );
    // 自分の更新は自分の watch へ戻さない。loop しない。
    assert!(writer_watch.take_if_changed(&workspace, 0).is_none());
    reader.close();
    writer.close();
}

#[test]
fn close_stops_receiving_later_changes() {
    let workspace = WorkspaceId::new("workspace-1");
    let shared = Arc::new(SharedView::new());
    let writer_watch = SessionViewWatch::new();
    let reader_watch = SessionViewWatch::new();
    let writer = SessionViewSession::start(shared.clone(), workspace.clone(), writer_watch.clone())
        .unwrap();
    let reader = SessionViewSession::start(shared.clone(), workspace.clone(), reader_watch.clone())
        .unwrap();
    writer.publish(SharedView::view("document-a")).unwrap();
    let (last, _) = wait_for(&reader_watch, &workspace, 0);
    reader.close();
    writer.publish(SharedView::view("document-b")).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert!(reader_watch.take_if_changed(&workspace, last).is_none());
    writer.close();
}

#[test]
fn resync_replaces_a_stale_local_revision_with_the_host_value() {
    let workspace = WorkspaceId::new("workspace-1");
    let shared = Arc::new(SharedView::new());
    let watch = SessionViewWatch::new();
    let session =
        SessionViewSession::start(shared.clone(), workspace.clone(), watch.clone()).unwrap();
    shared
        .update(&workspace, SharedView::view("document-a"))
        .unwrap();
    let (last, _) = wait_for(&watch, &workspace, 0);
    // event を取りこぼした状態を作り、snapshot で戻す。
    shared
        .state
        .lock()
        .unwrap()
        .listeners
        .clear();
    let view = SharedView::view("document-b");
    shared.state.lock().unwrap().view = Some(view.clone());
    shared.state.lock().unwrap().revision += 1;
    session.resync().unwrap();
    let (_, view) = wait_for(&watch, &workspace, last);
    assert_eq!(
        view.active_document_id,
        Some(editor_core::DocumentId::new("document-b"))
    );
    session.close();
}

#[test]
fn failed_update_reports_an_error_and_keeps_the_current_view() {
    let workspace = WorkspaceId::new("workspace-1");
    let shared = Arc::new(SharedView::new());
    let session = SessionViewSession::start(
        shared.clone(),
        workspace.clone(),
        SessionViewWatch::new(),
    )
    .unwrap();
    session.publish(SharedView::view("document-a")).unwrap();
    shared.set_fail_update(true);
    assert!(session.publish(SharedView::view("document-b")).is_err());
    assert_eq!(
        shared.state.lock().unwrap().view.as_ref().unwrap().active_document_id,
        Some(editor_core::DocumentId::new("document-a"))
    );
    session.close();
}

#[test]
fn sync_opens_once_per_workspace_and_switches_subscriptions() {
    let shared = Arc::new(SharedView::new());
    let sync = SessionViewSync::new(shared.clone(), SessionViewWatch::new());
    let first = WorkspaceId::new("workspace-1");
    sync.open(&first).unwrap();
    sync.open(&first).unwrap();
    assert_eq!(shared.state.lock().unwrap().subscribed, 1);
    let second = WorkspaceId::new("workspace-2");
    sync.open(&second).unwrap();
    sync.close();
    assert_eq!(shared.state.lock().unwrap().subscribed, 0);
}

#[test]
fn null_view_events_are_not_applied() {
    let workspace = WorkspaceId::new("workspace-1");
    let shared = Arc::new(SharedView::new());
    let writer_watch = SessionViewWatch::new();
    let reader_watch = SessionViewWatch::new();
    let writer = SessionViewSession::start(shared.clone(), workspace.clone(), writer_watch).unwrap();
    let reader = SessionViewSession::start(shared.clone(), workspace.clone(), reader_watch.clone())
        .unwrap();
    writer.publish(SharedView::view("document-a")).unwrap();
    let (last, _) = wait_for(&reader_watch, &workspace, 0);
    // 権限で伏せられた event は連番だけ進み、内容は配られない。
    let event = SessionViewEvent {
        revision: shared.revision() + 1,
        view: None,
    };
    shared.state.lock().unwrap().revision += 1;
    if let Some(listener) = shared.state.lock().unwrap().listeners.first() {
        listener.send(event).unwrap();
    }
    std::thread::sleep(Duration::from_millis(100));
    assert!(reader_watch.take_if_changed(&workspace, last).is_none());
    reader.close();
    writer.close();
}
