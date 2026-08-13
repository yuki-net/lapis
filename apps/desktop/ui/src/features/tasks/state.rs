use lapis_app_services::TaskSession;
use lapis_editor_core::ExecutionId;

pub(crate) struct TasksFeature {
    pub session: TaskSession,
    pub selected_execution: Option<ExecutionId>,
}

impl TasksFeature {
    pub fn new(session: TaskSession, selected_execution: Option<ExecutionId>) -> Self {
        Self {
            session,
            selected_execution,
        }
    }

    pub fn has_active_execution(&self) -> bool {
        self.session
            .records()
            .iter()
            .any(|record| !record.execution.status.is_terminal())
    }
}
