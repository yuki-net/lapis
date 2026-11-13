//! アプリ側の更新処理。
//!
//! `docs/specs/release.md` の経路をアプリから回す。役割は次のとおり。
//!
//! - セッション中にバックグラウンドで manifest を確認し、新しければ download・検証して
//!   `staging/<version>/` へ置く（軸A）。適用は次回起動時の launcher が行う。
//! - `breaking: true`（軸B）は同意を得てから staging へ置き、新しい data 世代を並置する。
//!   同意しなければ何も変えず、現在の版と世代を使い続ける。
//! - 失敗しても起動・利用を妨げない。状態だけを [`UpdateWatch`] へ流す。
//!
//! UI は [`UpdateWatch`] を購読して [`UpdateStatus`] を表示し、
//! [`UpdateController`] の 3 操作だけを呼ぶ。通信と file 操作はここに閉じる。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use desktop_client::{SettingsSession, UpdateController, UpdateStatus, UpdateWatch};
use lapis_launcher::{StageOptions, Verifier};
use settings::{UpdateMode, UpdateSource};

use crate::update_fetch::{ManifestSource, RemoteManifest, fetch_and_stage, fetch_manifest};

/// 更新の確認間隔。起動時と、その後のセッション中に同じ間隔で確認する。
pub(crate) const CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(6 * 60 * 60);

/// 更新処理が使う設定の写し。確認のたびに作り直す。
#[derive(Clone, Debug, PartialEq, Eq)]
struct UpdatePolicy {
    mode: UpdateMode,
    source: UpdateSource,
    location: Option<String>,
}

impl UpdatePolicy {
    fn from_settings(settings: &SettingsSession) -> Self {
        let values = settings.settings();
        Self {
            mode: values.update_mode(),
            source: values.update_source(),
            location: values.update_source_location(),
        }
    }

    fn source(&self) -> Result<ManifestSource, lapis_launcher::Error> {
        ManifestSource::from_setting(self.source.as_id(), self.location.as_deref())
    }
}

/// 更新処理の本体。Composition Root が 1 つだけ作る。
pub struct AppUpdater {
    watch: UpdateWatch,
    install_root: PathBuf,
    /// 設定の保存先。確認のたびに最新の更新方針を読む。
    settings: Mutex<SettingsSession>,
    /// 検証器。信頼の起点は repository の registry だけ。
    verifier: Verifier,
    /// 確認中に別の要求が来ても二重に走らせない。worker からも解除する。
    running: Arc<AtomicBool>,
    /// 実行中に来た再確認要求。完了後にもう一度走らせる。
    pending: Arc<AtomicBool>,
    /// 同意待ちの軸B。同意画面から参照する。
    consent: Arc<Mutex<Option<UpdateStatus>>>,
    /// 直近に確認した方針。設定変更で方針が変わったときだけ再確認する。
    last_policy: Arc<Mutex<Option<UpdatePolicy>>>,
    /// worker が毎回参照する現在の方針。設定変更で更新する。
    current_policy: Arc<Mutex<UpdatePolicy>>,
}

impl AppUpdater {
    pub fn new(install_root: PathBuf, settings: SettingsSession) -> Self {
        Self {
            watch: UpdateWatch::new(),
            install_root,
            settings: Mutex::new(settings),
            verifier: Verifier::repository().expect("trusted release keys must load"),
            running: Arc::new(AtomicBool::new(false)),
            pending: Arc::new(AtomicBool::new(false)),
            consent: Arc::new(Mutex::new(None)),
            last_policy: Arc::new(Mutex::new(None)),
            current_policy: Arc::new(Mutex::new(UpdatePolicy {
                mode: UpdateMode::Auto,
                source: UpdateSource::Github,
                location: None,
            })),
        }
    }

    pub fn watch(&self) -> UpdateWatch {
        self.watch.clone()
    }

