use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use desktop_client::{RestoredTerminal, TerminalSession};
use editor_core::ConversationId;
use terminal::TerminalError;
#[cfg(test)]
use terminal::TerminalStatus;

pub(super) struct TerminalSessions {
    pub session: TerminalSession,
    active_context: (Option<PathBuf>, ConversationId),
    inactive_sessions: HashMap<(Option<PathBuf>, ConversationId), TerminalSession>,
    /// 画面の投影順を決める単調増加の版。worker 上で観測した順序を表す。
    version: u64,
    pub(super) live_sessions: Arc<AtomicBool>,
}

impl TerminalSessions {
    pub fn new(
        session: TerminalSession,
        root: Option<PathBuf>,
        conversation: ConversationId,
    ) -> Self {
        let live_sessions = Arc::new(AtomicBool::new(session.terminals().iter().any(
            |terminal| {
                matches!(
                    terminal.status,
                    terminal::TerminalStatus::Running | terminal::TerminalStatus::Disconnected
                )
            },
        )));
        Self {
            session,
            active_context: (root, conversation),
            inactive_sessions: HashMap::new(),
            version: 0,
            live_sessions,
        }
    }

    /// 最後に観測した画面の版。UI はより新しい版だけを適用する。
    pub fn version(&self) -> u64 {
        self.version
    }

    /// 各 Terminal の操作権。owner だけが入力できる。
    pub fn owners(&self) -> &std::collections::HashMap<String, Option<String>> {
        self.session.owners()
    }

    fn bump(&mut self) {
        let live = std::iter::once(&self.session)
            .chain(self.inactive_sessions.values())
            .flat_map(TerminalSession::terminals)
            .any(|terminal| {
                matches!(
                    terminal.status,
                    terminal::TerminalStatus::Running | terminal::TerminalStatus::Disconnected
                )
            });
        self.live_sessions.store(live, Ordering::Release);
        self.version = self.version.wrapping_add(1);
    }

    pub fn switch_context(
        &mut self,
        root: Option<PathBuf>,
        conversation: ConversationId,
        summaries: &[RestoredTerminal],
    ) -> Vec<terminal::TerminalSnapshot> {
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
        self.bump();
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
        if changed || first_error.is_some() {
            self.bump();
        }
        first_error.map_or(Ok(changed), Err)
    }

    /// この実体が所有する全 Session を終了する。
    ///
    /// タブごとに独立した TerminalFeature を持つため、ここで止まるのは
    /// このタブのセッションだけ。他のタブの PTY には触れない。
    /// 退避中のコンテキストも同じタブの所有物なので併せて止める。
    pub fn close_all(&mut self) -> (Result<(), String>, Vec<terminal::TerminalSnapshot>) {
        let mut failure = None;
        for session in std::iter::once(&mut self.session).chain(self.inactive_sessions.values_mut())
        {
            let ids = session
                .terminals()
                .iter()
                .map(|terminal| terminal.id.clone())
                .collect::<Vec<_>>();
            for id in ids {
                if let Err(error) = session.terminate_and_forget(&id)
                    && failure.is_none()
                {
                    failure = Some(error.to_string());
                }
            }
        }
        self.bump();
        (
            failure.map_or(Ok(()), Err),
            self.session.terminals().to_vec(),
        )
    }

    pub(super) fn action(
        &mut self,
        operation: impl FnOnce(&mut TerminalSession) -> Result<(), TerminalError>,
    ) -> (Result<(), String>, Vec<terminal::TerminalSnapshot>) {
        let result = operation(&mut self.session).map_err(|error| error.to_string());
        self.bump();
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
