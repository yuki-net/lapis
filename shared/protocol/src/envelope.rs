use serde::{Deserialize, Serialize};

use crate::{EventBody, EventSequence, RequestBody, RequestId, ResponseBody};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestEnvelope {
    pub request_id: RequestId,
    #[serde(flatten)]
    pub body: RequestBody,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseEnvelope {
    pub request_id: RequestId,
    #[serde(flatten)]
    pub body: ResponseBody,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub event_sequence: EventSequence,
    #[serde(flatten)]
    pub body: EventBody,
}

impl ResponseEnvelope {
    pub fn matches_request(&self, request: &RequestEnvelope) -> bool {
        self.request_id == request.request_id
            && self
                .body
                .method()
                .is_none_or(|method| method == request.body.method())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        DocumentEditRequest, DocumentId, DocumentTextEdit, DocumentTransaction, EventBody,
        TerminalId, WorkspaceId, WorkspaceListRequest,
    };

    #[test]
    fn request_envelope_has_stable_external_tag() {
        let request = RequestEnvelope {
            request_id: RequestId::try_new("request-1").unwrap(),
            body: RequestBody::WorkspaceList(WorkspaceListRequest::default()),
        };

        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(
            json,
            r#"{"request_id":"request-1","type":"workspace.list","payload":{}}"#
        );
        assert_eq!(
            serde_json::from_str::<RequestEnvelope>(&json).unwrap(),
            request
        );
    }

    #[test]
    fn response_keeps_request_id_when_returning_an_error() {
        let response = ResponseEnvelope {
            request_id: RequestId::try_new("request-2").unwrap(),
            body: ResponseBody::Error(crate::ProtocolError::new(
                crate::ErrorCode::try_new(crate::INVALID_REQUEST).unwrap(),
            )),
        };

        let restored =
            serde_json::from_str::<ResponseEnvelope>(&serde_json::to_string(&response).unwrap())
                .unwrap();

        assert_eq!(
            restored.request_id,
            RequestId::try_new("request-2").unwrap()
        );
        assert!(matches!(restored.body, ResponseBody::Error(_)));
    }

    #[test]
    fn event_round_trip_preserves_sequence_and_typed_workspace_id() {
        let event = EventEnvelope {
            event_sequence: EventSequence::new(7),
            body: EventBody::WorkspaceChanged {
                workspace_id: WorkspaceId::try_new("workspace-1").unwrap(),
            },
        };

        let restored =
            serde_json::from_str::<EventEnvelope>(&serde_json::to_string(&event).unwrap()).unwrap();

        assert_eq!(restored, event);
    }

    #[test]
    fn document_request_round_trip_preserves_base_revision() {
        let request = RequestEnvelope {
            request_id: RequestId::try_new("request-3").unwrap(),
            body: RequestBody::DocumentEdit(DocumentEditRequest {
                document_id: DocumentId::try_new("document-1").unwrap(),
                base_revision: crate::Revision::new(12),
                transaction: DocumentTransaction::try_new(vec![
                    DocumentTextEdit::try_new(0, 0, "updated").unwrap(),
                ])
                .unwrap(),
            }),
        };

        let restored =
            serde_json::from_str::<RequestEnvelope>(&serde_json::to_string(&request).unwrap())
                .unwrap();

        assert_eq!(restored, request);
        let RequestBody::DocumentEdit(edit) = restored.body else {
            panic!("expected document edit request");
        };
        assert_eq!(edit.base_revision, crate::Revision::new(12));
        assert_eq!(edit.transaction.edits()[0].replacement(), "updated");
    }

    #[test]
    fn terminal_delta_event_keeps_resource_and_revision() {
        let event = EventEnvelope {
            event_sequence: EventSequence::new(21),
            body: EventBody::TerminalDelta {
                terminal_id: TerminalId::try_new("terminal-1").unwrap(),
                delta: crate::TerminalDelta {
                    revision: 4,
                    size: crate::TerminalSize::new(80, 24),
                    changed_rows: Vec::new(),
                    cursor: crate::TerminalCursor::default(),
                    modes: crate::TerminalModes::default(),
                    scrollback: Vec::new(),
                },
            },
        };

        let restored =
            serde_json::from_str::<EventEnvelope>(&serde_json::to_string(&event).unwrap()).unwrap();

        assert_eq!(restored, event);
        let EventBody::TerminalDelta { delta, .. } = restored.body else {
            panic!("expected terminal delta event");
        };
        assert_eq!(delta.revision, 4);
    }

    #[test]
    fn document_event_carries_delta_between_revisions() {
        let event = EventEnvelope {
            event_sequence: EventSequence::new(22),
            body: EventBody::DocumentEdited {
                document_id: DocumentId::try_new("document-1").unwrap(),
                base_revision: crate::Revision::new(5),
                revision: crate::Revision::new(6),
                transaction: DocumentTransaction::try_new(vec![
                    DocumentTextEdit::try_new(3, 5, "new").unwrap(),
                ])
                .unwrap(),
            },
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"document.edited""#));
        assert_eq!(serde_json::from_str::<EventEnvelope>(&json).unwrap(), event);
    }

    #[test]
    fn response_must_match_request_id_and_method() {
        let request = RequestEnvelope {
            request_id: RequestId::try_new("request-4").unwrap(),
            body: RequestBody::WorkspaceList(WorkspaceListRequest::default()),
        };
        let mismatched = ResponseEnvelope {
            request_id: request.request_id.clone(),
            body: ResponseBody::TerminalInput(crate::TerminalCommandResponse {
                terminal: crate::TerminalSnapshot {
                    terminal_id: TerminalId::try_new("terminal-1").unwrap(),
                    workspace_id: WorkspaceId::try_new("workspace-1").unwrap(),
                    cwd: None,
                    status: crate::TerminalStatus::Running,
                    revision: 0,
                    screen: crate::TerminalScreen::empty(crate::TerminalSize::new(80, 24)),
                },
            }),
        };

        assert!(!mismatched.matches_request(&request));
    }
}
