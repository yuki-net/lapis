use client_api::{
    CURRENT_PROTOCOL, ClientId, ClientKind, HelloRequest, ProtocolRange, WorkspaceId,
};
use host::BackendClient;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};
type Error = Box<dyn std::error::Error + Send + Sync>;

fn create_oidc_client_id() -> Result<String, Error> {
    let mut random = [0u8; 16];
    getrandom::fill(&mut random)?;
    Ok(format!(
        "desktop-{}",
        random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    ))
}

pub(super) fn open_picker(directory: PathBuf) -> Result<(), String> {
    let mut command =
        std::process::Command::new(std::env::current_exe().map_err(|error| error.to_string())?);
    command
        .arg("--connect-host")
        .arg("--state-directory")
        .arg(directory);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command.spawn().map_err(|error| error.to_string())?;
    Ok(())
}

pub(super) struct Startup {
    pub backend: BackendClient,
    pub root: Option<PathBuf>,
    pub directory: PathBuf,
    pub connector_directory: PathBuf,
    /// Remote Host の表示情報。Local Host は接続後に解決する。
    pub connection: Option<host::HostProfileInfo>,
}
impl Startup {
    pub fn open(arguments: &[OsString]) -> Result<Option<Self>, Error> {
        let directory = option(arguments, "--state-directory")?
            .map(PathBuf::from)
            .map(Ok)
            .unwrap_or_else(default_state_directory)?;
        let directory = if directory.is_absolute() {
            directory
        } else {
            std::env::current_dir()?.join(directory)
        };
        // 接続先は CLI、環境変数の順に解決する。どちらも無ければローカルHostを起動する。
        let host_file = option(arguments, "--host")?.map(PathBuf::from).or_else(|| {
            std::env::var_os("LAPIS_HOST_FILE")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        });
        let host_file = if arguments.iter().any(|value| value == "--connect-host") {
            let chosen = rfd::FileDialog::new()
                .set_title("Hostの招待または接続profileを開く")
                .set_directory(directory.join("hosts"))
                .add_filter("Host", &["json", "host"])
                .pick_file();
            let Some(chosen) = chosen else {
                return Ok(None);
            };
            Some(chosen)
        } else {
            host_file
        };
        if let Some(path) = host_file {
            if path.extension().is_none_or(|extension| extension != "host") {
                let invitation = host::read_invitation(&path)?;
                if invitation.token.is_empty() {
                    let client = create_oidc_client_id()?;
                    let hello = HelloRequest {
                        protocol: ProtocolRange::exact(CURRENT_PROTOCOL),
                        client_id: ClientId::try_new(&client)?,
                        client_name: "Desktop".to_owned(),
                        client_kind: ClientKind::Desktop,
                        requested_capabilities: host::supported_capabilities(),
                    };
                    let redirect_uri = std::env::var("LAPIS_DESKTOP_OIDC_REDIRECT_URI")
                        .unwrap_or_else(|_| "http://127.0.0.1:53682/auth/callback".to_owned());
                    let session = remote::login_desktop_oidc(remote::DesktopOidcLogin {
                        address: invitation.address,
                        server_name: invitation.server_name.clone(),
                        certificate: invitation.certificate.clone().into(),
                        hello,
                        workspace_id: if invitation.workspace_id.trim().is_empty() {
                            None
                        } else {
                            Some(WorkspaceId::try_new(&invitation.workspace_id)?)
                        },
                        redirect_uri,
                    })?;
                    let workspace_id = session
                        .workspace_id
                        .as_ref()
                        .map(|workspace| workspace.as_str().to_owned())
                        .unwrap_or_else(|| invitation.workspace_id.clone());
                    let (backend, root, profile) = host::write_oidc_profile(
                        &directory.join("hosts"),
                        host::OidcProfile {
                            address: invitation.address,
                            server_name: invitation.server_name.clone(),
                            certificate: invitation.certificate.clone(),
                            workspace_id,
                            name: invitation.name.clone(),
                            host_id: session.host_id,
                            client,
                            account: session.subject,
                            id_token: session.id_token.to_string(),
                        },
                    )?;
                    eprintln!("OIDCでHostに接続しました: {}", profile.display());
                    let connection = host::host_profile_info(&profile)?;
                    let connector_directory = directory.clone();
                    let directory = directory
                        .join("views")
                        .join(profile.file_stem().ok_or("Host profile名が不正です")?);
                    return Ok(Some(Self {
                        backend,
                        root: Some(root),
                        directory,
                        connector_directory,
                        connection: Some(connection),
                    }));
                }
            }
            let (backend, root, profile) = host::open_host_file(&path, &directory.join("hosts"))?;
            eprintln!("Host接続profile: {}", profile.display());
            let connection = host::host_profile_info(&profile)?;
            let connector_directory = directory.clone();
            let directory = directory
                .join("views")
                .join(profile.file_stem().ok_or("Host profile名が不正です")?);
            Ok(Some(Self {
                backend,
                root: Some(root),
                directory,
                connector_directory,
                connection: Some(connection),
            }))
        } else {
            let hosts_directory = local_hosts_directory(arguments, &directory)?;
            let backend = match local_host_executable(arguments)? {
                Some(executable) => BackendClient::local(executable, hosts_directory),
                None => BackendClient::local_in(hosts_directory)?,
            };
            Ok(Some(Self {
                backend,
                root: None,
                connector_directory: directory.clone(),
                directory,
                connection: None,
            }))
        }
    }
}

