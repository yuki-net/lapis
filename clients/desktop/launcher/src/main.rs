//! 起動時に staging 済みの更新を適用し、本体を起動する小さな launcher。
//!
//! 行うことは 2 つだけである。
//!
//! 1. `<install root>/staging/<version>/` に検証可能な package があれば、layout の
//!    規則どおり `versions/<version>/` へ展開し pointer を進める。
//! 2. `current.json` の version にある `lapis.exe` を起動する。
//!
//! 更新の失敗・staging の不在・manifest の不整合のいずれでも、更新は行わずに現行版を
//! 起動する。manifest の取得や解釈、download はアプリ側の責務であり、launcher は staging
//! と pointer の契約だけを使う（`docs/specs/release.md`）。

mod archive;
mod layout;
mod manifest;

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// launcher の失敗。起動可否の判断にだけ使い、原因は stderr へ出す。
pub(crate) struct Error {
    message: String,
}

impl Error {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::new(error.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::new(error.to_string())
    }
}

pub(crate) type Result<T> = std::result::Result<T, Error>;

const INSTALL_ROOT_FLAG: &str = "--install-root";
const INSTALL_ROOT_ENV: &str = "LAPIS_INSTALL_ROOT";

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("launcher: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<u8> {
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    let (install_root, forwarded) = parse_arguments(&arguments)?;
    let root = absolute(Path::new(
        &install_root.unwrap_or_else(default_install_root),
    ))?;

    // 更新の失敗は起動を妨げない。原因だけ残して現行版の起動へ進む。
    if let Err(error) = apply_pending(&root) {
        eprintln!("launcher: 更新を適用しません: {error}");
    }
    launch(&root, &forwarded)
}

/// per-user の install root。既定は `docs/releases.md` と同じ `<home>/.lapis` とし、
/// 管理者権限を要求しない。
fn default_install_root() -> OsString {
    if let Some(explicit) = std::env::var_os(INSTALL_ROOT_ENV) {
        return explicit;
    }
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(".lapis")
        .into_os_string()
}

/// pointer の version を起動する。install 未完了や pointer の破損は、勝手に直さず停止する。
fn launch(root: &Path, arguments: &[OsString]) -> Result<u8> {
    let version = layout::read_pointer(root)?.ok_or_else(|| {
        Error::new(format!(
            "install が完了していません ({} がありません)。docs/releases.md の install を実行してください",
            layout::pointer_path(root).display()
        ))
    })?;
    if !layout::installed_version_matches(root, &version)? {
        return Err(Error::new(format!(
            "versions/{version}/ の manifest が pointer と一致しません。docs/releases.md の復旧手順を確認してください"
        )));
    }
    let executable = layout::app_executable(root, &version)?;
    if !executable.is_file() {
        return Err(Error::new(format!(
            "起動する本体がありません: {}。docs/releases.md の復旧手順を確認してください",
            executable.display()
        )));
    }
    let status = Command::new(&executable)
        .args(arguments)
        .status()
        .map_err(|error| {
            Error::new(format!(
                "起動できません ({}): {error}",
                executable.display()
            ))
        })?;
    Ok(status
        .code()
        .and_then(|code| u8::try_from(code).ok())
        .unwrap_or(1))
}

/// `--install-root` を取り除いた引数を、アプリへそのまま渡す。
///
/// 値の無い `--install-root` は、既定の root へ黙って落とさず error にする。
fn parse_arguments(arguments: &[OsString]) -> Result<(Option<OsString>, Vec<OsString>)> {
    let mut install_root = None;
    let mut forwarded = Vec::with_capacity(arguments.len());
    let mut iterator = arguments.iter();
    while let Some(argument) = iterator.next() {
        if argument == OsStr::new(INSTALL_ROOT_FLAG) {
            install_root = Some(
                iterator
                    .next()
                    .cloned()
                    .ok_or_else(|| Error::new("--install-root に値がありません"))?,
            );
        } else if let Some(value) = argument
            .to_str()
            .and_then(|value| value.strip_prefix("--install-root="))
        {
            install_root = Some(OsString::from(value));
        } else {
            forwarded.push(argument.clone());
        }
    }
    Ok((install_root, forwarded))
}

