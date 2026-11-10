use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
    mpsc,
};

use editor_core::WorkspaceId;
use workbench::{
    ConversationViewState, VersionedViewState, WorkbenchConnection, WorkbenchReplica,
    WorkbenchUpdate, WorkbenchWritePlan,
};

/// Client??????????????Host??????????
pub struct WorkbenchSync {
    connection: Arc<dyn WorkbenchConnection>,
    workspace: Option<WorkspaceId>,
    replica: WorkbenchReplica,
}

/// 別Clientが保存したWorkbenchをUIへ渡すための共有スロット。
#[derive(Clone, Default)]
pub struct WorkbenchWatch {
    revision: Arc<AtomicU64>,
    view: Arc<Mutex<Option<ConversationViewState>>>,
    listeners: Arc<Mutex<Vec<mpsc::Sender<u64>>>>,
}

impl WorkbenchWatch {
    pub fn new() -> Self {
        Self {
            revision: Arc::new(AtomicU64::new(u64::MAX)),
            view: Arc::new(Mutex::new(None)),
            listeners: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    /// 新しいrevisionの通知を受け取る。UIはblocking受信をworker threadへ任せる。
    pub fn subscribe(&self) -> mpsc::Receiver<u64> {
        let (sender, receiver) = mpsc::channel();
        if let Ok(mut listeners) = self.listeners.lock() {
            listeners.push(sender);
        }
        receiver
    }

    pub fn publish(&self, revision: u64, view: ConversationViewState) {
        if let Ok(mut slot) = self.view.lock() {
            *slot = Some(view);
            self.revision.store(revision, Ordering::Release);
            if let Ok(mut listeners) = self.listeners.lock() {
                listeners.retain(|listener| listener.send(revision).is_ok());
            }
        }
    }

    pub fn take_if_changed(&self, last: u64) -> Option<(u64, ConversationViewState)> {
        let revision = self.revision();
        if revision == last || revision == u64::MAX {
            return None;
        }
        let view = self.view.lock().ok()?.clone()?;
        Some((revision, view))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum WorkbenchSyncResult {
    Updated(VersionedViewState),
    Conflict(VersionedViewState),
}

impl WorkbenchSync {
    pub fn new(connection: Arc<dyn WorkbenchConnection>) -> Self {
        Self {
            connection,
            workspace: None,
            replica: WorkbenchReplica::default(),
        }
    }

    pub fn open(&mut self, workspace: WorkspaceId) -> Result<VersionedViewState, String> {
        self.replica.ensure_can_open()?;
        let snapshot = self.connection.get(&workspace)?;
        self.replica.open(snapshot.clone())?;
        self.workspace = Some(workspace);
        Ok(snapshot)
    }

    pub fn pending(&self) -> Option<&ConversationViewState> {
        self.replica.pending()
    }
    pub fn confirmed(&self) -> Option<&VersionedViewState> {
        self.replica.confirmed()
    }
    pub fn conflict(&self) -> Option<&VersionedViewState> {
        self.replica.conflict()
    }

    pub fn refresh(&mut self) -> Result<Option<VersionedViewState>, String> {
        let workspace = self.workspace.as_ref().ok_or("Workspace??????????")?;
        let snapshot = self.connection.get(workspace)?;
        self.replica.refresh(snapshot)
    }

    /// 新しい接続で正本を確認し、未確定値を保持してから接続を交換する。
    pub fn reconnect(
        &mut self,
        connection: Arc<dyn WorkbenchConnection>,
    ) -> Result<Option<VersionedViewState>, String> {
        let workspace = self
            .workspace
            .as_ref()
            .ok_or("Workspaceが接続されていません")?;
        let snapshot = connection.get(workspace)?;
        let refreshed = self.replica.refresh(snapshot)?;
        self.connection = connection;
        Ok(refreshed)
    }

    pub fn save(&mut self, view: ConversationViewState) -> Result<WorkbenchSyncResult, String> {
        let plan = self.replica.stage(view)?;
        let workspace = self.workspace.as_ref().ok_or("Workspace??????????")?;
        let result = match plan {
            WorkbenchWritePlan::Conflict(snapshot) => {
                return Ok(WorkbenchSyncResult::Conflict(snapshot));
            }
            WorkbenchWritePlan::Update { revision, view } => {
                self.connection.update(workspace, revision, view)?
            }
        };
        Ok(match self.replica.finish(result)? {
            WorkbenchUpdate::Updated(snapshot) => WorkbenchSyncResult::Updated(snapshot),
            WorkbenchUpdate::Conflict(snapshot) => WorkbenchSyncResult::Conflict(snapshot),
        })
    }

    /// ????Host??????????????????????
    pub fn accept_remote(&mut self) -> Result<VersionedViewState, String> {
        self.replica.accept_remote()
    }

    /// 送信が中断された値を再送する。競合している場合は上書きしない。
    pub fn retry_pending(&mut self) -> Result<WorkbenchSyncResult, String> {
        let pending = self
            .replica
            .pending()
            .cloned()
            .ok_or("未確定のWorkbench変更がありません")?;
        self.save(pending)
    }

    /// ??????????????????????revision?????????
    pub fn retry_local(&mut self) -> Result<WorkbenchSyncResult, String> {
        let pending = self.replica.prepare_retry()?;
        self.save(pending)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;
    use workbench::{
        TabRef, WorkbenchSubscription, WorkbenchUpdate, ZoneLayoutViewState, ZonePanelViewState,
        ZoneViewState,
    };

    fn tabs_view(tabs: &[&str]) -> ConversationViewState {
        let tabs = tabs
            .iter()
            .map(|id| TabRef::Document {
                id: editor_core::DocumentId::new(*id),
            })
            .collect::<Vec<_>>();
        ConversationViewState {
            layout: Some(ZoneLayoutViewState {
                version: workbench::ZONE_LAYOUT_VERSION,
                zones: vec![ZoneViewState {
                    position: "main".to_owned(),
                    open: true,
                    extent_ratio: 1.0,
                    layout: None,
                    panels: vec![ZonePanelViewState {
                        id: "panel".to_owned(),
                        active_tab: tabs.first().cloned(),
                        tabs,
                        pinned_tabs: Vec::new(),
                        open: true,
                    }],
                }],
                bottom_span_left: false,
                bottom_span_right: false,
            }),
            ..Default::default()
        }
    }

    fn tab_ids(view: &ConversationViewState) -> Vec<&str> {
        view.layout
            .as_ref()
            .and_then(|layout| layout.zones.first())
            .and_then(|zone| zone.panels.first())
            .map(|panel| {
                panel
                    .tabs
                    .iter()
                    .map(|tab| match tab {
                        TabRef::Document { id } => id.as_str(),
                        _ => "",
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn watch_exposes_only_unseen_views() {
        let watch = WorkbenchWatch::new();
        assert!(watch.take_if_changed(watch.revision()).is_none());
        watch.publish(
            3,
            ConversationViewState {
                show_panel_controls: false,
                ..Default::default()
            },
        );
        assert!(watch.take_if_changed(3).is_none());
        let (revision, view) = watch.take_if_changed(2).unwrap();
        assert_eq!(revision, 3);
        assert!(!view.show_panel_controls);
    }

    #[test]
    fn watchers_are_notified_of_new_revisions_without_polling() {
        let watch = WorkbenchWatch::new();
        let first = watch.subscribe();
        let second = watch.subscribe();
        watch.publish(4, ConversationViewState::default());
        assert_eq!(
            first.recv_timeout(Duration::from_secs(1)).unwrap(),
            4,
            "最初の購読者へ通知されません"
        );
        assert_eq!(
            second.recv_timeout(Duration::from_secs(1)).unwrap(),
            4,
            "二番目の購読者へ通知されません"
        );
        drop(first);
        watch.publish(5, ConversationViewState::default());
        assert_eq!(
            second.recv_timeout(Duration::from_secs(1)).unwrap(),
            5,
            "切断した購読者が残りの通知を妨げています"
        );
    }

    #[test]
    fn remote_tab_changes_replace_the_confirmed_view_and_retain_pending_local_changes() {
        let store = Arc::new(Store::default());
        let workspace = WorkspaceId::new("workspace");
        let mut remote = WorkbenchSync::new(store.clone());
        remote.open(workspace.clone()).unwrap();
        let mut local = WorkbenchSync::new(store.clone());
        local.open(workspace).unwrap();

        // 別Clientのタブ追加・移動を取り込む。
        assert!(matches!(
            remote.save(tabs_view(&["doc-a", "doc-b"])).unwrap(),
            WorkbenchSyncResult::Updated(_)
        ));
        let applied = local.refresh().unwrap().unwrap();
        assert_eq!(tab_ids(applied.view.as_ref().unwrap()), ["doc-a", "doc-b"]);

        // ローカルの未確定変更は上書きせず、競合として保持する。
        *store.fail.lock().unwrap() = true;
        let pending = tabs_view(&["doc-local"]);
        assert!(local.save(pending.clone()).is_err());
        *store.fail.lock().unwrap() = false;
        assert!(matches!(
            remote.save(tabs_view(&["doc-b", "doc-c"])).unwrap(),
            WorkbenchSyncResult::Updated(_)
        ));
        assert!(local.refresh().unwrap().is_none());
        assert_eq!(local.pending(), Some(&pending));
        assert_eq!(
            tab_ids(local.conflict().unwrap().view.as_ref().unwrap()),
            ["doc-b", "doc-c"]
        );

        // リモートを採用すると削除と移動が反映される。
        let accepted = local.accept_remote().unwrap();
        assert_eq!(tab_ids(accepted.view.as_ref().unwrap()), ["doc-b", "doc-c"]);
        assert!(local.pending().is_none());
    }

    #[derive(Default)]
    struct Store {
        state: Mutex<VersionedViewState>,
        fail: Mutex<bool>,
    }
    impl WorkbenchConnection for Store {
        fn get(&self, _: &WorkspaceId) -> Result<VersionedViewState, String> {
            Ok(self.state.lock().unwrap().clone())
        }
        fn update(
            &self,
            _: &WorkspaceId,
            expected: u64,
            view: ConversationViewState,
        ) -> Result<WorkbenchUpdate, String> {
            if *self.fail.lock().unwrap() {
                return Err("offline".into());
            }
            let mut state = self.state.lock().unwrap();
            if state.update(expected, view).is_err() {
                return Ok(WorkbenchUpdate::Conflict(state.clone()));
            }
            Ok(WorkbenchUpdate::Updated(state.clone()))
        }
        fn subscribe(&self, _: &WorkspaceId) -> Result<Box<dyn WorkbenchSubscription>, String> {
            Err("unused".into())
        }
    }

    #[test]
    fn conflict_preserves_local_view_until_explicit_resolution() {
        let store = Arc::new(Store::default());
        let workspace = WorkspaceId::new("workspace");
        let mut first = WorkbenchSync::new(store.clone());
        let mut second = WorkbenchSync::new(store);
        first.open(workspace.clone()).unwrap();
        second.open(workspace).unwrap();
        let view = ConversationViewState {
            show_panel_controls: false,
            ..Default::default()
        };
        assert!(matches!(
            first.save(view.clone()).unwrap(),
            WorkbenchSyncResult::Updated(_)
        ));
        let pending = ConversationViewState::default();
        assert!(matches!(
            second.save(pending.clone()).unwrap(),
            WorkbenchSyncResult::Conflict(_)
        ));
        assert_eq!(second.pending(), Some(&pending));
        assert_eq!(second.confirmed().unwrap().revision, 0);
        assert_eq!(second.conflict().unwrap().view, Some(view));
        assert!(second.open(WorkspaceId::new("other")).is_err());
        assert!(matches!(
            second.retry_local().unwrap(),
            WorkbenchSyncResult::Updated(_)
        ));
        assert!(second.pending().is_none());
    }

    #[test]
    fn failed_send_preserves_pending_changes_and_detects_new_remote_state() {
        let store = Arc::new(Store::default());
        let workspace = WorkspaceId::new("workspace");
        let mut sync = WorkbenchSync::new(store.clone());
        sync.open(workspace.clone()).unwrap();
        *store.fail.lock().unwrap() = true;
        let pending = ConversationViewState {
            show_panel_controls: false,
            ..Default::default()
        };
        assert!(sync.save(pending.clone()).is_err());
        *store.fail.lock().unwrap() = false;
        store
            .update(&workspace, 0, ConversationViewState::default())
            .unwrap();
        assert!(sync.refresh().unwrap().is_none());
        assert_eq!(sync.pending(), Some(&pending));
        assert_eq!(sync.conflict().unwrap().revision, 1);
        let accepted = sync.accept_remote().unwrap();
        assert_eq!(accepted.revision, 1);
        assert!(sync.pending().is_none());
        assert!(sync.conflict().is_none());
    }

    #[test]
    fn reconnect_retains_pending_and_retries_with_the_new_connection() {
        let old = Arc::new(Store::default());
        let replacement = Arc::new(Store::default());
        let mut sync = WorkbenchSync::new(old.clone());
        sync.open(WorkspaceId::new("workspace")).unwrap();
        *old.fail.lock().unwrap() = true;
        let pending = ConversationViewState {
            show_panel_controls: false,
            ..Default::default()
        };
        assert!(sync.save(pending.clone()).is_err());
        assert!(sync.reconnect(replacement.clone()).unwrap().is_none());
        assert_eq!(sync.pending(), Some(&pending));
        assert!(matches!(
            sync.retry_pending().unwrap(),
            WorkbenchSyncResult::Updated(_)
        ));
        assert_eq!(replacement.state.lock().unwrap().view, Some(pending));
        assert!(sync.pending().is_none());
    }

    #[test]
    fn conflict_refresh_does_not_replace_a_newer_observed_state() {
        let store = Arc::new(Store::default());
        let workspace = WorkspaceId::new("workspace");
        let mut sync = WorkbenchSync::new(store.clone());
        sync.open(workspace.clone()).unwrap();
        store
            .update(&workspace, 0, ConversationViewState::default())
            .unwrap();
        store
            .update(&workspace, 1, ConversationViewState::default())
            .unwrap();
        let pending = ConversationViewState {
            show_panel_controls: false,
            ..Default::default()
        };
        assert!(matches!(
            sync.save(pending.clone()).unwrap(),
            WorkbenchSyncResult::Conflict(_)
        ));
        *store.state.lock().unwrap() = VersionedViewState {
            revision: 1,
            view: Some(ConversationViewState::default()),
        };
        assert!(sync.refresh().is_err());
        assert_eq!(sync.conflict().unwrap().revision, 2);
        assert_eq!(sync.pending(), Some(&pending));
    }
}
