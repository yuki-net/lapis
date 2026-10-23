use std::collections::HashMap;
use std::path::PathBuf;

use lapis_desktop_client::{RestoredTerminal, TerminalSession};
use lapis_editor_core::ConversationId;
#[cfg(test)]
use lapis_terminal::TerminalStatus;
use lapis_terminal::{TerminalError, TerminalInput};

pub(crate) struct TerminalSessions {
    pub session: TerminalSession,
    active_context: (Option<PathBuf>, ConversationId),
    inactive_sessions: HashMap<(Option<PathBuf>, ConversationId), TerminalSession>,
}

impl TerminalSessions {
    pub fn new(
        session: TerminalSession,
        root: Option<PathBuf>,
        conversation: ConversationId,
    ) -> Self {
        Self {
            session,
            active_context: (root, conversation),
            inactive_sessions: HashMap::new(),
        }
    }

    pub fn switch_context(
        &mut self,
        root: Option<PathBuf>,
        conversation: ConversationId,
        summaries: &[RestoredTerminal],
    ) -> Vec<lapis_terminal::TerminalSnapshot> {
        let context = (root, conversation);
        if self.active_context == context {
            return self.session.terminals().to_vec();
        }
        let next = self.inactive_sessions.remove(&context).unwrap_or_else(|| {
            let mut session = TerminalSession::new(self.session.backend());
            if context.0.is_some() {
                session.restore_summaries(summaries);
            }
            session
        });
        let previous = std::mem::replace(&mut self.session, next);
        let previous_context = std::mem::replace(&mut self.active_context, context);
        self.inactive_sessions.insert(previous_context, previous);
        self.session.terminals().to_vec()
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
    context_unavailable: bool,
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
        if self.context_unavailable || self.poll_pending || self.pending_actions > 0 {
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
    pub(super) discovered: Vec<lapis_terminal::TerminalSnapshot>,
    pub(super) selected: Option<lapis_terminal::TerminalId>,
    selected_by_context: HashMap<(Option<PathBuf>, ConversationId), lapis_terminal::TerminalId>,
    pub(super) focus: gpui::FocusHandle,
    pub(super) marked: String,
    pub(super) visible_output: Option<(lapis_terminal::TerminalId, u64)>,
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
        root: Option<PathBuf>,
        conversation: ConversationId,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        let terminals = session.terminals().to_vec();
        let sessions = TerminalSessions::new(session, root.clone(), conversation.clone());
        let feature = Self {
            worker: crate::client_adapter::ClientWorker::new(sessions),
            terminals,
            discovered: Vec::new(),
            selected: None,
            selected_by_context: HashMap::new(),
            focus: cx.focus_handle(),
            marked: String::new(),
            visible_output: None,
            root,
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
        if !self.requests.context_unavailable
            && self.conversation == conversation
            && self.root == root
        {
            return;
        }
        let next = conversation.clone();
        let next_root = root.clone();
        self.requests.gate.invalidate();
        let token = self.requests.begin_action();
        let admitted = self.worker.submit(
            cx,
            move |sessions| Ok(sessions.switch_context(next_root, next, &summaries)),
            move |feature, result, cx| {
                if !feature.requests.finish_action(token) {
                    return;
                }
                match result {
                    Ok(terminals) => feature.terminals = terminals,
                    Err(error) => {
                        feature.requests.context_unavailable = true;
                        feature.root = None;
                        feature.terminals.clear();
                        feature.message = error;
                        feature.message_from_poll = false;
                    }
                }
                cx.notify();
            },
        );
        if let Err(error) = admitted {
            self.requests.finish_action(token);
            self.requests.context_unavailable = true;
            self.root = None;
            self.terminals.clear();
            self.message = error;
            self.message_from_poll = false;
            cx.notify();
            return;
        }
        self.requests.context_unavailable = false;
        if let Some(selected) = self.active_id() {
            self.selected_by_context
                .insert((self.root.clone(), self.conversation.clone()), selected);
        }
        self.selected = self
            .selected_by_context
            .get(&(root.clone(), conversation.clone()))
            .cloned();
        self.discovered.clear();
        self.marked.clear();
        self.root = root;
        self.conversation = conversation;
        self.terminals.clear();
        cx.notify();
    }
    pub(crate) fn start(&mut self, cx: &mut gpui::Context<Self>) {
        self.start_shell(lapis_terminal::TerminalShell::Default, cx);
    }

    pub(super) fn start_shell(
        &mut self,
        shell: lapis_terminal::TerminalShell,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let token = self.requests.begin_action();
        let admitted = self.worker.submit(
            cx,
            move |sessions| {
                Ok(sessions
                    .action(|session| session.start_with_shell(&root, 120, 30, shell).map(|_| ())))
            },
            move |feature, result, cx| {
                if !feature.requests.finish_action(token) {
                    return;
                }
                match result {
                    Ok((result, terminals)) => {
                        feature.terminals = terminals;
                        feature.selected =
                            feature.terminals.last().map(|terminal| terminal.id.clone());
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
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        self.send_input(TerminalInput::Paste(text), cx);
    }

    pub(super) fn active_id(&self) -> Option<lapis_terminal::TerminalId> {
        self.selected
            .as_ref()
            .filter(|id| self.terminals.iter().any(|terminal| terminal.id == **id))
            .cloned()
            .or_else(|| self.terminals.last().map(|terminal| terminal.id.clone()))
    }

    pub(super) fn active_screen(&self) -> Option<&lapis_terminal::TerminalScreen> {
        let active = self.active_id()?;
        self.terminals
            .iter()
            .find(|terminal| terminal.id == active)
            .map(|terminal| &terminal.screen)
    }

    pub(super) fn discover(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let token = self.requests.begin_action();
        let admitted = self.worker.submit(
            cx,
            move |sessions| {
                sessions
                    .session
                    .discover(&root)
                    .map_err(|error| error.to_string())
            },
            move |feature, result, cx| {
                if feature.requests.finish_action(token) {
                    match result {
                        Ok(terminals) => {
                            feature.discovered = terminals;
                            feature.message.clear();
                        }
                        Err(error) => feature.message = error,
                    }
                    cx.notify();
                }
            },
        );
        if let Err(error) = admitted {
            self.requests.finish_action(token);
            self.message = error;
            cx.notify();
        }
    }

    pub(super) fn attach(&mut self, id: lapis_terminal::TerminalId, cx: &mut gpui::Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let selected = id.clone();
        let token = self.requests.begin_action();
        let admitted = self.worker.submit(
            cx,
            move |sessions| Ok(sessions.action(|session| session.attach(&root, &id))),
            move |feature, result, cx| {
                if feature.requests.finish_action(token) {
                    match result {
                        Ok((outcome, terminals)) => {
                            if outcome.is_ok() {
                                feature.selected = Some(selected);
                            }
                            feature.terminals = terminals;
                            feature.message = outcome.err().unwrap_or_default();
                        }
                        Err(error) => feature.message = error,
                    }
                    cx.notify();
                }
            },
        );
        if let Err(error) = admitted {
            self.requests.finish_action(token);
            self.message = error;
            cx.notify();
        }
    }

    pub(super) fn send_text(&mut self, text: String, cx: &mut gpui::Context<Self>) {
        self.send_input(TerminalInput::Text(text), cx);
    }

    pub(super) fn send_input(&mut self, input: TerminalInput, cx: &mut gpui::Context<Self>) {
        let Some(id) = self.active_id() else { return };
        self.operate(move |session| session.input_semantic(&id, input), cx);
    }

    pub(super) fn resize(&mut self, columns: u16, rows: u16, cx: &mut gpui::Context<Self>) {
        let Some(id) = self.active_id() else { return };
        self.operate(move |session| session.resize(&id, columns, rows), cx);
    }

    pub(super) fn confirm_terminate(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(id) = self.active_id() else { return };
        let message = format!(
            "{} のshellとCLIを終了します。他のClientもこのTerminalを使用できなくなります。",
            id.as_str()
        );
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            "Terminalを終了しますか？",
            Some(&message),
            &[
                gpui::PromptButton::new("終了"),
                gpui::PromptButton::cancel("戻る"),
            ],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if matches!(answer.await, Ok(0)) {
                let _ = this.update(cx, |this, cx| {
                    this.operate(move |session| session.terminate(&id), cx)
                });
            }
        })
        .detach();
    }

    fn operate(
        &mut self,
        operation: impl FnOnce(&mut TerminalSession) -> Result<(), TerminalError> + Send + 'static,
        cx: &mut gpui::Context<Self>,
    ) {
        let token = self.requests.begin_action();
        let admitted = self.worker.submit(
            cx,
            move |sessions| Ok(sessions.action(operation)),
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
    use lapis_terminal::{
        TerminalBackend, TerminalDelta, TerminalEvent, TerminalInput, TerminalRow,
        TerminalRowUpdate, TerminalSize,
    };
    use std::{
        path::Path,
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };

    fn model_row(columns: u16, text: &str) -> TerminalRow {
        let mut row = TerminalRow::empty(columns);
        for (column, character) in text.chars().take(columns as usize).enumerate() {
            row.cells[column].text = character.to_string();
        }
        row
    }

    fn model_delta(revision: u64, size: TerminalSize, text: &str) -> TerminalDelta {
        TerminalDelta {
            revision,
            size,
            changed_rows: vec![TerminalRowUpdate {
                row: 0,
                value: model_row(size.columns, text),
            }],
            cursor: Default::default(),
            modes: Default::default(),
            scrollback: Vec::new(),
        }
    }

    #[test]
    fn failed_context_admission_suppresses_polling_until_retry_is_admitted() {
        let mut requests = TerminalRequests::default();
        let previous_poll = requests.begin_poll().unwrap();
        requests.gate.invalidate();
        let rejected = requests.begin_action();
        requests.finish_action(rejected);
        requests.context_unavailable = true;
        assert!(!requests.finish_poll(previous_poll));
        assert!(requests.begin_poll().is_none());
        requests.gate.invalidate();
        let retry = requests.begin_action();
        requests.context_unavailable = false;
        assert!(requests.begin_poll().is_none());
        assert!(requests.finish_action(retry));
        assert!(requests.begin_poll().is_some());
    }

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
            vec![TerminalEvent::Delta(model_delta(
                1,
                TerminalSize::new(80, 24),
                "background output",
            ))],
        );
        let mut requests = TerminalRequests::default();
        let poll = requests.begin_poll().unwrap();
        assert!(sessions.refresh().unwrap());
        let action = requests.begin_action();
        let (outcome, terminals) = sessions.action(|_| Err(TerminalError::new("input failed")));
        assert!(!requests.finish_poll(poll));
        assert!(requests.finish_action(action));
        assert_eq!(outcome.unwrap_err(), "input failed");
        assert!(
            terminals[0].screen.rows[0]
                .plain_text()
                .starts_with("background output")
        );
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
        inputs: Mutex<Vec<TerminalId>>,
    }

    impl TerminalBackend for Backend {
        fn start(&self, _: &Path, _: u16, _: u16) -> Result<TerminalId, TerminalError> {
            Ok(TerminalId::new(format!(
                "terminal-{}",
                self.next.fetch_add(1, Ordering::Relaxed)
            )))
        }
        fn input(&self, id: &TerminalId, _: &TerminalInput) -> Result<(), TerminalError> {
            self.inputs.lock().unwrap().push(id.clone());
            Ok(())
        }
        fn resize(
            &self,
            _: &TerminalId,
            columns: u16,
            rows: u16,
        ) -> Result<TerminalDelta, TerminalError> {
            Ok(model_delta(1, TerminalSize::new(columns, rows), ""))
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
                Some("workspace".into()),
                ConversationId::new("first"),
            ),
            backend,
        )
    }

    #[test]
    fn workspace_switch_isolates_terminal_operations_without_terminating_previous_context() {
        let (mut sessions, backend) = feature();
        let original = sessions
            .session
            .start(Path::new("workspace"), 80, 24)
            .unwrap();
        let conversation = ConversationId::new("first");
        assert!(
            sessions
                .switch_context(Some("other".into()), conversation.clone(), &[])
                .is_empty()
        );
        backend.events.lock().unwrap().insert(
            original.as_str().to_owned(),
            vec![TerminalEvent::Delta(model_delta(
                1,
                TerminalSize::new(80, 24),
                "inactive output",
            ))],
        );
        assert!(sessions.refresh().unwrap());
        assert!(sessions.session.terminals().is_empty());
        let current = sessions.session.start(Path::new("other"), 80, 24).unwrap();
        let (result, terminals) = sessions.action(|session| {
            session.input(
                &session.terminals().last().unwrap().id,
                "new workspace input",
            )
        });
        result.unwrap();
        assert_eq!(terminals.len(), 1);
        assert_eq!(terminals[0].id, current);
        assert_eq!(terminals[0].cwd, Path::new("other"));
        assert_eq!(*backend.inputs.lock().unwrap(), vec![current.clone()]);
        assert!(
            sessions
                .switch_context(
                    None,
                    conversation.clone(),
                    &[RestoredTerminal {
                        id: None,
                        cwd: "other".into(),
                        status: TerminalStatus::Running,
                        columns: 80,
                        rows: 24,
                    }]
                )
                .is_empty()
        );
        assert!(!sessions.refresh().unwrap());
        let original_snapshot =
            sessions.switch_context(Some("workspace".into()), conversation.clone(), &[]);
        assert_eq!(original_snapshot.len(), 1);
        assert_eq!(original_snapshot[0].id, original);
        assert!(
            original_snapshot[0].screen.rows[0]
                .plain_text()
                .starts_with("inactive output")
        );
        assert_eq!(original_snapshot[0].status, TerminalStatus::Running);
        let current_snapshot = sessions.switch_context(Some("other".into()), conversation, &[]);
        assert_eq!(current_snapshot.len(), 1);
        assert_eq!(current_snapshot[0].id, current);
        assert_eq!(backend.terminated.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn workspace_change_rejects_previous_poll_and_switch_results() {
        let (mut sessions, backend) = feature();
        sessions
            .session
            .start(Path::new("workspace"), 80, 24)
            .unwrap();
        let worker = crate::client_adapter::ClientWorker::new(sessions);
        let mut requests = TerminalRequests::default();
        let poll = requests.begin_poll().unwrap();
        let old_snapshot = worker
            .try_request(|sessions| Ok(sessions.session.terminals().to_vec()))
            .unwrap();
        requests.gate.invalidate();
        let switched = requests.begin_action();
        let next_snapshot =
            worker
                .try_request(|sessions| {
                    Ok(sessions.switch_context(
                        Some("other".into()),
                        ConversationId::new("first"),
                        &[],
                    ))
                })
                .unwrap();
        assert_eq!(
            old_snapshot
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap()
                .unwrap()
                .len(),
            1
        );
        assert!(!requests.finish_poll(poll));
        assert!(requests.begin_poll().is_none());
        assert!(
            next_snapshot
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap()
                .unwrap()
                .is_empty()
        );
        requests.gate.invalidate();
        let closed = requests.begin_action();
        let closed_snapshot = worker
            .try_request(|sessions| {
                Ok(sessions.switch_context(None, ConversationId::new("first"), &[]))
            })
            .unwrap();
        assert!(!requests.finish_action(switched));
        assert!(
            closed_snapshot
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap()
                .unwrap()
                .is_empty()
        );
        assert!(requests.finish_action(closed));
        assert!(requests.begin_poll().is_some());
        assert_eq!(backend.terminated.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn conversation_switch_returns_idle_and_restored_terminals_without_new_events() {
        let (mut sessions, backend) = feature();
        let terminal = sessions
            .session
            .start(Path::new("workspace"), 80, 24)
            .unwrap();
        let summaries = [RestoredTerminal {
            id: None,
            cwd: "saved".into(),
            status: TerminalStatus::Running,
            columns: 120,
            rows: 30,
        }];
        let restored = sessions.switch_context(
            Some("workspace".into()),
            ConversationId::new("second"),
            &summaries,
        );
        assert!(!sessions.refresh().unwrap());
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].cwd, Path::new("saved"));
        assert_eq!(restored[0].status, TerminalStatus::Exited);
        let resumed =
            sessions.switch_context(Some("workspace".into()), ConversationId::new("first"), &[]);
        assert!(!sessions.refresh().unwrap());
        assert_eq!(resumed.len(), 1);
        assert_eq!(resumed[0].id, terminal);
        assert_eq!(resumed[0].status, TerminalStatus::Running);
        assert_eq!(backend.terminated.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn rapid_conversation_switches_only_project_the_latest_snapshot() {
        let (mut sessions, backend) = feature();
        let terminal = sessions
            .session
            .start(Path::new("workspace"), 80, 24)
            .unwrap();
        let worker = crate::client_adapter::ClientWorker::new(sessions);
        let mut requests = TerminalRequests::default();
        requests.gate.invalidate();
        let first = requests.begin_action();
        let first_response = worker
            .try_request(|sessions| {
                Ok(sessions.switch_context(
                    Some("workspace".into()),
                    ConversationId::new("second"),
                    &[],
                ))
            })
            .unwrap();
        requests.gate.invalidate();
        let second = requests.begin_action();
        let second_response = worker
            .try_request(|sessions| {
                Ok(sessions.switch_context(
                    Some("workspace".into()),
                    ConversationId::new("first"),
                    &[],
                ))
            })
            .unwrap();
        assert!(requests.begin_poll().is_none());
        assert!(
            first_response
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap()
                .unwrap()
                .is_empty()
        );
        assert!(!requests.finish_action(first));
        assert!(requests.begin_poll().is_none());
        let terminals = second_response
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap()
            .unwrap();
        assert!(requests.finish_action(second));
        assert_eq!(terminals.len(), 1);
        assert_eq!(terminals[0].id, terminal);
        assert_eq!(terminals[0].status, TerminalStatus::Running);
        assert!(requests.begin_poll().is_some());
        assert_eq!(backend.terminated.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn switching_conversations_retains_live_terminal_identity() {
        let (mut feature, backend) = feature();
        let terminal = feature
            .session
            .start(Path::new("workspace"), 80, 24)
            .unwrap();
        feature.switch_context(Some("workspace".into()), ConversationId::new("second"), &[]);
        assert!(feature.session.terminals().is_empty());
        assert!(feature.has_running_process());
        feature.switch_context(Some("workspace".into()), ConversationId::new("first"), &[]);
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
        feature.switch_context(Some("workspace".into()), ConversationId::new("second"), &[]);
        backend.events.lock().unwrap().insert(
            terminal.as_str().to_owned(),
            vec![
                TerminalEvent::Delta(model_delta(1, TerminalSize::new(80, 24), "background")),
                TerminalEvent::Exited { code: Some(0) },
            ],
        );
        assert!(feature.refresh().unwrap());
        feature.switch_context(Some("workspace".into()), ConversationId::new("first"), &[]);
        assert!(
            feature.session.terminals()[0].screen.rows[0]
                .plain_text()
                .starts_with("background")
        );
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
            id: None,
            cwd: "old".into(),
            status: TerminalStatus::Running,
            columns: 40,
            rows: 10,
        }];
        feature.switch_context(
            Some("workspace".into()),
            ConversationId::new("second"),
            &summaries,
        );
        assert_eq!(
            feature.session.terminals()[0].status,
            TerminalStatus::Exited
        );
        feature.switch_context(
            Some("workspace".into()),
            ConversationId::new("first"),
            &summaries,
        );
        assert_eq!(feature.session.terminals()[0].id, terminal);
        drop(feature);
        assert_eq!(backend.terminated.load(Ordering::Relaxed), 0);
    }
}
