use lapis_app_services::ConversationSession;

pub(crate) struct ConversationFeature {
    pub session: ConversationSession,
}

impl ConversationFeature {
    pub fn new(session: ConversationSession) -> Self {
        Self { session }
    }
}
