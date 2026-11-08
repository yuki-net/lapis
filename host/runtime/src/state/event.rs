//! Event 連番・保持と revision 検証。Runtime の再利用層。

use std::collections::VecDeque;

use client_api::{DocumentId, EventBody, EventEnvelope, EventSequence, Revision, RevisionConflict};

use crate::BackendStateError;

/// 保持する event の上限。古い event から捨てる。
pub(crate) const MAX_RETAINED_EVENTS: usize = 1024;

#[derive(Default)]
pub(crate) struct EventJournal {
    next_sequence: EventSequence,
    events: VecDeque<EventEnvelope>,
}

impl EventJournal {
    pub(crate) fn push(&mut self, body: EventBody) -> Result<EventEnvelope, BackendStateError> {
        let sequence = self
            .next_sequence
            .checked_next()
            .ok_or(BackendStateError::CounterOverflow)?;
        self.next_sequence = sequence;
        let event = EventEnvelope {
            event_sequence: sequence,
            body,
        };
        self.events.push_back(event.clone());
        if self.events.len() > MAX_RETAINED_EVENTS {
            self.events.pop_front();
        }
        Ok(event)
    }

    pub(crate) fn watermark(&self) -> EventSequence {
        self.next_sequence
    }

    #[cfg(test)]
    pub(crate) fn retained(&self) -> &VecDeque<EventEnvelope> {
        &self.events
    }
}

/// 期待 revision と現在値が一致することを要求する。
pub(crate) fn require_revision(
    document_id: &DocumentId,
    expected: Revision,
    actual: Revision,
) -> Result<(), BackendStateError> {
    if expected == actual {
        Ok(())
    } else {
        Err(BackendStateError::RevisionConflict(RevisionConflict {
            document_id: document_id.clone(),
            expected,
            actual,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use client_api::WorkspaceId;

    #[test]
    fn sequence_increments_and_watermark_tracks_last_event() {
        let mut journal = EventJournal::default();
        let workspace_id = WorkspaceId::try_new("workspace-1").unwrap();
        let first = journal
            .push(EventBody::WorkspaceChanged {
                workspace_id: workspace_id.clone(),
            })
            .unwrap();
        let second = journal
            .push(EventBody::WorkspaceChanged { workspace_id })
            .unwrap();
        assert_eq!(first.event_sequence, EventSequence::new(1));
        assert_eq!(second.event_sequence, EventSequence::new(2));
        assert_eq!(journal.watermark(), EventSequence::new(2));
    }

    #[test]
    fn retained_events_are_capped() {
        let mut journal = EventJournal::default();
        let workspace_id = WorkspaceId::try_new("workspace-1").unwrap();
        for _ in 0..(MAX_RETAINED_EVENTS + 8) {
            journal
                .push(EventBody::WorkspaceChanged {
                    workspace_id: workspace_id.clone(),
                })
                .unwrap();
        }
        assert_eq!(journal.retained().len(), MAX_RETAINED_EVENTS);
        assert_eq!(
            journal.retained().front().unwrap().event_sequence,
            EventSequence::new(9)
        );
    }

    #[test]
    fn revision_mismatch_reports_both_values() {
        let document_id = DocumentId::try_new("document-1").unwrap();
        let error = require_revision(&document_id, Revision::new(2), Revision::new(3)).unwrap_err();
        match error {
            BackendStateError::RevisionConflict(conflict) => {
                assert_eq!(conflict.expected, Revision::new(2));
                assert_eq!(conflict.actual, Revision::new(3));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
