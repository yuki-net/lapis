//! Web の Terminal UI に渡す表示状態とイベント。

use terminal::{TerminalEvent, TerminalScreen, TerminalSnapshot, TerminalStatus};

pub(crate) struct Projection {
    pub snapshot: TerminalSnapshot,
    owner: Option<String>,
    pub events: Vec<TerminalEvent>,
    pub finished: bool,
}

impl Projection {
    pub fn new(snapshot: TerminalSnapshot) -> Self {
        Self {
            snapshot,
            owner: None,
            events: Vec::new(),
            finished: false,
        }
    }

    pub fn update(
        &mut self,
        screen: TerminalScreen,
        owner: Option<String>,
        exit: Option<Option<i32>>,
    ) -> bool {
        if self.finished {
            return false;
        }
        let mut changed = false;
        if self.snapshot.screen != screen {
            self.snapshot.revision += 1;
            self.snapshot.screen = screen;
            // 非表示タブでも出力は続くため、最新 snapshot だけを保持する。
            self.events
                .retain(|event| !matches!(event, TerminalEvent::Snapshot(_)));
            self.events
                .push(TerminalEvent::Snapshot(self.snapshot.clone()));
            changed = true;
        }
        if self.owner != owner {
            self.owner = owner.clone();
            self.events
                .retain(|event| !matches!(event, TerminalEvent::Owner { .. }));
            self.events.push(TerminalEvent::Owner { owner });
            changed = true;
        }
        if let Some(code) = exit {
            self.finished = true;
            self.snapshot.status = TerminalStatus::Exited;
            self.events.push(TerminalEvent::Exited { code });
            changed = true;
        }
        changed
    }

    pub fn fail(&mut self, error: String) {
        if !self.finished {
            self.finished = true;
            self.snapshot.status = TerminalStatus::Failed;
            self.events.push(TerminalEvent::Failed(error));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use terminal::{TerminalId, TerminalSize};

    #[test]
    fn delivers_changed_screen_owner_and_exit_in_order() {
        let snapshot = TerminalSnapshot::empty(
            TerminalId::new("client-id"),
            ".".into(),
            TerminalStatus::Running,
            TerminalSize::new(8, 2),
        );
        let mut state = Projection::new(snapshot.clone());
        assert!(!state.update(snapshot.screen.clone(), None, None));
        let mut screen = snapshot.screen;
        screen.rows[0].cells[0].text = "x".into();
        assert!(state.update(screen.clone(), Some("owner".into()), None));
        assert!(
            matches!(&state.events[0], TerminalEvent::Snapshot(snapshot) if snapshot.revision == 1)
        );
        assert!(
            matches!(&state.events[1], TerminalEvent::Owner { owner: Some(owner) } if owner == "owner")
        );
        assert!(!state.update(screen.clone(), Some("owner".into()), None));
        assert!(state.update(screen.clone(), None, Some(Some(0))));
        assert_eq!(state.snapshot.status, TerminalStatus::Exited);
        assert!(!state.update(screen, Some("late".into()), None));
        state.fail("late".into());
        assert_eq!(state.events.len(), 3);
    }

    #[test]
    fn stalled_view_keeps_only_latest_snapshot_and_owner() {
        let snapshot = TerminalSnapshot::empty(
            TerminalId::new("id"),
            ".".into(),
            TerminalStatus::Running,
            TerminalSize::new(8, 2),
        );
        let mut state = Projection::new(snapshot.clone());
        for value in 0..1000 {
            let mut screen = snapshot.screen.clone();
            screen.rows[0].cells[0].text = value.to_string();
            state.update(screen, Some(value.to_string()), None);
        }
        assert_eq!(state.events.len(), 2);
        assert_eq!(state.snapshot.revision, 1000);
        assert!(
            matches!(&state.events[1], TerminalEvent::Owner { owner: Some(owner) } if owner == "999")
        );
    }
}
