//! Desktop の各ウィンドウが所有する拡張宣言の取得状態。
use std::sync::{Arc, mpsc};

use desktop_client::ExtensionContributionSession;
use editor_core::WorkspaceId;

pub(crate) type Loader =
    Arc<dyn Fn(WorkspaceId, u64, ExtensionContributionSession, mpsc::Receiver<()>) + Send + Sync>;

pub(crate) struct ExtensionLoading {
    session: ExtensionContributionSession,
    loader: Loader,
    workspace: Option<WorkspaceId>,
    stop: Option<mpsc::Sender<()>>,
}

impl ExtensionLoading {
    pub(crate) fn new(session: ExtensionContributionSession, loader: Loader) -> Self {
        Self {
            session,
            loader,
            workspace: None,
            stop: None,
        }
    }

    pub(crate) fn set_workspace(&mut self, workspace: Option<WorkspaceId>) {
        if self.workspace == workspace {
            return;
        }
        self.stop.take();
        self.session.disconnect();
        self.workspace = workspace.clone();
        if let Some(workspace) = workspace {
            let generation = self.session.begin_reload();
            let (stop, cancelled) = mpsc::channel();
            self.stop = Some(stop);
            (self.loader)(workspace, generation, self.session.clone(), cancelled);
        }
    }
}

impl Drop for ExtensionLoading {
    fn drop(&mut self) {
        self.stop.take();
        self.session.disconnect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desktop_client::SettingsSession;
    use settings::{SettingsDocument, SettingsError, SettingsRepository, SettingsScope};
    use std::sync::mpsc;

    struct Memory;
    impl SettingsRepository for Memory {
        fn load(&self) -> Result<SettingsDocument, SettingsError> {
            Ok(SettingsDocument::empty(SettingsScope::User))
        }
        fn save(&self, _: &SettingsDocument) -> Result<(), SettingsError> {
            Ok(())
        }
    }

    #[test]
    fn workspace_changes_invalidate_responses_and_drop_clears_contributions() {
        let settings = SettingsSession::load(Arc::new(Memory)).unwrap();
        let session = settings.extension_contributions();
        let (tx, rx) = mpsc::channel();
        let loader = Arc::new(move |workspace, generation, session, cancelled| {
            tx.send((workspace, generation, session, cancelled))
                .unwrap();
        });
        let mut loading = ExtensionLoading::new(session.clone(), loader);
        loading.set_workspace(Some(WorkspaceId::new("first")));
        let (_, old_generation, old_session, old_cancelled): (
            _,
            _,
            ExtensionContributionSession,
            mpsc::Receiver<()>,
        ) = rx.recv().unwrap();
        loading.set_workspace(Some(WorkspaceId::new("second")));
        assert!(matches!(
            old_cancelled.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
        let (_, generation, current, cancelled): (
            _,
            _,
            ExtensionContributionSession,
            mpsc::Receiver<()>,
        ) = rx.recv().unwrap();
        let empty = || serde_json::from_str(r#"{"extensions":[],"contributions":[]}"#).unwrap();
        assert!(!old_session.apply(old_generation, empty()).unwrap());
        assert!(current.apply(generation, empty()).unwrap());
        assert!(session.snapshot().is_some());
        loading.set_workspace(Some(WorkspaceId::new("second")));
        assert!(rx.try_recv().is_err());
        drop(loading);
        assert!(matches!(
            cancelled.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
        assert!(session.snapshot().is_none());
        assert!(!current.apply(generation, empty()).unwrap());
    }
}