fn local_host_executable(arguments: &[OsString]) -> Result<Option<PathBuf>, Error> {
    let Some(path) = option(arguments, "--local-host-executable")? else {
        return Ok(None);
    };
    let path = Path::new(path);
    let executable = path.canonicalize().map_err(|error| {
        format!(
            "Local Host 実行ファイルを解決できません ({}): {error}",
            path.display()
        )
    })?;
    if !executable.is_file() {
        return Err(format!("Local Host 実行ファイルではありません: {}", path.display()).into());
    }
    Ok(Some(executable))
}

fn local_hosts_directory(arguments: &[OsString], directory: &Path) -> Result<PathBuf, Error> {
    let hosts = match option(arguments, "--local-host-directory")? {
        Some(hosts) => PathBuf::from(hosts),
        // Local Host の接続 credential は data 世代の外に置く。世代交代で再 pairing を
        // 起こさない（docs/specs/release.md）。
        None => match crate::install::InstallLayout::detect() {
            Some(layout) => {
                let target = layout.credentials_directory().join("hosts");
                migrate_legacy_hosts(&target, directory);
                target
            }
            None => directory.join("hosts"),
        },
    };
    if hosts.is_absolute() {
        Ok(hosts)
    } else {
        Ok(std::env::current_dir()?.join(hosts))
    }
}

