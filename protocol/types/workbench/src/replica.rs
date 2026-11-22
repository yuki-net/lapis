//! 通信方式に依存しない作業状態の競合管理。

use crate::{ConversationViewState, VersionedViewState, WorkbenchUpdate};

#[derive(Default)]
pub struct WorkbenchReplica {
    confirmed: Option<VersionedViewState>,
    pending: Option<ConversationViewState>,
    conflict: Option<VersionedViewState>,
}

pub enum WorkbenchWritePlan {
    Update {
        revision: u64,
        view: ConversationViewState,
    },
    Conflict(VersionedViewState),
}

impl WorkbenchReplica {
    pub fn pending(&self) -> Option<&ConversationViewState> {
        self.pending.as_ref()
    }
    pub fn confirmed(&self) -> Option<&VersionedViewState> {
        self.confirmed.as_ref()
    }
    pub fn conflict(&self) -> Option<&VersionedViewState> {
        self.conflict.as_ref()
    }

    pub fn ensure_can_open(&self) -> Result<(), String> {
        if self.pending.is_some() {
            Err("未確定のWorkbench変更があります".into())
        } else {
            Ok(())
        }
    }

    pub fn open(&mut self, snapshot: VersionedViewState) -> Result<(), String> {
        self.ensure_can_open()?;
        if !snapshot.is_consistent() {
            return Err("HostのWorkbench状態が不正です".into());
        }
        self.confirmed = Some(snapshot);
        self.conflict = None;
        Ok(())
    }

    pub fn refresh(
        &mut self,
        snapshot: VersionedViewState,
    ) -> Result<Option<VersionedViewState>, String> {
        if !snapshot.is_consistent() {
            return Err("HostのWorkbench状態が不正です".into());
        }
        if self.confirmed.is_none() && self.conflict.is_none() {
            let has_pending = self.pending.is_some();
            self.confirmed = Some(snapshot.clone());
            if has_pending {
                self.conflict = Some(snapshot.clone());
            }
            return Ok(Some(snapshot));
        }
        let previous = self
            .conflict
            .as_ref()
            .or(self.confirmed.as_ref())
            .ok_or("Workbenchが初期化されていません")?;
        if snapshot.revision < previous.revision
            || (snapshot.revision == previous.revision && snapshot != *previous)
        {
            return Err("HostのWorkbench状態が以前のrevisionと一致しません".into());
        }
        if snapshot.revision == previous.revision {
            return Ok(None);
        }
        // 応答だけが失われた場合、Hostの正本が未確定値と一致すれば変更は残っている。
        if self.pending.as_ref() == snapshot.view.as_ref()
            && self.pending.is_some()
            && self
                .confirmed
                .as_ref()
                .and_then(|state| state.revision.checked_add(1))
                == Some(snapshot.revision)
        {
            self.confirmed = Some(snapshot.clone());
            self.pending = None;
            self.conflict = None;
            return Ok(Some(snapshot));
        }
        if self.pending.is_some() {
            self.conflict = Some(snapshot);
            return Ok(None);
        }
        self.confirmed = Some(snapshot.clone());
        self.conflict = None;
        Ok(Some(snapshot))
    }

    pub fn stage(&mut self, view: ConversationViewState) -> Result<WorkbenchWritePlan, String> {
        let revision = self
            .confirmed
            .as_ref()
            .ok_or("Workbenchが初期化されていません")?
            .revision;
        self.pending = Some(view.clone());
        if let Some(current) = self.conflict.clone() {
            return Ok(WorkbenchWritePlan::Conflict(current));
        }
        Ok(WorkbenchWritePlan::Update { revision, view })
    }

    pub fn finish(&mut self, result: WorkbenchUpdate) -> Result<WorkbenchUpdate, String> {
        let confirmed = self
            .confirmed
            .as_ref()
            .ok_or("Workbenchが初期化されていません")?;
        let pending = self
            .pending
            .as_ref()
            .ok_or("未確定のWorkbench変更がありません")?;
        match &result {
            WorkbenchUpdate::Updated(snapshot) | WorkbenchUpdate::Conflict(snapshot)
                if !snapshot.is_consistent() =>
            {
                return Err("HostのWorkbench状態が不正です".into());
            }
            WorkbenchUpdate::Updated(snapshot)
                if confirmed.revision.checked_add(1) != Some(snapshot.revision)
                    || snapshot.view.as_ref() != Some(pending) =>
            {
                return Err("HostのWorkbench更新応答が不正です".into());
            }
            WorkbenchUpdate::Conflict(snapshot) if snapshot.revision <= confirmed.revision => {
                return Err("HostのWorkbench競合応答が不正です".into());
            }
            _ => {}
        }
        Ok(match result {
            WorkbenchUpdate::Updated(snapshot) => {
                self.confirmed = Some(snapshot.clone());
                self.pending = None;
                self.conflict = None;
                WorkbenchUpdate::Updated(snapshot)
            }
            WorkbenchUpdate::Conflict(snapshot) => {
                self.conflict = Some(snapshot.clone());
                WorkbenchUpdate::Conflict(snapshot)
            }
        })
    }

