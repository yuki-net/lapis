use lapis_host::BackendClient;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};
type Error = Box<dyn std::error::Error + Send + Sync>;

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
        let host_file = option(arguments, "--host")?.map(PathBuf::from);
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
            let (backend, root, profile) =
                lapis_host::open_host_file(&path, &directory.join("hosts"))?;
            eprintln!("Host接続profile: {}", profile.display());
            let connector_directory = directory.clone();
            let directory = directory
                .join("views")
                .join(profile.file_stem().ok_or("Host profile名が不正です")?);
            Ok(Some(Self {
                backend,
                root: Some(root),
                directory,
                connector_directory,
            }))
        } else {
            let backend = BackendClient::local(std::env::current_exe()?, directory.join("hosts"));
            Ok(Some(Self {
                backend,
                root: None,
                connector_directory: directory.clone(),
                directory,
            }))
        }
    }
}
fn default_state_directory() -> Result<PathBuf, Error> {
    let preferred = BackendClient::user_directory()?;
    let legacy = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Lapis");
    select_state_directory(&preferred, &legacy)
}

fn select_state_directory(preferred: &Path, legacy: &Path) -> Result<PathBuf, Error> {
    const FILES: [&str; 3] = [
        "session-v1.txt",
        "conversations-v1.json",
        "settings-v1.json",
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
            let settings = lapis_persistence::LocalGlobalSettingsRepository::new(
                startup.directory.join("settings-v1.json"),
            );
            let settings =
                lapis_desktop_client::SettingsSession::load(std::sync::Arc::new(settings)).unwrap();
            if directory == &first {
                settings.set_theme("lapis.white".into()).unwrap();
            } else {
                assert_eq!(settings.settings().theme, "lapis.dark");
            }
        }
        assert!(first.join("settings-v1.json").is_file());
        assert!(!second.join("settings-v1.json").exists());
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
