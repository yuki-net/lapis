use lapis_app_services::TerminalSession;
use lapis_terminal::TerminalStatus;

pub(crate) struct TerminalFeature {
    pub session: TerminalSession,
}

impl TerminalFeature {
    pub fn new(session: TerminalSession) -> Self {
        Self { session }
    }

    pub fn has_running_process(&self) -> bool {
        self.session
            .terminals()
            .iter()
            .any(|terminal| terminal.status == TerminalStatus::Running)
    }
}