    pub fn accept_remote(&mut self) -> Result<VersionedViewState, String> {
        let current = self.conflict.take().ok_or("解決する競合がありません")?;
        self.confirmed = Some(current.clone());
        self.pending = None;
        Ok(current)
    }

    pub fn prepare_retry(&mut self) -> Result<ConversationViewState, String> {
        let current = self.conflict.clone().ok_or("解決する競合がありません")?;
        let pending = self
            .pending
            .clone()
            .ok_or("未確定のWorkbench変更がありません")?;
        self.conflict = None;
        self.confirmed = Some(current);
        Ok(pending)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_initializes_an_uninitialized_replica() {
        let mut replica = WorkbenchReplica::default();
        let snapshot = VersionedViewState::default();

        assert_eq!(
            replica.refresh(snapshot.clone()).unwrap(),
            Some(snapshot.clone())
        );

        assert_eq!(replica.confirmed(), Some(&snapshot));
        assert!(replica.pending().is_none());
        assert!(replica.conflict().is_none());
    }

    #[test]
    fn stage_before_initialization_does_not_leave_pending() {
        let mut replica = WorkbenchReplica::default();

        assert!(replica.stage(ConversationViewState::default()).is_err());
        assert!(replica.pending().is_none());
    }

    #[test]
    fn invalid_update_response_preserves_pending_view() {
        let mut replica = WorkbenchReplica::default();
        replica.open(VersionedViewState::default()).unwrap();
        let pending = ConversationViewState {
            show_panel_controls: false,
            ..Default::default()
        };
        replica.stage(pending.clone()).unwrap();
        let invalid = VersionedViewState {
            revision: 1,
            view: Some(ConversationViewState::default()),
        };
        assert!(replica.finish(WorkbenchUpdate::Updated(invalid)).is_err());
        assert_eq!(replica.pending(), Some(&pending));
        assert_eq!(replica.confirmed().unwrap().revision, 0);
    }

    #[test]
    fn stale_conflict_response_preserves_pending_view() {
        let mut replica = WorkbenchReplica::default();
        replica.open(VersionedViewState::default()).unwrap();
        let pending = ConversationViewState::default();
        replica.stage(pending.clone()).unwrap();
        assert!(
            replica
                .finish(WorkbenchUpdate::Conflict(VersionedViewState::default()))
                .is_err()
        );
        assert_eq!(replica.pending(), Some(&pending));
    }

    #[test]
    fn matching_host_state_confirms_a_write_whose_response_was_lost() {
        let mut replica = WorkbenchReplica::default();
        replica.open(VersionedViewState::default()).unwrap();
        let pending = ConversationViewState::default();
        replica.stage(pending.clone()).unwrap();
        let saved = VersionedViewState {
            revision: 1,
            view: Some(pending),
        };
        assert_eq!(replica.refresh(saved.clone()).unwrap(), Some(saved.clone()));
        assert!(replica.pending().is_none());
        assert!(replica.conflict().is_none());
        assert_eq!(replica.confirmed(), Some(&saved));
    }

    #[test]
    fn inconsistent_snapshot_does_not_discard_pending_view() {
        let mut replica = WorkbenchReplica::default();
        replica.open(VersionedViewState::default()).unwrap();
        let pending = ConversationViewState::default();
        replica.stage(pending.clone()).unwrap();
        assert!(
            replica
                .refresh(VersionedViewState {
                    revision: 2,
                    view: None
                })
                .is_err()
        );
        assert_eq!(replica.pending(), Some(&pending));
        assert!(replica.conflict().is_none());
    }

    #[test]
    fn conflict_keeps_pending_until_local_changes_are_retried() {
        let mut replica = WorkbenchReplica::default();
        replica.open(VersionedViewState::default()).unwrap();
        let pending = ConversationViewState {
            show_panel_controls: false,
            ..Default::default()
        };
        replica.stage(pending.clone()).unwrap();
        let remote = VersionedViewState {
            revision: 1,
            view: Some(ConversationViewState::default()),
        };
        match replica
            .finish(WorkbenchUpdate::Conflict(remote.clone()))
            .unwrap()
        {
            WorkbenchUpdate::Conflict(snapshot) => assert_eq!(snapshot, remote),
            WorkbenchUpdate::Updated(_) => panic!("競合応答がUpdatedになりました"),
        }
        assert_eq!(replica.pending(), Some(&pending));
        assert_eq!(replica.conflict(), Some(&remote));

        let retried = replica.prepare_retry().unwrap();

        assert_eq!(retried, pending);
        assert_eq!(replica.confirmed(), Some(&remote));
        assert!(replica.conflict().is_none());
        assert!(replica.stage(retried).is_ok());
    }

    #[test]
    fn accept_remote_discards_pending_and_conflict() {
        let mut replica = WorkbenchReplica::default();
        replica.open(VersionedViewState::default()).unwrap();
        replica.stage(ConversationViewState::default()).unwrap();
        let remote = VersionedViewState {
            revision: 1,
            view: Some(ConversationViewState::default()),
        };
        replica
            .finish(WorkbenchUpdate::Conflict(remote.clone()))
            .unwrap();

        assert_eq!(replica.accept_remote().unwrap(), remote);
        assert!(replica.pending().is_none());
        assert!(replica.conflict().is_none());
        assert_eq!(replica.confirmed(), Some(&remote));
    }
}
