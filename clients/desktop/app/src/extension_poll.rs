//! 接続中に拡張 Host の状態変化を取り込む。購読 API ができるまでの取得経路。
use std::{sync::mpsc, time::Duration};

use client_api::HostExtensionsListResponse;
use desktop_client::ExtensionContributionSession;

pub(crate) fn run(
    session: ExtensionContributionSession,
    generation: u64,
    cancelled: mpsc::Receiver<()>,
    interval: Duration,
    mut fetch: impl FnMut() -> Result<HostExtensionsListResponse, String>,
) {
    let mut previous = None;
    let mut failed = false;
    loop {
        match cancelled.try_recv() {
            Err(mpsc::TryRecvError::Empty) => {}
            _ => break,
        }
        match fetch() {
            Ok(response) => {
                failed = false;
                if previous.as_ref() != Some(&response) {
                    match session.apply(generation, response.clone()) {
                        Ok(true) => previous = Some(response),
                        Ok(false) => break,
                        Err(error) => eprintln!("拡張宣言の適用に失敗しました: {error}"),
                    }
                }
            }
            Err(error) => {
                if !failed {
                    eprintln!("拡張宣言の取得に失敗しました: {error}");
                }
                failed = true;
                previous = None;
                if !session.clear_if_current(generation) {
                    break;
                }
            }
        }
        if !matches!(
            cancelled.recv_timeout(interval),
            Err(mpsc::RecvTimeoutError::Timeout)
        ) {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desktop_client::SettingsSession;
    use settings::{SettingsDocument, SettingsError, SettingsRepository, SettingsScope};
    use std::{sync::Arc, thread};

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
    fn connection_loss_clears_snapshot_and_recovery_restores_it() {
        let settings = SettingsSession::load(Arc::new(Memory)).unwrap();
        let session = settings.extension_contributions();
        let generation = session.begin_reload();
        let events = session.subscribe();
        let (source, responses) = mpsc::channel::<Result<HostExtensionsListResponse, String>>();
        let (stop, cancelled) = mpsc::channel();
        let worker_session = session.clone();
        let worker = thread::spawn(move || {
            run(
                worker_session,
                generation,
                cancelled,
                Duration::from_millis(1),
                || responses.recv().unwrap(),
            )
        });
        let timeout = Duration::from_secs(5);
        source
            .send(Ok(HostExtensionsListResponse::default()))
            .unwrap();
        assert!(events.recv_timeout(timeout).unwrap().is_some());
        source
            .send(Ok(HostExtensionsListResponse::default()))
            .unwrap();
        source.send(Err("offline".into())).unwrap();
        assert!(events.recv_timeout(timeout).unwrap().is_none());
        source
            .send(Ok(HostExtensionsListResponse::default()))
            .unwrap();
        assert!(events.recv_timeout(timeout).unwrap().is_some());
        drop(stop);
        let _ = source.send(Ok(HostExtensionsListResponse::default()));
        worker.join().unwrap();
    }
}
