//! アプリ側の更新状態の共有と、更新操作の入口。
//!
//! 更新の検出・download・検証・staging は Desktop の Composition Root が所有する。
//! UI はここで公開する状態と操作だけを使い、通信や file 操作を持たない
//! （[配布と更新の内部契約](../../../docs/specs/release.md)）。

use std::sync::{Arc, Mutex, mpsc};

/// 更新の進行状態。失敗してもアプリの利用は妨げない。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum UpdateStatus {
    /// 確認していない。
    #[default]
    Idle,
    /// manifest を確認中。
    Checking,
    /// 新しい version がある（通知のみのモード、または同意待ちでない軸A）。
    Available { version: String, notes_url: String },
    /// 軸B。data 世代が上がるため、適用に同意が必要。何も変更していない。
    Breaking {
        version: String,
        notes_url: String,
        /// 現在使っている data 世代。この世代の data が新世代へ引き継がれない。
        current_generation: u32,
    },
    /// 署名・SHA-256 を検証して staging に置いた。次回起動で適用される。
    Staged {
        version: String,
        /// 同意済みの軸B かどうか。
        breaking: bool,
    },
    /// 確認・download・検証のいずれかに失敗した。現在の版で使い続けられる。
    Failed { message: String },
}

impl UpdateStatus {
    /// 軸B の同意待ちかどうか。同意画面はこの状態だけで出す。
    pub fn awaits_consent(&self) -> bool {
        matches!(self, Self::Breaking { .. })
    }
}

/// 更新状態の購読。購読時点の最新値を最初に 1 件返す。
#[derive(Clone, Default)]
pub struct UpdateWatch {
    status: Arc<Mutex<UpdateStatus>>,
    listeners: Arc<Mutex<Vec<mpsc::Sender<UpdateStatus>>>>,
}

impl UpdateWatch {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn status(&self) -> UpdateStatus {
        self.status
            .lock()
            .map(|status| status.clone())
            .unwrap_or_default()
    }

    pub fn publish(&self, status: UpdateStatus) {
        if let Ok(mut current) = self.status.lock() {
            *current = status.clone();
            if let Ok(mut listeners) = self.listeners.lock() {
                listeners.retain(|listener| listener.send(status.clone()).is_ok());
            }
        }
    }

    /// 次の状態を受け取る購読。購読時点の最新値を最初に 1 件受け取る。
    pub fn subscribe(&self) -> mpsc::Receiver<UpdateStatus> {
        let (sender, receiver) = mpsc::channel();
        let _ = sender.send(self.status());
        if let Ok(mut listeners) = self.listeners.lock() {
            listeners.push(sender);
        }
        receiver
    }
}

/// UI から更新処理へ渡す操作。実装は Desktop の Composition Root が持つ。
pub trait UpdateController: Send + Sync {
    /// manifest を今すぐ確認する。結果は [`UpdateWatch`] で通知される。
    fn check(&self);
    /// 軸B の適用に同意する。新しい data 世代を並置し、再起動で切り替える。
    fn consent_breaking(&self);
    /// 軸B の同意を見送る。現在の版と data 世代を使い続ける。
    fn dismiss_breaking(&self);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn subscribers_receive_published_statuses() {
        let watch = UpdateWatch::new();
        let receiver = watch.subscribe();
        // 購読時点の最新値（未確認）を先に受け取る。
        assert_eq!(
            receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("current status"),
            UpdateStatus::Idle
        );
        watch.publish(UpdateStatus::Checking);
        assert_eq!(
            receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("status"),
            UpdateStatus::Checking
        );
        assert_eq!(watch.status(), UpdateStatus::Checking);
    }

    #[test]
    fn only_a_breaking_update_awaits_consent() {
        assert!(
            UpdateStatus::Breaking {
                version: "1.0.0".into(),
                notes_url: "https://example.invalid".into(),
                current_generation: 1,
            }
            .awaits_consent()
        );
        for status in [
            UpdateStatus::Idle,
            UpdateStatus::Checking,
            UpdateStatus::Available {
                version: "1.0.0".into(),
                notes_url: "https://example.invalid".into(),
            },
            UpdateStatus::Staged {
                version: "1.0.0".into(),
                breaking: true,
            },
            UpdateStatus::Failed {
                message: "offline".into(),
            },
        ] {
            assert!(!status.awaits_consent(), "{status:?}");
        }
    }

    #[test]
    fn the_latest_status_is_returned_to_a_late_subscriber() {
        let watch = UpdateWatch::new();
        watch.publish(UpdateStatus::Staged {
            version: "0.2.0".into(),
            breaking: false,
        });
        let receiver = watch.subscribe();
        assert_eq!(
            receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("current status"),
            UpdateStatus::Staged {
                version: "0.2.0".into(),
                breaking: false
            }
        );
        assert!(receiver.try_recv().is_err());
    }
}
