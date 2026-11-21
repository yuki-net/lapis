//! 配布元から manifest と package を取得する。
//!
//! 取得元は GitHub Releases・ローカル folder・自宅サーバーの静的配置の 3 つで、
//! manifest の形式・検証・staging は共通である（`docs/specs/release.md`）。
//! 検証と staging は launcher の library に任せ、ここは network と file の入出力だけを持つ。
//! launcher の実行ファイルは network をリンクしない。

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use lapis_launcher::{ManifestInfo, StageOptions, Staged, Verifier};

/// manifest と package の取得元。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManifestSource {
    /// GitHub Releases の asset。
    ///
    /// `base` は `https://github.com/<owner>/<repo>/releases`。manifest は最新 Release の
    /// `latest/download/manifest.json`、asset は manifest の version から `v<version>` を
    /// 決めて `download/v<version>/<name>` から取る。
    Github { base: String },
    /// `manifest.json` と package が並ぶ directory。
    Folder { directory: PathBuf },
    /// 静的配置の URL。`<base>/manifest.json` と、その隣の package を取得する。
    Http { base: String },
}

impl ManifestSource {
    /// GitHub Releases の既定の配布元。
    pub const GITHUB_BASE: &'static str = "https://github.com/yuki-net/lapis/releases";

    /// 設定値から取得元を組み立てる。location が無い取得元は既定を使う。
    pub fn from_setting(
        source: &str,
        location: Option<&str>,
    ) -> Result<Self, lapis_launcher::Error> {
        let error = |message: String| lapis_launcher::Error::from(std::io::Error::other(message));
        match source {
            "github" => Ok(Self::Github {
                base: Self::GITHUB_BASE.to_owned(),
            }),
            "local" => {
                let directory = location
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| error("ローカル配布元の folder が未設定".to_owned()))?;
                Ok(Self::Folder {
                    directory: PathBuf::from(directory),
                })
            }
            "server" => {
                let base = location
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| error("自宅サーバーの配布元 URL が未設定".to_owned()))?;
                if !(base.starts_with("https://") || base.starts_with("http://")) {
                    return Err(error(
                        "自宅サーバーの配布元は http(s) URL で指定する".to_owned(),
                    ));
                }
                Ok(Self::Http {
                    base: base.trim_end_matches('/').to_owned(),
                })
            }
            other => Err(error(format!("未知の配布元: {other}"))),
        }
    }
}

/// 取得に失敗した理由。呼び出し側は起動・利用を止めない。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchError {
    pub message: String,
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl From<lapis_launcher::Error> for FetchError {
    fn from(error: lapis_launcher::Error) -> Self {
        Self {
            message: error.to_string(),
        }
    }
}

impl From<std::io::Error> for FetchError {
    fn from(error: std::io::Error) -> Self {
        Self {
            message: error.to_string(),
        }
    }
}

/// 署名を検証した manifest。download の判断にだけ使う。
pub type RemoteManifest = ManifestInfo;

/// manifest を取得して形式と署名を検証する。package は取得しない。
pub fn fetch_manifest(
    source: &ManifestSource,
    verifier: &Verifier,
) -> std::result::Result<RemoteManifest, FetchError> {
    let directory = tempfile::tempdir()?;
    let raw = read_manifest(source, directory.path())?;
    Ok(verifier.inspect(&raw)?)
}

/// manifest と asset を取得し、検証して `staging/<version>/` へ置く。
///
/// `accept` は署名を検証した manifest を見て download を続けるかを決める。未知の
/// `key_id` は判断より前に拒否される。breaking の可否は `allow_breaking` で明示する。
pub fn fetch_and_stage(
    root: &Path,
    source: &ManifestSource,
    verifier: &Verifier,
    options: StageOptions,
    accept: impl FnOnce(&RemoteManifest) -> bool,
) -> std::result::Result<Option<Staged>, FetchError> {
    let directory = tempfile::tempdir()?;
    let raw = read_manifest(source, directory.path())?;
    let manifest = verifier.inspect(&raw)?;
    if !accept(&manifest) {
        return Ok(None);
    }
    std::fs::write(directory.path().join("manifest.json"), &raw)?;
    for (_, name) in &manifest.assets {
        read_asset(source, &manifest.version, name, directory.path())?;
    }
    // 複写した配置物は launcher の `stage` が再検証する。取得中に差し替えられても
    // 候補にしない。
    let staged = verifier.stage(root, directory.path(), options)?;
    Ok(Some(staged))
}