    /// 設定変更後に更新方針が変わったかを確認し、変わっていれば次の確認を予約する。
    ///
    /// 実行中の確認があっても要求を捨てず、完了後にもう一度評価させる。
    pub fn settings_changed(&self) {
        // 保存済みの設定から方針を読み直してから共有 slot を更新する。
        let policy = self.refresh_policy();
        if let Ok(last) = self.last_policy.lock()
            && last.as_ref() == Some(&policy)
        {
            return;
        }
        self.spawn_check();
    }

    fn policy(&self) -> UpdatePolicy {
        self.current_policy
            .lock()
            .map(|policy| policy.clone())
            .unwrap_or(UpdatePolicy {
                mode: UpdateMode::Auto,
                source: UpdateSource::Github,
                location: None,
            })
    }

    /// 保存済みの設定から方針を読み直し、worker が参照する共有 slot を更新する。
    fn refresh_policy(&self) -> UpdatePolicy {
        let policy = self
            .settings
            .lock()
            .map(|settings| UpdatePolicy::from_settings(&settings))
            .unwrap_or(UpdatePolicy {
                mode: UpdateMode::Auto,
                source: UpdateSource::Github,
                location: None,
            });
        if let Ok(mut current) = self.current_policy.lock() {
            *current = policy.clone();
        }
        policy
    }

    /// manifest を確認する。呼び出し側の thread を止めないよう、別 thread で走らせる。
    ///
    /// 既に確認中なら、その回の完了後にもう一度確認するよう要求を残す。設定変更で
    /// 方針が変わった場合に、古い方針の結果だけを確定しない。
    pub fn spawn_check(&self) {
        if self
            .running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            // 実行中の確認がある。完了後にもう一度走らせる要求だけ残す。
            self.pending.store(true, Ordering::Release);
            return;
        }
        let watch = self.watch.clone();
        let root = self.install_root.clone();
        let running = self.running.clone();
        let pending = self.pending.clone();
        let consent = self.consent.clone();
        let verifier = self.verifier.clone_verifier();
        let settings = self.settings.lock().map(|settings| settings.clone()).ok();
        let current_policy = self.current_policy.clone();
        let last_policy = self.last_policy.clone();
        let spawned = std::thread::Builder::new()
            .name("lapis-update-check".to_owned())
            .spawn(move || {
                loop {
                    // 方針は毎回読み直す。SettingsSession は共有モデルの handle なので、
                    // clone しても後から保存された値を読める。実行中の設定変更を古い
                    // 方針のまま確定しない。
                    let policy = settings
                        .as_ref()
                        .map(UpdatePolicy::from_settings)
                        .unwrap_or(UpdatePolicy {
                            mode: UpdateMode::Auto,
                            source: UpdateSource::Github,
                            location: None,
                        });
                    if let Ok(mut current) = current_policy.lock() {
                        *current = policy.clone();
                    }
                    if let Ok(mut last) = last_policy.lock() {
                        *last = Some(policy.clone());
                    }
                    let status = check_once(&root, &policy, &verifier);
                    if let Ok(mut pending_consent) = consent.lock() {
                        *pending_consent = status.awaits_consent().then(|| status.clone());
                    }
                    // 同意待ちも UI に知らせる。同意画面は Breaking のときだけ出す。
                    watch.publish(status);
                    if !pending.swap(false, Ordering::AcqRel) {
                        break;
                    }
                }
                running.store(false, Ordering::Release);
            });
        if spawned.is_err() {
            self.running.store(false, Ordering::Release);
            self.watch.publish(UpdateStatus::Failed {
                message: "更新の確認を開始できません".to_owned(),
            });
        }
    }

    /// 破壊的更新に同意する。新しい data 世代を並置してから staging へ置く。
    pub fn spawn_consent(&self) {
        let Some(UpdateStatus::Breaking { version, .. }) = self.consent() else {
            return;
        };
        self.set_consent(None);
        let policy = self.policy();
        let watch = self.watch.clone();
        let root = self.install_root.clone();
        let verifier = self.verifier.clone_verifier();
        let spawned = std::thread::Builder::new()
            .name("lapis-update-consent".to_owned())
            .spawn(move || {
                let status = consent_once(&root, &policy, &verifier, &version);
                watch.publish(status);
            });
        if spawned.is_err() {
            self.watch.publish(UpdateStatus::Failed {
                message: "更新の適用を開始できません".to_owned(),
            });
        }
    }

    /// 破壊的更新を見送る。現在の版と data 世代を使い続ける。
    pub fn dismiss_consent(&self) {
        let version = match self.consent() {
            Some(UpdateStatus::Breaking { version, .. }) => Some(version),
            _ => None,
        };
        self.set_consent(None);
        self.watch.publish(UpdateStatus::Idle);
        // 同意しなかった候補は staging に残さない。残すと launcher が適用を試み続ける。
        if let Some(version) = version {
            let staging = self.install_root.join("staging").join(version);
            let _ = std::fs::remove_dir_all(staging);
        }
    }

    fn consent(&self) -> Option<UpdateStatus> {
        self.consent.lock().ok().and_then(|consent| consent.clone())
    }

    fn set_consent(&self, status: Option<UpdateStatus>) {
        if let Ok(mut consent) = self.consent.lock() {
            *consent = status;
        }
    }
}

