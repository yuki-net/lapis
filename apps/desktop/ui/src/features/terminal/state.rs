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

    #[cfg(test)]
    pub fn has_running_process(&self) -> bool {
        std::iter::once(&self.session)
            .chain(self.inactive_sessions.values())
            .flat_map(TerminalSession::terminals)
            .any(|terminal| terminal.status == TerminalStatus::Running)
    }
}

pub(crate) struct TerminalFeature {
    worker: crate::client_adapter::ClientWorker<TerminalSessions>,
    pub(super) terminals: Vec<lapis_terminal::TerminalSnapshot>,
    pub(super) root: Option<std::path::PathBuf>,
    conversation: ConversationId,
    gate: crate::client_adapter::ResponseGate,
    busy: bool,
    pub(super) message: String,
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
            gate: crate::client_adapter::ResponseGate::default(),
            busy: false,
            message: String::new(),
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
            cx.notify();
            return;
        }
        self.root = root;
        self.conversation = conversation;
        self.gate.invalidate();
        self.terminals.clear();
        cx.notify();
    }
    pub(crate) fn start(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let token = self.gate.issue();
        let admitted = self.worker.submit(
            cx,
            move |sessions| {
                let result = sessions.session.start(&root, 80, 24);
                result.map_err(|error| error.to_string())?;
                Ok(sessions.session.terminals().to_vec())
            },
            move |feature, result, cx| {
                if !feature.gate.accept(token) {
                    return;
                }
                match result {
                    Ok(terminals) => {
                        feature.terminals = terminals;
                        feature.message.clear();
                    }
                    Err(error) => feature.message = error,
                }
                cx.notify();
            },
        );
        if let Err(error) = admitted {
            self.busy = false;
            self.message = error;
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
        let token = self.gate.issue();
        let admitted = self.worker.submit(
            cx,
            move |sessions| {
                sessions
                    .session
                    .input(&id, &text)
                    .map_err(|error| error.to_string())
            },
            move |feature, result, cx| {
                if feature.gate.accept(token) {
                    feature.message = result.err().unwrap_or_default();
                    cx.notify();
                }
            },
        );
        if let Err(error) = admitted {
            self.message = error;
            cx.notify();
        }
    }
    fn poll(&mut self, cx: &mut gpui::Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        let token = self.gate.issue();
        let admitted = self.worker.submit(
            cx,
            |sessions| {
                let result = sessions.refresh();
                result.map_err(|error| error.to_string())?;
                Ok(sessions.session.terminals().to_vec())
            },
            move |feature, result, cx| {
                feature.busy = false;
                if !feature.gate.accept(token) {
                    return;
                }
                match result {
                    Ok(terminals) => {
                        feature.terminals = terminals;
                        feature.message.clear();
                    }
                    Err(error) => feature.message = error,
                }
                cx.notify();
            },
        );
        if let Err(error) = admitted {
            self.busy = false;
            self.message = error;
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