fn read_manifest(
    source: &ManifestSource,
    directory: &Path,
) -> std::result::Result<Vec<u8>, FetchError> {
    let destination = directory.join("manifest.json");
    match source {
        ManifestSource::Folder { directory: base } => {
            let path = base.join("manifest.json");
            std::fs::copy(&path, &destination).map_err(|error| FetchError {
                message: format!("manifest を読めません ({}): {error}", path.display()),
            })?;
        }
        ManifestSource::Http { base } => {
            download(&format!("{base}/manifest.json"), &destination)?;
        }
        ManifestSource::Github { base } => {
            // manifest は固定の最新 Release から取る。tag は version から決まるため、
            // manifest を読む前には tag を選べない。
            download(
                &format!("{base}/latest/download/manifest.json"),
                &destination,
            )?;
        }
    }
    std::fs::read(&destination).map_err(|error| FetchError {
        message: format!("manifest を読めません: {error}"),
    })
}

fn read_asset(
    source: &ManifestSource,
    version: &str,
    name: &str,
    directory: &Path,
) -> std::result::Result<PathBuf, FetchError> {
    let destination = directory.join(name);
    match source {
        ManifestSource::Folder { directory: base } => {
            let path = base.join(name);
            std::fs::copy(&path, &destination).map_err(|error| FetchError {
                message: format!("package を読めません ({}): {error}", path.display()),
            })?;
        }
        ManifestSource::Http { base } => download(&format!("{base}/{name}"), &destination)?,
        ManifestSource::Github { base } => {
            download(&format!("{base}/download/v{version}/{name}"), &destination)?
        }
    }
    Ok(destination)
}