impl UpdateController for AppUpdater {
    fn check(&self) {
        self.spawn_check();
    }

    fn consent_breaking(&self) {
        self.spawn_consent();
    }

    fn dismiss_breaking(&self) {
        self.dismiss_consent();
    }
}

/// 1 回の確認。軸A は staging まで、軸B は同意待ちで止める。
fn check_once(root: &Path, policy: &UpdatePolicy, verifier: &Verifier) -> UpdateStatus {
    if policy.mode == UpdateMode::Off {
        return UpdateStatus::Idle;
    }
    if policy.source.uses_location() && policy.location.is_none() {
        return UpdateStatus::Failed {
            message: "配布元の場所が未設定です".to_owned(),
        };
    }
    let source = match policy.source() {
        Ok(source) => source,
        Err(error) => {
            return UpdateStatus::Failed {
                message: error.to_string(),
            };
        }
    };
    let current = verifier
        .current_version(root)
        .ok()
        .flatten()
        .unwrap_or_default();
    let remote = match fetch_manifest(&source, verifier) {
        Ok(remote) => remote,
        Err(error) => {
            return UpdateStatus::Failed {
                message: error.to_string(),
            };
        }
    };
    if !current.is_empty() && !lapis_launcher::is_newer(&remote.version, &current) {
        return UpdateStatus::Idle;
    }
    // 既に検証済みで staging にある同じ版は、download も通知もやり直さない。
    // pointer は次の launcher 起動まで旧版のままなので、毎回「新着」と誤認しやすい。
    if verifier
        .staged(root, &remote.version)
        .ok()
        .flatten()
        .is_some()
    {
        return match lapis_launcher::consented_generation(root, &remote.version) {
            Ok(Some(_)) => UpdateStatus::Staged {
                version: remote.version,
                breaking: true,
            },
            _ => UpdateStatus::Staged {
                version: remote.version,
                breaking: false,
            },
        };
    }
    // 通知のみのモードでは download しない。適用は利用者の操作から行う。
    if policy.mode == UpdateMode::Notify {
        return UpdateStatus::Available {
            version: remote.version,
            notes_url: remote.notes_url,
        };
    }
    // 軸B は同意なしに download・staging しない。何が reset されるかを示して同意を取る。
    if remote.breaking {
        let current_generation = lapis_launcher::read_generation(root).unwrap_or(1);
        return UpdateStatus::Breaking {
            version: remote.version,
            notes_url: remote.notes_url,
            current_generation,
        };
    }
    // 軸A は署名と SHA-256 を検証して staging まで進める。breaking は受け付けない。
    match fetch_and_stage(
        root,
        &source,
        verifier,
        StageOptions {
            allow_breaking: false,
        },
        |manifest| !manifest.breaking,
    ) {
        Ok(Some(staged)) => UpdateStatus::Staged {
            version: staged.version,
            breaking: false,
        },
        Ok(None) => UpdateStatus::Idle,
        Err(error) => UpdateStatus::Failed {
            message: error.to_string(),
        },
    }
}

