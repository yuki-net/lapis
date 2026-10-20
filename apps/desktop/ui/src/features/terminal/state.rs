use std::collections::HashMap;

use lapis_desktop_client::{RestoredTerminal, TerminalSession};
use lapis_editor_core::ConversationId;
use lapis_terminal::TerminalError;
#[cfg(test)]
use lapis_terminal::TerminalStatus;

pub(crate) struct TerminalSessions {
    pub session: TerminalSession,
    active_conversation: ConversationId,
    inactive_sessions: HashMap<ConversationId, TerminalSession>,
}

impl TerminalSessions {
    pub fn new(session: TerminalSession, conversation: ConversationId) -> Self {
        Self {
            session,
            active_conversation: conversation,
            inactive_sessions: HashMap::new(),
        }
    }

    pub fn switch_conversation(
        &mut self,
        conversation: ConversationId,
        summaries: &[RestoredTerminal],
    ) {
        if self.active_conversation == conversation {
            return;
        }
        let next = self
            .inactive_sessions
            .remove(&conversation)
            .unwrap_or_else(|| {
                let mut session = TerminalSession::new(self.session.backend());
                session.restore_summaries(summaries);
                session
            });
        let previous = std::mem::replace(&mut self.session, next);
        let previous_id = std::mem::replace(&mut self.active_conversation, conversation);
        self.inactive_sessions.insert(previous_id, previous);
    }

    pub fn refresh(&mut self) -> Result<bool, TerminalError> {
        let mut changed = false;
        let mut first_error = None;
        for session in std::iter::once(&mut self.session).chain(self.inactive_sessions.values_mut())
        {
            match session.refresh() {
                Ok(updated) => changed |= updated,
                Err(error) if first_error.is_none() => first_error = Some(error),
                Err(_) => {}
            }
        }
        first_error.map_or(Ok(changed), Err)
    }

    fn action(
        &mut self,
        operation: impl FnOnce(&mut TerminalSession) -> Result<(), TerminalError>,
    ) -> (Result<(), String>, Vec<lapis_terminal::TerminalSnapshot>) {
        let result = operation(&mut self.session).map_err(|error| error.to_string());
        (result, self.session.terminals().to_vec())
    }

    #[cfg(test)]
    pub fn has_running_process(&self) -> bool {
        std::iter::once(&self.session)
            .chain(self.inactive_sessions.values())
            .flat_map(TerminalSession::terminals)
            .any(|terminal| terminal.status == TerminalStatus::Running)
    }
}

#[derive(Default)]
struct TerminalRequests {
    gate: crate::client_adapter::ResponseGate,
    poll_pending: bool,
    pending_actions: usize,
}

impl TerminalRequests {
    fn begin_action(&mut self) -> crate::client_worker::RequestToken {
        self.pending_actions += 1;
        self.gate.issue()
    }

    fn finish_action(&mut self, token: crate::client_worker::RequestToken) -> bool {
        self.pending_actions = self.pending_actions.saturating_sub(1);
        self.gate.accept(token)
    }

    fn begin_poll(&mut self) -> Option<crate::client_worker::RequestToken> {
        if self.poll_pending || self.pending_actions > 0 {
            return None;
        }
        self.poll_pending = true;
        Some(self.gate.issue())
    }

    fn finish_poll(&mut self, token: crate::client_worker::RequestToken) -> bool {
        self.poll_pending = false;
        self.gate.accept(token)
    }
}

