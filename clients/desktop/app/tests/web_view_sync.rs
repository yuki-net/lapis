//! Web クライアントの Workspace view state 共有。
//!
//! Web の接続層（`desktop_client::SessionViewReplica`）と Editor の適用順を、
//! 実際の Host に対して確かめる。event は本番の Web と同じく購読 port から受け取る。

use std::sync::Arc;

use desktop_client::{
    EditorSession, SessionViewReplica, SessionViewWatch, WorkspaceStateRepository,
    WorkspaceViewState,
};
use host::BackendHost;

/// Web の DocumentClient と同じ順序で、購読 → snapshot 適用 → delta 適用 → 送信を回す。
struct Client {
    editor: EditorSession,
    replica: SessionViewReplica,
    /// 購読・解除で同じ Session を使うため、接続を 1 つだけ保持する。
    connection: std::sync::Arc<dyn workbench::SessionViewConnection>,
    /// 接続の event pump が受け取る `workspace.view_changed`。
    events: Option<Box<dyn workbench::SessionViewEvents>>,
    applied: u64,
    published: Option<desktop_client::WorkspaceView>,
}

impl Client {
    fn new(host: &BackendHost) -> Self {
        let state = Arc::new(ViewStore::default());
        let editor = EditorSession::new_empty(
            host.workspaces(),
            Arc::new(NoDialog),
            state,
            host.documents(),
        );
        Self {
            editor,
            replica: SessionViewReplica::new(SessionViewWatch::new()),
            connection: host.session_view(),
            events: None,
            applied: 0,
            published: None,
        }
    }

    /// `workspace.view.subscribe` で参加し、snapshot を取り込む。
    /// 先に event の受け口を作り、購読と snapshot の間の更新を取りこぼさない。
    fn subscribe(&mut self) {
        let workspace = self.editor.workspace_id().cloned().unwrap();
        if self.events.is_none() {
            self.events = Some(
                self.connection
                    .events(&workspace)
                    .expect("event を購読できる"),
            );
        }
        let snapshot = self.connection.subscribe(&workspace).expect("購読できる");
        self.replica
            .subscribe_snapshot(workspace, snapshot.revision, snapshot.view);
    }

    /// 購読と同じ Session で解除する。解除できなければテストを失敗させる。
    fn unsubscribe(&self) {
        let workspace = self.editor.workspace_id().cloned().unwrap();
        self.connection
            .unsubscribe(&workspace)
            .expect("購読を解除できる");
    }

    /// 受信口に溜まった event を、指定時間だけ読み捨てる。
    fn drain_events(&mut self, wait: std::time::Duration) -> usize {
        let mut seen = 0;
        if let Some(events) = self.events.as_mut() {
            let deadline = std::time::Instant::now() + wait;
            while std::time::Instant::now() < deadline {
                match events.next(std::time::Duration::from_millis(20)) {
                    Ok(Some(_)) => seen += 1,
                    Ok(None) => {}
                    Err(_) => break,
                }
            }
        }
        seen
    }

    /// 受信口に届いた `workspace.view_changed` の数だけを数える。
    fn drain_view_events(&mut self, wait: std::time::Duration) -> usize {
        let mut seen = 0;
        if let Some(events) = self.events.as_mut() {
            let deadline = std::time::Instant::now() + wait;
            while std::time::Instant::now() < deadline {
                match events.next(std::time::Duration::from_millis(20)) {
                    Ok(Some(event)) => {
                        if event.view.is_some() {
                            seen += 1;
                        }
                    }
                    Ok(None) => {}
                    Err(_) => break,
                }
            }
        }
        seen
    }

    /// event pump の 1 周分。届いた delta を replica に渡し、未適用の view を Editor へ反映する。
    fn apply_pending(&mut self) {
        let Some(workspace) = self.editor.workspace_id().cloned() else {
            // Workspace を閉じた後は適用対象が無い。受信口は読み捨てる。
            self.drain_events(std::time::Duration::from_millis(10));
            return;
        };
        if let Some(events) = self.events.as_mut() {
            while let Ok(Some(event)) = events.next(std::time::Duration::from_millis(0)) {
                self.replica
                    .apply_event(&workspace, event.revision, event.view);
            }
        }
        let Some((sequence, _, view)) = self
            .replica
            .watch()
            .take_if_changed(&workspace, self.applied)
        else {
            return;
        };
        self.applied = sequence;
        self.editor.apply_shared_view(&view).unwrap();
        self.published = Some(self.editor.snapshot().to_shared_tabs().unwrap());
    }