/// 同意済みの軸B。新しい data 世代を並置し、staging へ置く。
fn consent_once(
    root: &Path,
    policy: &UpdatePolicy,
    verifier: &Verifier,
    version: &str,
) -> UpdateStatus {
    let source = match policy.source() {
        Ok(source) => source,
        Err(error) => {
            return UpdateStatus::Failed {
                message: error.to_string(),
            };
        }
    };
    let staged = fetch_and_stage(
        root,
        &source,
        verifier,
        StageOptions {
            allow_breaking: true,
        },
        |manifest: &RemoteManifest| manifest.version == version && manifest.breaking,
    );
    let staged = match staged {
        Ok(Some(staged)) => staged,
        Ok(None) => {
            return UpdateStatus::Failed {
                message: "同意した version の配布物が見つかりません".to_owned(),
            };
        }
        Err(error) => {
            return UpdateStatus::Failed {
                message: error.to_string(),
            };
        }
    };
    // 新しい世代を用意し、同意済みとして記録する。現在世代の pointer はまだ進めない。
    // 適用は次の launcher が行い、実際に起動したときだけ世代を切り替える。
    let generation = match lapis_launcher::prepare_generation(root) {
        Ok(generation) => generation,
        Err(error) => {
            return UpdateStatus::Failed {
                message: error.to_string(),
            };
        }
    };
    if let Err(error) = lapis_launcher::record_consent(root, &staged.version, generation) {
        return UpdateStatus::Failed {
            message: error.to_string(),
        };
    }
    UpdateStatus::Staged {
        version: staged.version,
        breaking: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(mode: UpdateMode, source: UpdateSource, location: Option<&str>) -> UpdatePolicy {
        UpdatePolicy {
            mode,
            source,
            location: location.map(ToOwned::to_owned),
        }
    }

    fn verifier() -> Verifier {
        let registry = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../launcher/tests/fixtures/keys.json");
        let raw = std::fs::read(registry).expect("fixture registry");
        Verifier::from_registry(&raw).expect("registry")
    }

    fn distribution(name: &str) -> PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../launcher/tests/fixtures")
            .join(name)
    }

    fn installed(root: &Path, version: &str) {
        let directory = root.join("versions").join(version);
        std::fs::create_dir_all(&directory).expect("versions");
        std::fs::write(
            directory.join("manifest.json"),
            format!(
                r#"{{
  "assets": {{}},
  "breaking": false,
  "generated_at": "2026-11-12T00:00:00Z",
  "key_id": "launcher-fixture",
  "notes_url": "https://example.invalid/releases/v{version}",
  "schema": 1,
  "signature": "AA==",
  "version": "{version}"
}}"#
            ),
        )
        .expect("manifest");
        std::fs::write(
            root.join("current.json"),
            format!("{{\"schema\":1,\"version\":\"{version}\"}}\n"),
        )
        .expect("pointer");
    }

    #[test]
    fn the_policy_keeps_the_source_location_for_folder_and_server() {
        let server = policy(
            UpdateMode::Notify,
            UpdateSource::Server,
            Some("https://example.invalid/lapis"),
        );
        assert_eq!(server.mode, UpdateMode::Notify);
        assert!(server.source.uses_location());
        let folder = UpdatePolicy {
            source: UpdateSource::Local,
            ..server
        };
        assert!(folder.source.uses_location());
        let github = policy(UpdateMode::Auto, UpdateSource::Github, None);
        assert!(!github.source.uses_location());
    }

    #[test]
    fn off_checks_nothing() {
        let root = tempfile::tempdir().expect("temp");
        let status = check_once(
            root.path(),
            &policy(UpdateMode::Off, UpdateSource::Local, None),
            &verifier(),
        );
        assert_eq!(status, UpdateStatus::Idle);
        assert!(!root.path().join("staging").exists());
    }

    #[test]
    fn a_missing_source_location_fails_without_blocking() {
        let root = tempfile::tempdir().expect("temp");
        let status = check_once(
            root.path(),
            &policy(UpdateMode::Auto, UpdateSource::Local, None),
            &verifier(),
        );
        assert!(matches!(status, UpdateStatus::Failed { .. }), "{status:?}");
    }

    #[test]
    fn a_new_axis_a_version_is_staged_without_moving_the_pointer() {
        let root = tempfile::tempdir().expect("temp");
        installed(root.path(), "0.1.0-alpha.1");
        let status = check_once(
            root.path(),
            &policy(
                UpdateMode::Auto,
                UpdateSource::Local,
                Some(&distribution("distribution2").to_string_lossy()),
            ),
            &verifier(),
        );
        assert_eq!(
            status,
            UpdateStatus::Staged {
                version: "0.1.0-alpha.2".into(),
                breaking: false
            }
        );
        assert!(
            root.path()
                .join("staging/0.1.0-alpha.2/manifest.json")
                .is_file(),
            "staging に置く"
        );
        // 適用は次の launcher 起動。pointer は動かさない。
        let pointer = std::fs::read_to_string(root.path().join("current.json")).expect("pointer");
        assert!(pointer.contains("0.1.0-alpha.1"), "{pointer}");
    }

    #[test]
    fn the_same_or_older_version_is_not_staged() {
        let root = tempfile::tempdir().expect("temp");
        installed(root.path(), "0.1.0-alpha.2");
        let status = check_once(
            root.path(),
            &policy(
                UpdateMode::Auto,
                UpdateSource::Local,
                Some(&distribution("distribution").to_string_lossy()),
            ),
            &verifier(),
        );
        assert_eq!(status, UpdateStatus::Idle);
        assert!(!root.path().join("staging/0.1.0-alpha.1").exists());
    }

    #[test]
    fn notify_mode_reports_the_version_without_downloading() {
        let root = tempfile::tempdir().expect("temp");
        installed(root.path(), "0.1.0-alpha.1");
        let status = check_once(
            root.path(),
            &policy(
                UpdateMode::Notify,
                UpdateSource::Local,
                Some(&distribution("distribution2").to_string_lossy()),
            ),
            &verifier(),
        );
        assert_eq!(
            status,
            UpdateStatus::Available {
                version: "0.1.0-alpha.2".into(),
                notes_url: "https://example.invalid/releases/v0.1.0-alpha.2".into()
            }
        );
        assert!(!root.path().join("staging").exists());
    }

    #[test]
    fn a_breaking_version_waits_for_consent_and_changes_nothing() {
        let root = tempfile::tempdir().expect("temp");
        installed(root.path(), "0.1.0-alpha.1");
        let status = check_once(
            root.path(),
            &policy(
                UpdateMode::Auto,
                UpdateSource::Local,
                Some(&distribution("distribution-breaking").to_string_lossy()),
            ),
            &verifier(),
        );
        assert!(
            matches!(
                status,
                UpdateStatus::Breaking {
                    ref version,
                    current_generation: 1,
                    ..
                } if version == "0.3.0"
            ),
            "{status:?}"
        );
        assert!(status.awaits_consent());
        // 同意するまで何も download・作成しない。
        assert!(!root.path().join("staging").exists());
        assert!(!root.path().join("data/2").exists());
    }

    #[test]
    fn consenting_stages_the_breaking_version_and_prepares_a_generation() {
        let root = tempfile::tempdir().expect("temp");
        installed(root.path(), "0.1.0-alpha.1");
        lapis_launcher::record_generation(root.path(), "0.1.0-alpha.1", 1).expect("record");
        std::fs::create_dir_all(root.path().join("data/1")).expect("data");
        let policy = policy(
            UpdateMode::Auto,
            UpdateSource::Local,
            Some(&distribution("distribution-breaking").to_string_lossy()),
        );
        let status = consent_once(root.path(), &policy, &verifier(), "0.3.0");
        assert_eq!(
            status,
            UpdateStatus::Staged {
                version: "0.3.0".into(),
                breaking: true
            }
        );
        assert!(root.path().join("staging/0.3.0/manifest.json").is_file());
        // 同意しただけでは現在世代を進めない。実際に起動したときだけ切り替える。
        assert_eq!(
            lapis_launcher::read_generation(root.path()).expect("generation"),
            1
        );
        assert_eq!(
            lapis_launcher::consented_generation(root.path(), "0.3.0").expect("consent"),
            Some(2)
        );
        // 旧世代は残り、旧 version は旧世代で再開できる。
        assert!(root.path().join("data/1").is_dir());
        assert_eq!(
            lapis_launcher::generation_for(root.path(), "0.1.0-alpha.1").expect("read"),
            Some(1)
        );
        assert_eq!(
            lapis_launcher::generation_for(root.path(), "0.3.0").expect("read"),
            Some(2)
        );
        // launcher が起動したときに初めて現在世代を進める。
        assert_eq!(
            lapis_launcher::activate_generation(root.path(), "0.3.0").expect("activate"),
            Some(2)
        );
        assert_eq!(
            lapis_launcher::read_generation(root.path()).expect("generation"),
            2
        );
    }

    #[test]
    fn a_tampered_distribution_is_never_staged() {
        let root = tempfile::tempdir().expect("temp");
        installed(root.path(), "0.1.0-alpha.1");
        let source = tempfile::tempdir().expect("source");
        for entry in std::fs::read_dir(distribution("distribution2")).expect("fixture") {
            let entry = entry.expect("entry");
            std::fs::copy(entry.path(), source.path().join(entry.file_name())).expect("copy");
        }
        let package = source.path().join("lapis-0.1.0-alpha.2-windows-x86_64.zip");
        let mut bytes = std::fs::read(&package).expect("package");
        bytes[0] ^= 0xff;
        std::fs::write(&package, bytes).expect("tamper");
        let status = check_once(
            root.path(),
            &policy(
                UpdateMode::Auto,
                UpdateSource::Local,
                Some(&source.path().to_string_lossy()),
            ),
            &verifier(),
        );
        assert!(matches!(status, UpdateStatus::Failed { .. }), "{status:?}");
        assert!(!root.path().join("staging").exists());
    }

    #[test]
    fn dismissing_consent_removes_the_unwanted_candidate() {
        let root = tempfile::tempdir().expect("temp");
        let staging = root.path().join("staging/0.3.0");
        std::fs::create_dir_all(&staging).expect("staging");
        let updater = AppUpdater {
            watch: UpdateWatch::new(),
            install_root: root.path().to_owned(),
            settings: Mutex::new(
                SettingsSession::load(std::sync::Arc::new(MemorySettings)).expect("settings"),
            ),
            verifier: verifier(),
            running: Arc::new(AtomicBool::new(false)),
            pending: Arc::new(AtomicBool::new(false)),
            consent: Arc::new(Mutex::new(Some(UpdateStatus::Breaking {
                version: "0.3.0".into(),
                notes_url: "https://example.invalid".into(),
                current_generation: 1,
            }))),
            last_policy: Arc::new(Mutex::new(None)),
            current_policy: Arc::new(Mutex::new(UpdatePolicy {
                mode: UpdateMode::Auto,
                source: UpdateSource::Github,
                location: None,
            })),
        };
        updater.dismiss_consent();
        assert!(!staging.exists());
        assert!(updater.consent().is_none());
        assert_eq!(updater.watch().status(), UpdateStatus::Idle);
    }

    struct MemorySettings;

    impl settings::SettingsRepository for MemorySettings {
        fn load(&self) -> Result<settings::SettingsDocument, settings::SettingsError> {
            Ok(settings::SettingsDocument::empty(
                settings::SettingsScope::User,
            ))
        }

        fn save(&self, _: &settings::SettingsDocument) -> Result<(), settings::SettingsError> {
            Ok(())
        }
    }
}
