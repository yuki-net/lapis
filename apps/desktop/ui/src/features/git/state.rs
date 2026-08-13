use lapis_app_services::GitSession;

pub(crate) struct GitFeature {
    pub session: GitSession,
}

impl GitFeature {
    pub fn new(session: GitSession) -> Self {
        Self { session }
    }
}
