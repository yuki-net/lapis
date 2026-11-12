//! Workspace を開いている Client 間で共有する view state。
//!
//! 真実は Host にあり、更新は直列化して last-writer-wins で置き換える。
//! マージは行わず、最後に受理した値をそのまま配る。

use std::collections::HashSet;

use client_api::{SessionId, WorkspaceView, WorkspaceViewSnapshot};

use crate::BackendStateError;

/// Workspace 単位で保持する最後の view state と共有への参加者。
#[derive(Default)]
pub(crate) struct SessionViewState {
    revision: u64,
    view: Option<WorkspaceView>,
    participants: HashSet<SessionId>,
}

/// 受理した更新。共有中の Client がいる場合だけ配信用の値を返す。
pub(crate) struct SessionViewUpdate {
    pub(crate) revision: u64,
    pub(crate) broadcast: Option<WorkspaceView>,
}

impl SessionViewState {
    pub(crate) fn snapshot(&self) -> WorkspaceViewSnapshot {
        WorkspaceViewSnapshot {
            revision: self.revision,
            view: self.view.clone(),
        }
    }

    /// 共有へ参加し、参加時点の snapshot を返す。
    pub(crate) fn subscribe(&mut self, session_id: &SessionId) -> WorkspaceViewSnapshot {
        self.participants.insert(session_id.clone());
        self.snapshot()
    }

    pub(crate) fn unsubscribe(&mut self, session_id: &SessionId) {
        self.participants.remove(session_id);
    }

    /// 共有に参加している Client がいない間は event を積まない。
    /// 誰も見ていない更新で workspace の event 連番を消費しないため。
    pub(crate) fn update(
        &mut self,
        view: WorkspaceView,
    ) -> Result<SessionViewUpdate, BackendStateError> {
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(BackendStateError::CounterOverflow)?;
        self.revision = revision;
        self.view = Some(view);
        let broadcast = if self.participants.is_empty() {
            None
        } else {
            self.view.clone()
        };
        Ok(SessionViewUpdate {
            revision,
            broadcast,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use client_api::WorkspaceRelativePath;

    fn view(path: &str) -> WorkspaceView {
        WorkspaceView {
            open_documents: Vec::new(),
            active_path: Some(WorkspaceRelativePath::parse(path).unwrap()),
            active_document_id: None,
        }
    }

    #[test]
    fn subscribe_returns_the_last_accepted_view() {
        let mut state = SessionViewState::default();
        let session = SessionId::try_new("session-1").unwrap();
        assert_eq!(
            state.snapshot(),
            WorkspaceViewSnapshot {
                revision: 0,
                view: None
            }
        );

        state.subscribe(&session);
        state.update(view("src/main.rs")).unwrap();

        let snapshot = state.snapshot();
        assert_eq!(snapshot.revision, 1);
        assert_eq!(
            snapshot.view.unwrap().active_path,
            Some(WorkspaceRelativePath::parse("src/main.rs").unwrap())
        );
    }

    #[test]
    fn updates_are_only_broadcast_while_someone_participates() {
        let mut state = SessionViewState::default();
        let session = SessionId::try_new("session-1").unwrap();

        assert!(state.update(view("a.rs")).unwrap().broadcast.is_none());

        state.subscribe(&session);
        let update = state.update(view("b.rs")).unwrap();
        assert_eq!(update.revision, 2);
        assert_eq!(
            update.broadcast.unwrap().active_path,
            Some(WorkspaceRelativePath::parse("b.rs").unwrap())
        );

        state.unsubscribe(&session);
        assert!(state.update(view("c.rs")).unwrap().broadcast.is_none());
        assert_eq!(state.snapshot().revision, 3);
    }

    #[test]
    fn revision_overflow_does_not_replace_the_accepted_view() {
        let mut state = SessionViewState {
            revision: u64::MAX,
            ..Default::default()
        };
        let previous = state.snapshot();

        assert!(state.update(view("a.rs")).is_err());
        assert_eq!(state.snapshot(), previous);
    }
}
