use lapis_host::BackendClient;
use std::{ffi::OsString, path::PathBuf};
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
            .unwrap_or_else(BackendClient::user_directory)?;
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
fn option<'a>(arguments: &'a [OsString], name: &str) -> Result<Option<&'a OsString>, Error> {
    match arguments.iter().position(|value| value == name) {
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
    fn missing_connection_paths_are_rejected() {
        let arguments = ["lapis", "--host", "--workspace", "repo"].map(OsString::from);
        assert!(option(&arguments, "--host").is_err());
        assert_eq!(option(&arguments, "--workspace").unwrap().unwrap(), "repo");
        assert!(option(&arguments, "--state-directory").unwrap().is_none());
    }
}
