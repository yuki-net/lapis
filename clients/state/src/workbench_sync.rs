use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

use lapis_editor_core::WorkspaceId;
use lapis_workbench::{
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
}

impl WorkbenchWatch {
    pub fn new() -> Self {
        Self {
            revision: Arc::new(AtomicU64::new(u64::MAX)),
            view: Arc::new(Mutex::new(None)),
        }
    }

    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    pub fn publish(&self, revision: u64, view: ConversationViewState) {
        if let Ok(mut slot) = self.view.lock() {
            *slot = Some(view);
            self.revision.store(revision, Ordering::Release);
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
    use lapis_workbench::{WorkbenchSubscription, WorkbenchUpdate};
    use std::sync::Mutex;

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
