//! インストール layout の契約と、staging 済み package の適用。
//!
//! launcher の実体（`lapis-launcher`）とアプリ側の更新処理が同じ検証・適用規則を
//! 使うための library。ネットワーク・manifest の取得・更新の同意は持たない
//! （`docs/specs/release.md`）。
//!
//! - [`Verifier`]: 信頼済み公開鍵を持ち、manifest の取得・検証・staging・適用を行う。
//!   アプリは repository の registry で作った `Verifier` を使う。
//! - [`run`]: staging を適用して本体を起動する。launcher の実体が使う。

mod archive;
mod generation;
mod layout;
mod manifest;

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

pub use generation::{
    credentials_dir, data_dir, generation_dir, generation_for, prepare_generation, read_generation,
    record_generation, write_generation,
};

/// launcher と更新処理の失敗。起動可否の判断にだけ使い、原因は stderr へ出す。
pub struct Error {
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

impl std::error::Error for Error {}

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

pub type Result<T> = std::result::Result<T, Error>;

const INSTALL_ROOT_FLAG: &str = "--install-root";
const INSTALL_ROOT_ENV: &str = "LAPIS_INSTALL_ROOT";

/// 検証済みで staging へ置いた配布物。
pub struct Staged {
    pub version: String,
    /// 軸B（data 世代を上げる破壊的変更）かどうか。
    pub breaking: bool,
    /// staging の directory。
    pub directory: PathBuf,
}

/// 適用した version。
pub struct Applied {
    pub version: String,
}

/// staging へ置くときの条件。
#[derive(Clone, Copy, Debug, Default)]
pub struct StageOptions {
    /// `breaking: true` の配布物を置くことを許すか。同意済みのときだけ true。
    pub allow_breaking: bool,
}

/// `versions/<version>/` へ入れるときの条件。
#[derive(Clone, Copy, Debug, Default)]
pub struct ApplyOptions {
    /// `breaking: true` の配布物を適用することを許すか。同意済みのときだけ true。
    pub allow_breaking: bool,
}

/// 信頼済み公開鍵 registry を持つ検証器。
///
/// 信頼の起点は repository の `release/trusted-keys.json` だけである。配布物に含めた
/// 公開鍵を信頼の起点にしない。launcher は build 時に registry を埋め込む。
pub struct Verifier {
    keys: manifest::TrustedKeys,
}

impl Verifier {
    /// repository の registry を使う。
    pub fn repository() -> Result<Self> {
        Ok(Self {
            keys: manifest::TrustedKeys::repository()?,
        })
    }

    /// registry の bytes を明示して作る。検証規則を確かめるテストが使う。
    pub fn from_registry(raw: &[u8]) -> Result<Self> {
        Ok(Self {
            keys: manifest::TrustedKeys::parse(raw)?,
        })
    }

    /// registry の複写。同じ信頼の起点を共有する検証器を作る。
    pub fn clone_verifier(&self) -> Self {
        Self {
            keys: self.keys.clone(),
        }
    }

    /// manifest の bytes を形式と署名で検証し、download に必要な内容を返す。
    ///
    /// 取得そのものはアプリ側が行う。launcher は network をリンクしない。
    pub fn inspect(&self, raw: &[u8]) -> Result<ManifestInfo> {
        manifest::verify_signature(raw, &self.keys)?;
        let parsed = manifest::parse(raw)?;
        Ok(ManifestInfo {
            version: parsed.version,
            breaking: parsed.breaking,
            notes_url: parsed.notes_url,
            assets: parsed
                .assets
                .into_iter()
                .map(|(platform, asset)| (platform, asset.source))
                .collect(),
        })
    }

    /// 配布物の署名と SHA-256 を検証する。layout は変えない。
    pub fn verify(&self, distribution: &Path) -> Result<Verified> {
        let verified = manifest::verify_distribution(distribution, &self.keys)?;
        Ok(Verified {
            version: verified.manifest.version,
            breaking: verified.manifest.breaking,
        })
    }

    /// `staging/<version>/` に置いた適用候補を返す。無ければ `None`。
    pub fn staged(&self, root: &Path, version: &str) -> Result<Option<Staged>> {
        layout::staged_with_keys(root, version, &self.keys)
    }