fn absolute(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    Ok(std::env::current_dir()?.join(path))
}

/// 適用できる staging の候補を 1 つだけ適用する。
///
/// 新しい順に試し、署名・SHA-256・展開内容が一致した最初の候補だけを採用する。
/// 一致しない候補は削除せず残す（未完了の download を壊さないため）。
fn apply_pending(root: &Path) -> Result<()> {
    let candidates = layout::candidates(root)?;
    if candidates.is_empty() {
        return Ok(());
    }
    let keys = manifest::TrustedKeys::repository()?;
    let platform = platform_name().ok_or_else(|| Error::new("対応していない platform"))?;
    for version in candidates {
        match layout::apply(root, &version, &platform, &keys) {
            Ok(applied) => {
                eprintln!("launcher: {} を適用しました", applied.version);
                return Ok(());
            }
            // 適用しない候補は pointer を変えないため、既存の version で起動できる。
            Err(error) => eprintln!("launcher: staging の {version} を使いません: {error}"),
        }
    }
    Ok(())
}

/// package と manifest が想定する platform 名。`scripts/release` の platform 名と揃える。
fn platform_name() -> Option<String> {
    let os = match std::env::consts::OS {
        "windows" => "windows",
        "linux" => "linux",
        "macos" => "macos",
        other => return Some(other.to_owned()),
    };
    Some(format!("{os}-{}", std::env::consts::ARCH))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(arguments: &[&str]) -> (Option<OsString>, Vec<OsString>) {
        let arguments: Vec<OsString> = arguments.iter().map(OsString::from).collect();
        parse_arguments(&arguments).expect("arguments")
    }

    #[test]
    fn install_root_is_removed_from_forwarded_arguments() {
        let (root, forwarded) = parse(&["--install-root", "C:/lapis", "--workspace", "repo"]);
        assert_eq!(root, Some(OsString::from("C:/lapis")));
        assert_eq!(
            forwarded,
            vec![OsString::from("--workspace"), OsString::from("repo")]
        );
    }

    #[test]
    fn install_root_can_be_an_equals_option() {
        let (root, forwarded) = parse(&["--install-root=/tmp/lapis", "--show-tasks"]);
        assert_eq!(root, Some(OsString::from("/tmp/lapis")));
        assert_eq!(forwarded, vec![OsString::from("--show-tasks")]);
    }

    #[test]
    fn app_arguments_are_forwarded_unchanged() {
        let (root, forwarded) = parse(&["--host", "invitation.json", "--loopback"]);
        assert_eq!(root, None);
        assert_eq!(
            forwarded,
            vec![
                OsString::from("--host"),
                OsString::from("invitation.json"),
                OsString::from("--loopback")
            ]
        );
    }

    #[test]
    fn install_root_without_a_value_is_an_error() {
        let arguments = [OsString::from("--install-root")];
        assert!(parse_arguments(&arguments).is_err());
    }

    #[test]
    fn missing_pointer_does_not_guess_an_executable() {
        let root = tempfile::tempdir().expect("temp");
        assert!(launch(root.path(), &[]).is_err());
    }

    #[test]
    fn corrupt_pointer_is_not_repaired() {
        let root = tempfile::tempdir().expect("temp");
        let pointer = layout::pointer_path(root.path());
        std::fs::write(&pointer, "not json").expect("pointer");
        assert!(launch(root.path(), &[]).is_err());
        assert_eq!(
            std::fs::read_to_string(&pointer).expect("pointer"),
            "not json"
        );
    }

    #[test]
    fn missing_manifest_for_the_pointer_version_is_an_error() {
        let root = tempfile::tempdir().expect("temp");
        std::fs::create_dir_all(layout::version_dir(root.path(), "0.1.0").expect("dir"))
            .expect("mkdir");
        layout::write_pointer(root.path(), "0.1.0").expect("pointer");
        assert!(launch(root.path(), &[]).is_err());
    }
}