pub(crate) struct TerminalFeature {
    worker: crate::client_adapter::ClientWorker<TerminalSessions>,
    pub(super) terminals: Vec<lapis_terminal::TerminalSnapshot>,
    pub(super) root: Option<std::path::PathBuf>,
    conversation: ConversationId,
    requests: TerminalRequests,
    pub(super) message: String,
    message_from_poll: bool,
    pub(super) scroll: crate::components::ScrollState,
}
impl TerminalFeature {
    pub(crate) fn summaries(&self) -> Vec<lapis_terminal::TerminalSnapshot> {
        self.terminals.clone()
    }
    pub(crate) fn new(
        session: TerminalSession,
        conversation: ConversationId,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        let sessions = TerminalSessions::new(session, conversation.clone());
        let feature = Self {
            worker: crate::client_adapter::ClientWorker::new(sessions),
            terminals: Vec::new(),
            root: None,
            conversation,
            requests: TerminalRequests::default(),
            message: String::new(),
            message_from_poll: false,
            scroll: crate::components::ScrollState::new(),
        };
        cx.spawn(async move |this, cx| {
            loop {
                gpui::Timer::after(std::time::Duration::from_millis(250)).await;
                if this.update(cx, |feature, cx| feature.poll(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        feature
    }
    pub(crate) fn set_context(
        &mut self,
        root: Option<std::path::PathBuf>,
        conversation: ConversationId,
        summaries: Vec<RestoredTerminal>,
        cx: &mut gpui::Context<Self>,
    ) {
        if self.conversation == conversation {
            self.root = root;
            return;
        }
        let next = conversation.clone();
        if let Err(error) = self.worker.try_request(move |sessions| {
            sessions.switch_conversation(next, &summaries);
            Ok(())
        }) {
            self.root = None;
            self.message = error;
            self.message_from_poll = false;
            cx.notify();
            return;
        }
        self.root = root;
        self.conversation = conversation;
        self.requests.gate.invalidate();
        self.terminals.clear();
        cx.notify();
    }
    pub(crate) fn start(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let token = self.requests.begin_action();
        let admitted = self.worker.submit(
            cx,
            move |sessions| Ok(sessions.action(|session| session.start(&root, 80, 24).map(|_| ()))),
            move |feature, result, cx| {
                if !feature.requests.finish_action(token) {
                    return;
                }
                match result {
                    Ok((result, terminals)) => {
                        feature.terminals = terminals;
                        feature.message = result.err().unwrap_or_default();
                        feature.message_from_poll = false;
                    }
                    Err(error) => {
                        feature.message = error;
                        feature.message_from_poll = false;
                    }
                }
                cx.notify();
            },
        );
        if let Err(error) = admitted {
            self.requests.finish_action(token);
            self.message = error;
            self.message_from_poll = false;
            cx.notify();
        }
    }
    pub(crate) fn send_clipboard(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(id) = self.terminals.last().map(|terminal| terminal.id.clone()) else {
            return;
        };
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let token = self.requests.begin_action();
        let admitted = self.worker.submit(
            cx,
            move |sessions| Ok(sessions.action(|session| session.input(&id, &text))),
            move |feature, result, cx| {
                if feature.requests.finish_action(token) {
                    match result {
                        Ok((outcome, terminals)) => {
                            feature.terminals = terminals;
                            feature.message = outcome.err().unwrap_or_default();
                        }
                        Err(error) => feature.message = error,
                    }
                    feature.message_from_poll = false;
                    cx.notify();
                }
            },
        );
        if let Err(error) = admitted {
            self.requests.finish_action(token);
            self.message = error;
            self.message_from_poll = false;
            cx.notify();
        }
    }
    fn poll(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(token) = self.requests.begin_poll() else {
            return;
        };
        let admitted = self.worker.submit(
            cx,
            |sessions| {
                let changed = sessions.refresh().map_err(|error| error.to_string())?;
                Ok(changed.then(|| sessions.session.terminals().to_vec()))
            },
            move |feature, result, cx| {
                if !feature.requests.finish_poll(token) {
                    return;
                }
                match result {
                    Ok(Some(terminals)) => {
                        feature.terminals = terminals;
                        if feature.message_from_poll {
                            feature.message.clear();
                            feature.message_from_poll = false;
                        }
                        cx.notify();
                    }
                    Ok(None) => {
                        if feature.message_from_poll {
                            feature.message.clear();
                            feature.message_from_poll = false;
                            cx.notify();
                        }
                    }
                    Err(error) => {
                        feature.message = error;
                        feature.message_from_poll = true;
                        cx.notify();
                    }
                }
            },
        );
        if let Err(error) = admitted {
            self.requests.finish_poll(token);
            self.message = error;
            self.message_from_poll = true;
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lapis_terminal::TerminalId;
    use lapis_terminal::{TerminalBackend, TerminalEvent, TerminalOutput};
    use std::{
        path::Path,
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };

    #[test]
    fn pending_operation_errors_cannot_be_superseded_by_periodic_polling() {
        for message in ["PTY start failed", "terminal input failed"] {
            let worker = crate::client_adapter::ClientWorker::new(());
            let mut requests = TerminalRequests::default();
            let token = requests.begin_action();
            let receive = worker
                .try_request(move |_| Err::<(), _>(message.to_owned()))
                .unwrap();
            let result = receive
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap();
            assert!(requests.begin_poll().is_none());
            assert!(requests.finish_action(token));
            assert_eq!(result.unwrap_err(), message);
            let poll = requests.begin_poll().unwrap();
            assert!(requests.finish_poll(poll));
        }
    }

    #[test]
    fn rejected_or_stale_actions_release_poll_suppression_only_after_all_callbacks_finish() {
        let mut requests = TerminalRequests::default();
        let first = requests.begin_action();
        let second = requests.begin_action();
        assert!(!requests.finish_action(first));
        assert!(requests.begin_poll().is_none());
        requests.gate.invalidate();
        assert!(!requests.finish_action(second));
        assert_eq!(requests.pending_actions, 0);
        let poll = requests.begin_poll().unwrap();
        assert!(requests.begin_poll().is_none());
        assert!(requests.finish_poll(poll));
        let rejected = requests.begin_action();
        assert!(requests.finish_action(rejected));
        assert_eq!(requests.pending_actions, 0);
        assert!(requests.begin_poll().is_some());
    }

    #[test]
    fn action_snapshots_retain_output_consumed_by_a_superseded_poll_even_on_failure() {
        let (mut sessions, backend) = feature();
        let id = sessions
            .session
            .start(Path::new("workspace"), 80, 24)
            .unwrap();
        backend.events.lock().unwrap().insert(
            id.as_str().to_owned(),
            vec![TerminalEvent::Output(TerminalOutput {
                sequence: 1,
                data: b"background output".to_vec(),
            })],
        );
        let mut requests = TerminalRequests::default();
        let poll = requests.begin_poll().unwrap();
        assert!(sessions.refresh().unwrap());
        let action = requests.begin_action();
        let (outcome, terminals) = sessions.action(|_| Err(TerminalError::new("input failed")));
        assert!(!requests.finish_poll(poll));
        assert!(requests.finish_action(action));
        assert_eq!(outcome.unwrap_err(), "input failed");
        assert_eq!(terminals[0].output, b"background output");
        assert!(!sessions.refresh().unwrap());
    }

    #[test]
    fn a_pending_poll_cannot_overwrite_a_later_user_action() {
        let mut requests = TerminalRequests::default();
        let poll = requests.begin_poll().unwrap();
        let action = requests.begin_action();
        assert!(!requests.finish_poll(poll));
        assert!(requests.begin_poll().is_none());
        assert!(requests.finish_action(action));
        let next_poll = requests.begin_poll().unwrap();
        assert!(requests.finish_poll(next_poll));
    }

    #[derive(Default)]
    struct Backend {
        next: AtomicUsize,
        terminated: AtomicUsize,
        events: Mutex<HashMap<String, Vec<TerminalEvent>>>,
    }

    impl TerminalBackend for Backend {
        fn start(&self, _: &Path, _: u16, _: u16) -> Result<TerminalId, TerminalError> {
            Ok(TerminalId::new(format!(
                "terminal-{}",
                self.next.fetch_add(1, Ordering::Relaxed)
            )))
        }
        fn input(&self, _: &TerminalId, _: &[u8]) -> Result<(), TerminalError> {
            Ok(())
        }
        fn resize(&self, _: &TerminalId, _: u16, _: u16) -> Result<(), TerminalError> {
            Ok(())
        }
        fn poll(&self, id: &TerminalId) -> Result<Vec<TerminalEvent>, TerminalError> {
            Ok(self
                .events
                .lock()
                .unwrap()
                .remove(id.as_str())
                .unwrap_or_default())
        }
        fn terminate(&self, _: &TerminalId) -> Result<(), TerminalError> {
            self.terminated.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    fn feature() -> (TerminalSessions, Arc<Backend>) {
        let backend = Arc::new(Backend::default());
        (
            TerminalSessions::new(
                TerminalSession::new(backend.clone()),
                ConversationId::new("first"),
            ),
            backend,
        )
    }

    #[test]
    fn switching_conversations_retains_live_terminal_identity() {
        let (mut feature, backend) = feature();
        let terminal = feature
            .session
            .start(Path::new("workspace"), 80, 24)
            .unwrap();
        feature.switch_conversation(ConversationId::new("second"), &[]);
        assert!(feature.session.terminals().is_empty());
        assert!(feature.has_running_process());
        feature.switch_conversation(ConversationId::new("first"), &[]);
        assert_eq!(feature.session.terminals()[0].id, terminal);
        assert_eq!(
            feature.session.terminals()[0].status,
            TerminalStatus::Running
        );
        assert_eq!(backend.terminated.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn inactive_terminal_output_and_exit_are_consumed() {
        let (mut feature, backend) = feature();
        let terminal = feature
            .session
            .start(Path::new("workspace"), 80, 24)
            .unwrap();
        feature.switch_conversation(ConversationId::new("second"), &[]);
        backend.events.lock().unwrap().insert(
            terminal.as_str().to_owned(),
            vec![
                TerminalEvent::Output(TerminalOutput {
                    sequence: 1,
                    data: b"background".to_vec(),
                }),
                TerminalEvent::Exited { code: Some(0) },
            ],
        );
        assert!(feature.refresh().unwrap());
        feature.switch_conversation(ConversationId::new("first"), &[]);
        assert_eq!(feature.session.terminals()[0].output, b"background");
        assert_eq!(
            feature.session.terminals()[0].status,
            TerminalStatus::Exited
        );
    }

    #[test]
    fn summaries_never_replace_live_sessions_and_drop_is_not_termination() {
        let (mut feature, backend) = feature();
        let terminal = feature
            .session
            .start(Path::new("workspace"), 80, 24)
            .unwrap();
        let summaries = [RestoredTerminal {
            cwd: "old".into(),
            status: TerminalStatus::Running,
            columns: 40,
            rows: 10,
        }];
        feature.switch_conversation(ConversationId::new("second"), &summaries);
        assert_eq!(
            feature.session.terminals()[0].status,
            TerminalStatus::Exited
        );
        feature.switch_conversation(ConversationId::new("first"), &summaries);
        assert_eq!(feature.session.terminals()[0].id, terminal);
        drop(feature);
        assert_eq!(backend.terminated.load(Ordering::Relaxed), 0);
    }
}