    /// staging に置かれた候補を新しい順に返す。検証できない候補は飛ばす。
    pub fn pending_staging(&self, root: &Path) -> Result<Vec<Staged>> {
        let mut pending = Vec::new();
        for version in layout::candidates(root)? {
            match layout::staged_with_keys(root, &version, &self.keys) {
                Ok(Some(staged)) => pending.push(staged),
                Ok(None) => {}
                Err(error) => eprintln!("staging の {version} を検証できません: {error}"),
            }
        }
        Ok(pending)
    }

    /// 配布物を検証し、`staging/<version>/` へ原子的に置く。
    ///
    /// 現行 version と同じか古い配布物は置かない。`breaking: true` は同意済み
    /// （`allow_breaking`）のときだけ置く。
    pub fn stage(&self, root: &Path, distribution: &Path, options: StageOptions) -> Result<Staged> {
        layout::stage_with_keys(root, distribution, &self.keys, options)
    }

    /// `staging/<version>/` を検証して `versions/<version>/` へ入れ、pointer を進める。
    pub fn apply(&self, root: &Path, version: &str, options: ApplyOptions) -> Result<Applied> {
        let platform = platform_name().ok_or_else(|| Error::new("対応していない platform"))?;
        layout::apply_with_platform(root, version, &platform, &self.keys, options)
    }

    /// 現在の pointer が指す version。pointer が無ければ `None`。
    pub fn current_version(&self, root: &Path) -> Result<Option<String>> {
        layout::read_pointer(root)
    }
}

/// 検証済み配布物の要約。
pub struct Verified {
    pub version: String,
    pub breaking: bool,
}

/// 署名を検証した manifest の内容。download の対象と同意の判断にだけ使う。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestInfo {
    pub version: String,
    pub breaking: bool,
    pub notes_url: String,
    /// platform 名 → manifest と同じ directory からの相対 file 名。
    pub assets: Vec<(String, String)>,
}

/// 配布物の platform 名。`scripts/release` の platform 名と揃える。
pub fn platform_name() -> Option<String> {
    let os = match std::env::consts::OS {
        "windows" => "windows",
        "linux" => "linux",
        "macos" => "macos",
        other => return Some(other.to_owned()),
    };
    Some(format!("{os}-{}", std::env::consts::ARCH))
}

/// `candidate` が `current` より新しいか。比較できない形式では、同じでなければ true。
pub fn is_newer(candidate: &str, current: &str) -> bool {
    layout::is_newer(candidate, current)
}

/// per-user の install root。`--install-root` を除いた既定値。
///
/// 環境変数 `LAPIS_INSTALL_ROOT`、`<home>/.lapis` の順に決める。
pub fn install_root() -> PathBuf {
    if let Some(explicit) = std::env::var_os(INSTALL_ROOT_ENV) {
        return PathBuf::from(explicit);
    }
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(".lapis")
}

/// launcher の実体。staging を適用して本体を起動し、終了 code を返す。
pub fn run(arguments: Vec<OsString>) -> Result<u8> {
    let (requested_root, forwarded) = parse_arguments(&arguments)?;
    let requested_root = requested_root.unwrap_or_else(|| install_root().into_os_string());
    let root = absolute(Path::new(&requested_root))?;
    // 更新の失敗は起動を妨げない。原因だけ残して現行版の起動へ進む。
    if let Err(error) = apply_pending(&root) {
        eprintln!("launcher: 更新を適用しません: {error}");
    }
    launch(&root, &forwarded)
}

/// pointer の version を起動する。install 未完了や pointer の破損は、勝手に直さず停止する。
pub fn launch(root: &Path, arguments: &[OsString]) -> Result<u8> {
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
    let verifier = Verifier::repository()?;
    let platform = platform_name().ok_or_else(|| Error::new("対応していない platform"))?;
    for version in candidates {
        match layout::apply_with_platform(
            root,
            &version,
            &platform,
            verifier.keys(),
            ApplyOptions::default(),
        ) {
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

impl Verifier {
    fn keys(&self) -> &manifest::TrustedKeys {
        &self.keys
    }
}

/// launcher の終了 code。
pub fn exit_code(result: Result<u8>) -> ExitCode {
    match result {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("launcher: {error}");
            ExitCode::FAILURE
        }
    }
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
