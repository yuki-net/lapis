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
        if self.pending.is_some() {
            self.conflict = Some(snapshot);
            return Ok(None);
        }
        self.confirmed = Some(snapshot.clone());
        self.conflict = None;
        Ok(Some(snapshot))
    }

    pub fn stage(&mut self, view: ConversationViewState) -> Result<WorkbenchWritePlan, String> {
        self.pending = Some(view.clone());
        let revision = self
            .confirmed
            .as_ref()
            .ok_or("Workbenchが初期化されていません")?
            .revision;
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
}