/// 旧保存先の Local Host credential を、新しい `credentials/hosts` へ引き継ぐ。
///
/// 保存先を世代の外へ移した初回に、既存の pairing を失わせないため。既にある
/// credential は上書きせず、失敗しても旧 credential を消さない（複写のみ）。
fn migrate_legacy_hosts(target: &Path, state_directory: &Path) {
    let mut sources = vec![state_directory.join("hosts")];
    if let Ok(user) = BackendClient::user_directory() {
        sources.push(user.join("hosts"));
    }
    sources.extend([std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Lapis/hosts")]);
    for source in sources {
        if same_path(&source, target) || !source.is_dir() {
            continue;
        }
        let entries = match std::fs::read_dir(&source) {
            Ok(entries) => entries,
            Err(error) => {
                eprintln!(
                    "旧 Host credential を読めません ({}): {error}",
                    source.display()
                );
                continue;
            }
        };
        if let Err(error) = std::fs::create_dir_all(target) {
            eprintln!("credential の保存先を作れません: {error}");
            return;
        }
        for entry in entries.flatten() {
            if !entry.path().is_file() {
                continue;
            }
            // 排他的に作成し、既存の credential は上書きしない。`exists()` による
            // 事前確認では、確認と書き込みの間に別プロセスが入る余地が残る。
            copy_new_file(&entry.path(), &target.join(entry.file_name()));
        }
    }
}

/// 移行先が無い場合だけ複写する。既にあれば何もしない。
fn copy_new_file(source: &Path, destination: &Path) {
    let mut input = match std::fs::File::open(source) {
        Ok(file) => file,
        Err(error) => {
            eprintln!(
                "旧 Host credential を読めません ({}): {error}",
                source.display()
            );
            return;
        }
    };
    let mut output = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
    {
        // 既にある credential は上書きしない。
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return,
        Err(error) => {
            eprintln!(
                "旧 Host credential を引き継げません ({}): {error}",
                destination.display()
            );
            return;
        }
        Ok(file) => file,
    };
    // 途中で失敗した半端な file は残さない。次回起動で作り直せる。
    if let Err(error) = std::io::copy(&mut input, &mut output) {
        eprintln!(
            "旧 Host credential を引き継げません ({}): {error}",
            destination.display()
        );
        drop(output);
        let _ = std::fs::remove_file(destination);
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

fn default_state_directory() -> Result<PathBuf, Error> {
    let legacy = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Lapis");
    // install layout があれば、data は世代 directory に置く。軸A は同じ世代を
    // 使い続け、軸B だけが並置した新しい世代へ切り替える（docs/specs/release.md）。
    if let Some(layout) = crate::install::InstallLayout::detect() {
        layout.record();
        let data = layout.data_directory();
        std::fs::create_dir_all(&data)?;
        // 初回だけ従来の保存先から引き継ぐ。breaking 更新で用意した新しい世代は、
        // 旧保存先へフォールバックさせない（世代分離を保つ）。
        if layout.is_first_generation() {
            return select_state_directory(&data, &legacy);
        }
        return Ok(data);
    }
    let preferred = BackendClient::user_directory()?;
    select_state_directory(&preferred, &legacy)
}

fn select_state_directory(preferred: &Path, legacy: &Path) -> Result<PathBuf, Error> {
    const FILES: [&str; 4] = [
        "session-v1.txt",
        "conversations-v1.json",
        "settings-v1.json",
        "settings.json",
    ];
    let has_state = |directory: &Path| -> Result<bool, std::io::Error> {
        for name in FILES {
            if directory.join(name).try_exists()? {
                return Ok(true);
            }
        }
        Ok(false)
    };
    if preferred != legacy && has_state(legacy)? {
        if has_state(preferred)? {
            // 両方に状態がある場合、別のdraftや会話を自動的に混ぜない。
            eprintln!(
                "旧保存先にも状態があります: {}。必要なら --state-directory で指定してください。現在の保存先: {}",
                legacy.display(),
                preferred.display()
            );
        } else {
            // 一式を元の場所で読むことで、部分移行や上書きを避ける。
            eprintln!(
                "既存のdraft・会話・設定を旧保存先から復元します: {}",
                legacy.display()
            );
            return Ok(legacy.to_owned());
        }
    }
    Ok(preferred.to_owned())
}

fn option<'a>(arguments: &'a [OsString], name: &str) -> Result<Option<&'a OsString>, Error> {
    match arguments.iter().rposition(|value| value == name) {
        Some(index) => Ok(Some(
            arguments
                .get(index + 1)
                .filter(|value| !value.to_string_lossy().starts_with("--"))
                .ok_or("optionに値がありません")?,
        )),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_local_host_is_resolved_from_the_launch_directory() {
        let cwd = std::env::current_dir().unwrap();
        let temp = tempfile::tempdir_in(&cwd).unwrap();
        let host = temp.path().join("host.exe");
        std::fs::write(&host, "fixture").unwrap();
        let args = [
            OsString::from("lapis"),
            OsString::from("--local-host-executable"),
            host.strip_prefix(&cwd).unwrap().as_os_str().to_owned(),
        ];
        assert_eq!(
            local_host_executable(&args).unwrap(),
            Some(host.canonicalize().unwrap())
        );
    }

    #[test]
    fn explicit_local_host_is_resolved_without_sibling_binary_or_cwd_changes() {
        let temp = tempfile::tempdir().unwrap();
        let host = temp.path().join("Host with spaces.exe");
        std::fs::write(&host, "fixture").unwrap();
        let args = [
            OsString::from("lapis"),
            OsString::from("--local-host-executable"),
            host.into_os_string(),
        ];
        assert_eq!(
            local_host_executable(&args).unwrap(),
            Some(
                temp.path()
                    .join("Host with spaces.exe")
                    .canonicalize()
                    .unwrap()
            )
        );
        assert!(local_host_executable(&["lapis".into()]).unwrap().is_none());
        assert!(local_host_executable(&args[..2]).is_err());
        std::fs::remove_file(&args[2]).unwrap();
        assert!(
            local_host_executable(&args)
                .unwrap_err()
                .to_string()
                .contains("Local Host")
        );
        std::fs::create_dir(&args[2]).unwrap();
        assert!(local_host_executable(&args).is_err());
    }

    #[test]
    fn local_host_directory_can_be_separated_from_desktop_state() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        let session = temp.path().join("session-hosts");
        let args = [
            OsString::from("lapis"),
            OsString::from("--local-host-directory"),
            session.as_os_str().to_owned(),
        ];
        assert_eq!(local_hosts_directory(&args, &state).unwrap(), session);
        assert_eq!(
            local_hosts_directory(&[OsString::from("lapis")], &state).unwrap(),
            state.join("hosts")
        );
        assert!(
            local_hosts_directory(
                &[
                    OsString::from("lapis"),
                    OsString::from("--local-host-directory")
                ],
                &state
            )
            .is_err()
        );
    }
    #[test]
    fn explicit_worktree_state_keeps_settings_and_hosts_separate() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first worktree/target/dev-state");
        let second = temp.path().join("second worktree/target/dev-state");
        for directory in [&first, &second] {
            let arguments = [
                OsString::from("lapis"),
                OsString::from("--state-directory"),
                directory.as_os_str().to_owned(),
            ];
            let startup = Startup::open(&arguments).unwrap().unwrap();
            assert_eq!(startup.directory, *directory);
            assert_eq!(startup.connector_directory, *directory);
            let settings =
                persistence::LocalUserSettingsRepository::in_directory(&startup.directory);
            let settings =
                desktop_client::SettingsSession::load(std::sync::Arc::new(settings)).unwrap();
            if directory == &first {
                settings.set_theme("lapis.white".into()).unwrap();
            } else {
                assert_eq!(settings.settings().theme(), "lapis.dark");
            }
        }
        assert!(first.join("settings.json").is_file());
        assert!(!second.join("settings.json").exists());
    }

    #[test]
    fn legacy_drafts_conversations_and_settings_keep_their_original_directory() {
        let temp = tempfile::tempdir().unwrap();
        let preferred = temp.path().join("state");
        let legacy = temp.path().join("legacy");
        assert_eq!(
            select_state_directory(&preferred, &legacy).unwrap(),
            preferred
        );
        std::fs::create_dir(&legacy).unwrap();
        for name in [
            "session-v1.txt",
            "conversations-v1.json",
            "settings-v1.json",
        ] {
            let file = legacy.join(name);
            std::fs::write(&file, "unsaved state").unwrap();
            assert_eq!(select_state_directory(&preferred, &legacy).unwrap(), legacy);
            assert_eq!(std::fs::read_to_string(&file).unwrap(), "unsaved state");
            assert!(!preferred.exists());
            std::fs::remove_file(file).unwrap();
        }
        std::fs::write(legacy.join("session-v1.txt"), "old draft").unwrap();
        std::fs::create_dir(&preferred).unwrap();
        std::fs::write(preferred.join("session-v1.txt"), "new draft").unwrap();
        assert_eq!(
            select_state_directory(&preferred, &legacy).unwrap(),
            preferred
        );
        assert_eq!(
            std::fs::read_to_string(legacy.join("session-v1.txt")).unwrap(),
            "old draft"
        );
        assert_eq!(
            std::fs::read_to_string(preferred.join("session-v1.txt")).unwrap(),
            "new draft"
        );
        assert_eq!(select_state_directory(&legacy, &legacy).unwrap(), legacy);
    }
    #[test]
    fn missing_connection_paths_are_rejected() {
        let arguments = ["lapis", "--host", "--workspace", "repo"].map(OsString::from);
        assert!(option(&arguments, "--host").is_err());
        assert_eq!(option(&arguments, "--workspace").unwrap().unwrap(), "repo");
        assert!(option(&arguments, "--state-directory").unwrap().is_none());
    }

    #[test]
    fn repeated_options_use_the_last_value() {
        let arguments = [
            "lapis",
            "--state-directory",
            "default-state",
            "--state-directory",
            "explicit-state",
        ]
        .map(OsString::from);
        assert_eq!(
            option(&arguments, "--state-directory").unwrap().unwrap(),
            "explicit-state"
        );
    }
}
