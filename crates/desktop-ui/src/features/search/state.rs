use std::ops::Range;

use lapis_app_services::WorkspaceSearchSession;

pub(crate) struct SearchFeature {
    pub workspace: WorkspaceSearchSession,
    pub query: String,
    pub matches: Vec<Range<usize>>,
    pub current_match: usize,
}

impl SearchFeature {
    pub fn new(workspace: WorkspaceSearchSession) -> Self {
        Self {
            workspace,
            query: String::new(),
            matches: Vec::new(),
            current_match: 0,
        }
    }
}
