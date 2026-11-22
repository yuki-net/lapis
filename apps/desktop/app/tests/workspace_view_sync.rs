use std::sync::Arc;

use desktop_client::{
    EditorSession, SessionViewSync, SessionViewWatch, WorkspaceStateRepository, WorkspaceViewState,
};
use host::BackendHost;

/// Desktop の DocumentClient と同じ順序で、購読 → snapshot 適用 → 送信を繰り返す。
struct Client {
    editor: EditorSession,
    sync: Arc<SessionViewSync>,
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
            sync: Arc::new(SessionViewSync::new(
                host.session_view(),
                SessionViewWatch::new(),
            )),
            applied: 0,
            published: None,
        }
    }

    /// snapshot を取り込み、Host が返した view をローカルへ適用する。
    fn sync_from_host(&mut self) {
        let workspace = self.editor.workspace_id().cloned().unwrap();
        self.sync.open(&workspace).unwrap();
        let Some((sequence, _, view)) = self.sync.watch().take_if_changed(&workspace, self.applied)
        else {
            return;
        };
        self.applied = sequence;
        self.editor.apply_shared_view(&view).unwrap();
        self.published = Some(self.editor.snapshot().to_shared_view().unwrap());
    }

    /// 自分のタブ構成を共有へ送る。
    fn publish(&mut self) {
        let workspace = self.editor.workspace_id().cloned().unwrap();
        self.sync.open(&workspace).unwrap();
        let view = self.editor.snapshot().to_shared_view().unwrap();
        if self.published.as_ref() == Some(&view) {
            return;
        }
        self.sync.publish(view.clone()).unwrap();
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
}

fn wait_until(mut condition: impl FnMut() -> bool) {
    for _ in 0..200 {
        if condition() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("共有 view が反映されません");
}

#[test]
fn two_desktop_clients_share_tab_set_and_active_document() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    std::fs::write(root.join("a.rs"), "fn a() {}").unwrap();
    std::fs::write(root.join("b.rs"), "fn b() {}").unwrap();
    let host = BackendHost::start().unwrap();
    let mut first = Client::new(&host);
    let mut second = Client::new(&host);
    first.editor.open_workspace(root.clone()).unwrap();
    second.editor.open_workspace(root.clone()).unwrap();

    // 最初に開いた側のタブ構成が、後から開いた側へ snapshot で届く。
    first.editor.open_path(root.join("a.rs")).unwrap();
    first.publish();
    wait_until(|| {
        second.sync_from_host();
        second.tabs().len() == 1
    });
    assert_eq!(second.tabs(), ["a.rs"]);
    assert_eq!(second.active(), first.active());

    // 片方でタブを開くと、もう片方へ delta で反映される。
    first.editor.open_path(root.join("b.rs")).unwrap();
    first.publish();
    wait_until(|| {
        second.sync_from_host();
        second.tabs().len() == 2
    });
    assert_eq!(second.tabs(), ["a.rs", "b.rs"]);

    // アクティブ文書の変更も反映される。
    first
        .editor
        .activate_document(&first.editor.tabs()[0].id.clone());
    first.publish();
    wait_until(|| {
        second.sync_from_host();
        second.active() == first.active()
    });

    // 逆向きのタブ削除も反映される。
    let id = second.editor.tabs()[1].id.clone();
    second
        .editor
        .close_document(
            &id,
            desktop_client::DocumentCloseDisposition::DiscardChanges,
        )
        .unwrap();
    second.publish();
    wait_until(|| {
        first.sync_from_host();
        first.tabs().len() == 1
    });
    assert_eq!(first.tabs(), ["a.rs"]);

    first.sync.close();
    second.sync.close();
}

#[test]
fn a_closed_workspace_does_not_apply_later_changes() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    std::fs::write(root.join("a.rs"), "fn a() {}").unwrap();
    std::fs::write(root.join("b.rs"), "fn b() {}").unwrap();
    let host = BackendHost::start().unwrap();
    let mut first = Client::new(&host);
    let mut second = Client::new(&host);
    first.editor.open_workspace(root.clone()).unwrap();
    second.editor.open_workspace(root.clone()).unwrap();
    first.editor.open_path(root.join("a.rs")).unwrap();
    first.publish();
    wait_until(|| {
        second.sync_from_host();
        second.tabs().len() == 1
    });

    // second は Workspace を閉じ、購読も解除する。
    second.sync.close();
    second.editor.close_workspace().unwrap();

    first.editor.open_path(root.join("b.rs")).unwrap();
    first.publish();
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(
        second.editor.tabs().is_empty(),
        "閉じた側へ変更が反映されました"
    );
    first.sync.close();
}

#[test]
fn unsaved_drafts_are_kept_when_shared_tabs_change() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    std::fs::write(root.join("a.rs"), "fn a() {}").unwrap();
    std::fs::write(root.join("b.rs"), "fn b() {}").unwrap();
    let host = BackendHost::start().unwrap();
    let mut first = Client::new(&host);
    let mut second = Client::new(&host);
    first.editor.open_workspace(root.clone()).unwrap();
    second.editor.open_workspace(root.clone()).unwrap();
    first.editor.open_path(root.join("a.rs")).unwrap();
    first.publish();
    wait_until(|| {
        second.sync_from_host();
        second.tabs().len() == 1
    });

    // 未保存の入力を抱えた側へタブ構成を合わせても、内容は失わない。
    second.editor.replace_range(0..0, "draft ").unwrap();
    first.editor.open_path(root.join("b.rs")).unwrap();
    first.publish();
    wait_until(|| {
        second.sync_from_host();
        second.tabs().len() == 2
    });
    let dirty = second
        .editor
        .tabs()
        .into_iter()
        .find(|tab| tab.display_name == "a.rs")
        .expect("未保存のタブが失われました");
    assert!(dirty.dirty);
    assert!(second.editor.activate_document(&dirty.id));
    assert_eq!(second.editor.active_text().unwrap(), "draft fn a() {}");

    first.sync.close();
    second.sync.close();
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
