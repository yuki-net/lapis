use std::sync::Arc;

use lapis_editor_core::WorkspaceId;
use lapis_workbench::{
    ConversationViewState, VersionedViewState, WorkbenchConnection, WorkbenchUpdate,
};

/// Client側の未確定変更を保持し、古いHost状態で上書きしない。
pub struct WorkbenchSync {
    connection: Arc<dyn WorkbenchConnection>,
    workspace: Option<WorkspaceId>,
    confirmed: Option<VersionedViewState>,
    pending: Option<ConversationViewState>,
    conflict: Option<VersionedViewState>,
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
            confirmed: None,
            pending: None,
            conflict: None,
        }
    }

    pub fn open(&mut self, workspace: WorkspaceId) -> Result<VersionedViewState, String> {
        if self.pending.is_some() {
            return Err("未確定のWorkbench変更があります".into());
        }
        let snapshot = self.connection.get(&workspace)?;
        self.workspace = Some(workspace);
        self.confirmed = Some(snapshot.clone());
        self.conflict = None;
        Ok(snapshot)
    }

    pub fn pending(&self) -> Option<&ConversationViewState> {
        self.pending.as_ref()
    }
    pub fn confirmed(&self) -> Option<&VersionedViewState> {
        self.confirmed.as_ref()
    }
    pub fn conflict(&self) -> Option<&VersionedViewState> {
        self.conflict.as_ref()
    }

    pub fn refresh(&mut self) -> Result<Option<VersionedViewState>, String> {
        let workspace = self
            .workspace
            .as_ref()
            .ok_or("Workspaceが接続されていません")?;
        let snapshot = self.connection.get(workspace)?;
        let previous = self
            .confirmed
            .as_ref()
            .ok_or("Workbenchが初期化されていません")?;
        if snapshot.revision < previous.revision
            || (snapshot.revision == previous.revision && snapshot != *previous)
        {
            return Err("HostのWorkbench状態が以前のrevisionと一致しません".into());
        }
        if snapshot.revision == previous.revision {
            return Ok(None);
        }
        if self.pending.is_some() {
            self.conflict = Some(snapshot);
            return Ok(None);
        }
        self.confirmed = Some(snapshot.clone());
        self.conflict = None;
        Ok(Some(snapshot))
    }

    pub fn save(&mut self, view: ConversationViewState) -> Result<WorkbenchSyncResult, String> {
        self.pending = Some(view.clone());
        let workspace = self
            .workspace
            .as_ref()
            .ok_or("Workspaceが接続されていません")?;
        let revision = self
            .confirmed
            .as_ref()
            .ok_or("Workbenchが初期化されていません")?
            .revision;
        if let Some(current) = self.conflict.clone() {
            return Ok(WorkbenchSyncResult::Conflict(current));
        }
        match self.connection.update(workspace, revision, view)? {
            WorkbenchUpdate::Updated(snapshot) => {
                self.confirmed = Some(snapshot.clone());
                self.pending = None;
                self.conflict = None;
                Ok(WorkbenchSyncResult::Updated(snapshot))
            }
            WorkbenchUpdate::Conflict(snapshot) => {
                self.conflict = Some(snapshot.clone());
                Ok(WorkbenchSyncResult::Conflict(snapshot))
            }
        }
    }

    /// 利用者がHostの状態を選んだ場合だけ未確定変更を破棄する。
    pub fn accept_remote(&mut self) -> Result<VersionedViewState, String> {
        let current = self.conflict.take().ok_or("解決する競合がありません")?;
        self.confirmed = Some(current.clone());
        self.pending = None;
        Ok(current)
    }

    /// 利用者がローカル変更を選んだ場合だけ、競合したrevisionを基点に再送する。
    pub fn retry_local(&mut self) -> Result<WorkbenchSyncResult, String> {
        let current = self.conflict.clone().ok_or("解決する競合がありません")?;
        let pending = self
            .pending
            .clone()
            .ok_or("未確定のWorkbench変更がありません")?;
        self.conflict = None;
        self.confirmed = Some(current);
        self.save(pending)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lapis_workbench::{WorkbenchSubscription, WorkbenchUpdate};
    use std::sync::Mutex;

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
}