/// 1 file を download する。上限を超える応答は読まずに失敗させる。
fn download(url: &str, destination: &Path) -> std::result::Result<(), FetchError> {
    const LIMIT: u64 = 512 * 1024 * 1024;
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(600))
        .connect_timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|error| FetchError {
            message: error.to_string(),
        })?;
    let response = client.get(url).send().map_err(|error| FetchError {
        message: format!("{url}: {error}"),
    })?;
    if !response.status().is_success() {
        return Err(FetchError {
            message: format!("{url}: HTTP {}", response.status()),
        });
    }
    if response
        .content_length()
        .is_some_and(|length| length > LIMIT)
    {
        return Err(FetchError {
            message: format!("{url}: 応答が大きすぎる"),
        });
    }
    let mut file = std::fs::File::create(destination).map_err(|error| FetchError {
        message: format!("{}: {error}", destination.display()),
    })?;
    // 読み取り自体を LIMIT + 1 byte に制限する。Content-Length が無い応答でも、
    // 上限を超えた時点で打ち切り、一時領域を書き切らない。
    let mut limited = response.take(LIMIT + 1);
    let written = std::io::copy(&mut limited, &mut file).map_err(|error| FetchError {
        message: format!("{url}: {error}"),
    })?;
    if written > LIMIT {
        drop(file);
        let _ = std::fs::remove_file(destination);
        return Err(FetchError {
            message: format!("{url}: 応答が大きすぎる"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn distribution(name: &str) -> PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../launcher/tests/fixtures")
            .join(name)
    }

    fn verifier() -> Verifier {
        let raw = std::fs::read(distribution("keys.json")).expect("fixture registry");
        Verifier::from_registry(&raw).expect("registry")
    }

    #[test]
    fn settings_map_to_the_three_entry_points() {
        assert_eq!(
            ManifestSource::from_setting("github", None).expect("github"),
            ManifestSource::Github {
                base: ManifestSource::GITHUB_BASE.to_owned()
            }
        );
        assert_eq!(
            ManifestSource::from_setting("local", Some("C:/releases")).expect("local"),
            ManifestSource::Folder {
                directory: PathBuf::from("C:/releases")
            }
        );
        assert_eq!(
            ManifestSource::from_setting("server", Some("https://example.invalid/lapis/"))
                .expect("server"),
            ManifestSource::Http {
                base: "https://example.invalid/lapis".to_owned()
            }
        );
        // 必要な location が無い・形式が違う場合は、黙って既定へ落とさない。
        assert!(ManifestSource::from_setting("local", None).is_err());
        assert!(ManifestSource::from_setting("server", Some("ftp://example.invalid")).is_err());
        assert!(ManifestSource::from_setting("unknown", None).is_err());
    }

    #[test]
    fn a_folder_manifest_is_verified_and_offers_the_version() {
        let manifest = fetch_manifest(
            &ManifestSource::Folder {
                directory: distribution("distribution2"),
            },
            &verifier(),
        )
        .expect("manifest");
        assert_eq!(manifest.version, "0.1.0-alpha.2");
        assert!(!manifest.breaking);
        assert!(manifest.notes_url.contains("0.1.0-alpha.2"));
    }

    #[test]
    fn a_folder_distribution_is_staged_after_verification() {
        let root = tempfile::tempdir().expect("temp");
        let verifier = verifier();
        let staged = fetch_and_stage(
            root.path(),
            &ManifestSource::Folder {
                directory: distribution("distribution2"),
            },
            &verifier,
            StageOptions {
                allow_breaking: false,
            },
            |_| true,
        )
        .expect("fetch")
        .expect("staged");
        assert_eq!(staged.version, "0.1.0-alpha.2");
        assert!(staged.directory.join("manifest.json").is_file());
        assert!(
            verifier
                .staged(root.path(), "0.1.0-alpha.2")
                .expect("staged")
                .is_some()
        );
        // staging へ置いただけでは pointer を動かさない。
        assert_eq!(
            verifier.current_version(root.path()).expect("pointer"),
            None
        );
    }

    #[test]
    fn declining_the_manifest_does_not_download_or_stage() {
        let root = tempfile::tempdir().expect("temp");
        let staged = fetch_and_stage(
            root.path(),
            &ManifestSource::Folder {
                directory: distribution("distribution2"),
            },
            &verifier(),
            StageOptions {
                allow_breaking: false,
            },
            |manifest| {
                assert_eq!(manifest.version, "0.1.0-alpha.2");
                false
            },
        )
        .expect("fetch");
        assert!(staged.is_none());
        assert!(!root.path().join("staging").exists());
    }

    #[test]
    fn a_tampered_manifest_is_rejected_before_download() {
        let directory = tempfile::tempdir().expect("temp");
        let source = distribution("distribution2");
        for entry in std::fs::read_dir(&source).expect("fixture") {
            let entry = entry.expect("entry");
            std::fs::copy(entry.path(), directory.path().join(entry.file_name())).expect("copy");
        }
        let manifest = directory.path().join("manifest.json");
        let text = std::fs::read_to_string(&manifest).expect("manifest");
        std::fs::write(
            &manifest,
            text.replace("\"breaking\": false", "\"breaking\": true"),
        )
        .expect("tamper");
        assert!(
            fetch_manifest(
                &ManifestSource::Folder {
                    directory: directory.path().to_owned()
                },
                &verifier()
            )
            .is_err()
        );
    }

    #[test]
    fn a_missing_folder_manifest_fails_without_touching_the_layout() {
        let root = tempfile::tempdir().expect("temp");
        let result = fetch_and_stage(
            root.path(),
            &ManifestSource::Folder {
                directory: root.path().join("missing"),
            },
            &verifier(),
            StageOptions {
                allow_breaking: false,
            },
            |_| true,
        );
        assert!(result.is_err());
        assert!(!root.path().join("staging").exists());
    }
}