    fn publish(&mut self) {
        let workspace = self.editor.workspace_id().cloned().unwrap();
        if !self.replica.watch().is_synced(&workspace) {
            return;
        }
        self.publish_directly();
    }

    /// 購読していなくても view を送る。参加者の有無による配信の違いを確かめるため。
    fn publish_directly(&mut self) {
        let workspace = self.editor.workspace_id().cloned().unwrap();
        let view = self.editor.snapshot().to_shared_tabs().unwrap();
        if self.published.as_ref() == Some(&view) {
            return;
        }
        let revision = self.connection.update(&workspace, view.clone()).unwrap();
        self.replica.accept_local(revision);
        self.published = Some(view);
    }

    fn tabs(&self) -> Vec<String> {
        self.editor
            .tabs()
            .into_iter()
            .map(|tab| tab.display_name)
            .collect()
    }

    fn active(&self) -> Option<String> {
        self.editor
            .active_path()
            .map(|path| path.display().to_string())
    }

    /// 選択中の文書の本文。共有された active 文書が本文まで切り替わるか確かめる。
    fn active_text(&self) -> Option<String> {
        self.editor.active_text().ok()
    }
}

fn wait_until(mut condition: impl FnMut() -> bool) {
    for _ in 0..300 {
        if condition() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("共有 view が反映されません");
}

#[test]
fn web_client_shares_tabs_with_a_desktop_client_on_the_same_workspace() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    std::fs::write(root.join("a.rs"), "fn a() {}").unwrap();
    std::fs::write(root.join("b.rs"), "fn b() {}").unwrap();
    let host = BackendHost::start().unwrap();
    let mut web = Client::new(&host);
    let mut other = Client::new(&host);
    web.editor.open_workspace(root.clone()).unwrap();
    other.editor.open_workspace(root.clone()).unwrap();
    web.subscribe();
    other.subscribe();

    // Web が開いたタブが、同じ Workspace の別 Client へ delta で届く。
    web.editor.open_path(root.join("a.rs")).unwrap();
    web.publish();
    wait_until(|| {
        other.apply_pending();
        other.tabs().len() == 1
    });
    assert_eq!(other.tabs(), ["a.rs"]);
    // Web は document_id を共有しないため、アクティブ文書は path で一致させる。
    assert_eq!(other.active(), web.active());
    // 共有されたアクティブ文書の本文まで切り替わる。
    assert_eq!(other.active_text(), Some("fn a() {}".to_owned()));

    // 逆向きのタブ追加も Web に反映される。
    other.editor.open_path(root.join("b.rs")).unwrap();
    other.publish();
    wait_until(|| {
        web.apply_pending();
        web.tabs().len() == 2
    });
    assert_eq!(web.tabs(), ["a.rs", "b.rs"]);
    // Web 側でもアクティブ文書の本文が追従する。
    assert_eq!(web.active_text(), Some("fn b() {}".to_owned()));

    // Web 側のタブ削除も共有される。
    let id = web.editor.tabs()[1].id.clone();
    web.editor
        .close_document(
            &id,
            desktop_client::DocumentCloseDisposition::DiscardChanges,
        )
        .unwrap();
    web.publish();
    wait_until(|| {
        other.apply_pending();
        other.tabs().len() == 1
    });
    assert_eq!(other.tabs(), ["a.rs"]);
}

#[test]
fn web_client_does_not_reapply_its_own_update() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    std::fs::write(root.join("a.rs"), "fn a() {}").unwrap();
    let host = BackendHost::start().unwrap();
    let mut web = Client::new(&host);
    web.editor.open_workspace(root.clone()).unwrap();
    web.subscribe();
    web.editor.open_path(root.join("a.rs")).unwrap();
    let before = web.applied;
    web.publish();

    // 自分の送信が event として戻っても、最初の適用で取り込まない。
    web.apply_pending();
    assert_eq!(web.applied, before, "自分の更新を適用しました");
    web.apply_pending();
    assert_eq!(web.applied, before);
    assert_eq!(web.tabs(), ["a.rs"]);
}

#[test]
fn web_client_resyncs_from_a_snapshot_after_missing_a_delta() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    std::fs::write(root.join("a.rs"), "fn a() {}").unwrap();
    std::fs::write(root.join("b.rs"), "fn b() {}").unwrap();
    let host = BackendHost::start().unwrap();
    let mut web = Client::new(&host);
    let mut other = Client::new(&host);
    web.editor.open_workspace(root.clone()).unwrap();
    other.editor.open_workspace(root.clone()).unwrap();
    web.subscribe();
    other.subscribe();
    web.editor.open_path(root.join("a.rs")).unwrap();
    web.publish();
    wait_until(|| {
        other.apply_pending();
        other.tabs().len() == 1
    });

    // Web が delta を取りこぼした状態を作り、snapshot で戻す。
    other.editor.open_path(root.join("b.rs")).unwrap();
    other.publish();
    // 取りこぼしを表現するため、Web は event を読まずに snapshot を取り直す。
    web.events = None;
    web.subscribe();
    web.apply_pending();
    assert_eq!(web.tabs(), ["a.rs", "b.rs"]);
}

#[test]
fn closed_workspace_stops_receiving_shared_changes() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    std::fs::write(root.join("a.rs"), "fn a() {}").unwrap();
    std::fs::write(root.join("b.rs"), "fn b() {}").unwrap();
    let host = BackendHost::start().unwrap();
    let mut web = Client::new(&host);
    // 書き手は購読しない。参加者が Web だけの状態を作り、解除の効果を観測する。
    let mut other = Client::new(&host);
    web.editor.open_workspace(root.clone()).unwrap();
    other.editor.open_workspace(root.clone()).unwrap();
    web.subscribe();
    web.editor.open_path(root.join("a.rs")).unwrap();
    web.publish();
    // 参加者がいる間は、他の Client の更新が view の delta として届く。
    web.drain_events(std::time::Duration::from_millis(100));
    other.editor.open_path(root.join("b.rs")).unwrap();
    other.publish_directly();
    assert_eq!(
        web.drain_view_events(std::time::Duration::from_millis(300)),
        1,
        "参加中は view の delta が届く"
    );

    // Web は Workspace を閉じ、購読を解除する。
    web.unsubscribe();
    web.replica.reset();
    web.drain_events(std::time::Duration::from_millis(100));
    web.editor.close_workspace().unwrap();

    // 参加者がいなくなったので、Host は以降の view を配信しない。
    other.editor.open_path(root.join("a.rs")).unwrap();
    other.publish_directly();
    assert_eq!(
        web.drain_view_events(std::time::Duration::from_millis(300)),
        0,
        "購読解除した Session へ view の delta が届きました"
    );
    web.apply_pending();
    assert!(
        web.editor.tabs().is_empty(),
        "閉じた側が共有状態を適用しました"
    );
    assert!(web.replica.watch().is_stale());
}

#[derive(Default)]
struct ViewStore(std::sync::Mutex<Option<WorkspaceViewState>>);

impl WorkspaceStateRepository for ViewStore {
    fn load(&self) -> Result<Option<WorkspaceViewState>, workspace::WorkspaceError> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn save(&self, snapshot: &WorkspaceViewState) -> Result<(), workspace::WorkspaceError> {
        *self.0.lock().unwrap() = Some(snapshot.clone());
        Ok(())
    }
}

struct NoDialog;

impl desktop_client::WorkspaceDialog for NoDialog {
    fn choose_workspace_path(&self) -> Option<std::path::PathBuf> {
        None
    }
    fn choose_save_path(&self, _: &str) -> Option<std::path::PathBuf> {
        None
    }
}
