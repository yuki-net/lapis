//! インストール layout と、staging 済み package の適用。
//!
//! 定数と規則は `scripts/release/layout.py` と同じものを使う。launcher は version の
//! 探索や保持数を独自に決めない。pointer が壊れている場合は何も展開せずに停止し、
//! 勝手に作り直さない。

use std::fs::File;
use std::path::{Path, PathBuf};

use crate::archive;
use crate::manifest::{self, MANIFEST_NAME};

pub(crate) const POINTER_NAME: &str = "current.json";
pub(crate) const POINTER_SCHEMA: i64 = 1;
pub(crate) const LOCK_NAME: &str = ".layout.lock";
pub(crate) const VERSIONS_DIR: &str = "versions";
pub(crate) const STAGING_DIR: &str = "staging";
pub(crate) const KEEP_VERSIONS: usize = 2;

use crate::{Error, Result};

pub(crate) fn pointer_path(root: &Path) -> PathBuf {
    root.join(POINTER_NAME)
}

pub(crate) fn versions_dir(root: &Path) -> PathBuf {
    root.join(VERSIONS_DIR)
}

pub(crate) fn version_dir(root: &Path, version: &str) -> Result<PathBuf> {
    validate_name(version, "version")?;
    Ok(versions_dir(root).join(version))
}

pub(crate) fn staging_dir(root: &Path) -> PathBuf {
    root.join(STAGING_DIR)
}

fn validate_name(value: &str, label: &str) -> Result<()> {
    if value.is_empty()
        || value.contains('/')
        || value.contains('\\')
        || matches!(value, "." | "..")
    {
        return Err(Error::new(format!(
            "{label} を directory 名に使えない: {value:?}"
        )));
    }
    Ok(())
}

/// 現在 version を返す。pointer が無ければ `None`、壊れていれば error。
pub(crate) fn read_pointer(root: &Path) -> Result<Option<String>> {
    let path = pointer_path(root);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(Error::new(format!("pointer を読めません: {error}"))),
    };
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|error| Error::new(format!("pointer が JSON でない: {error}")))?;
    let object = value
        .as_object()
        .filter(|object| {
            object.len() == 2 && object.contains_key("schema") && object.contains_key("version")
        })
        .ok_or_else(|| Error::new(format!("pointer の field が不正: {}", path.display())))?;
    if object["schema"].as_i64() != Some(POINTER_SCHEMA) {
        return Err(Error::new(format!(
            "pointer の schema が不正: {}",
            path.display()
        )));
    }
    let version = object["version"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::new(format!("pointer の version が不正: {}", path.display())))?;
    Ok(Some(version.to_owned()))
}

/// pointer を原子的に置き換える。書き込み途中の値を見せない。
pub(crate) fn write_pointer(root: &Path, version: &str) -> Result<()> {
    let target = version_dir(root, version)?;
    if !target.is_dir() {
        return Err(Error::new(format!(
            "version directory がありません: {}",
            target.display()
        )));
    }
    std::fs::create_dir_all(root)
        .map_err(|error| Error::new(format!("install root を作れません: {error}")))?;
    let payload = format!(
        "{{\"schema\":{POINTER_SCHEMA},\"version\":{}}}\n",
        json_string(version)
    );
    let mut temporary = tempfile::Builder::new()
        .prefix(".current-")
        .suffix(".tmp")
        .tempfile_in(root)
        .map_err(|error| Error::new(format!("pointer を書けません: {error}")))?;
    use std::io::Write;
    temporary
        .write_all(payload.as_bytes())
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|error| Error::new(format!("pointer を書けません: {error}")))?;
    temporary
        .persist(pointer_path(root))
        .map_err(|error| Error::new(format!("pointer を置き換えられません: {error}")))?;
    Ok(())
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).expect("string は常に JSON にできる")
}

/// root 単位の排他。install 中の別処理と重ならないようにする。
///
/// 獲得できなければ `None` を返す。launcher は起動を止めないため、待たずに今回の
/// 適用を見送る。
pub(crate) struct Lock {
    file: File,
}

impl Lock {
    pub(crate) fn try_acquire(root: &Path) -> Result<Option<Self>> {
        std::fs::create_dir_all(root)
            .map_err(|error| Error::new(format!("install root を作れません: {error}")))?;
        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join(LOCK_NAME))
            .map_err(|error| Error::new(format!("layout の lock を開けません: {error}")))?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Self { file })),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(error)) => Err(Error::new(format!(
                "layout の lock を取得できません: {error}"
            ))),
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// staging に置かれた適用候補（directory 名 = version）を新しい順に返す。
pub(crate) fn candidates(root: &Path) -> Result<Vec<String>> {
    let directory = staging_dir(root);
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(Error::new(format!(
                "staging を読めません ({}): {error}",
                directory.display()
            )));
        }
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| Error::new(format!("staging を読めません: {error}")))?;
        if !entry
            .file_type()
            .map_err(|error| Error::new(format!("staging を読めません: {error}")))?
            .is_dir()
        {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if manifest::validate_version(&name).is_err() {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        found.push((name, modified));
    }
    found.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| right.0.cmp(&left.0)));
    Ok(found.into_iter().map(|(name, _)| name).collect())
}

pub(crate) struct Applied {
    pub(crate) version: String,
}

/// 適用候補を検証して `versions/<version>/` へ展開し、pointer を進める。
///
/// 署名・SHA-256・展開内容のいずれかが一致しなければ pointer を更新しない。現在の
/// version と同じか古い候補、および `breaking: true` の候補は適用しない（軸B は同意が
/// 必要であり、同意画面はアプリ側の範囲）。
pub(crate) fn apply(
    root: &Path,
    candidate: &str,
    platform: &str,
    keys: &manifest::TrustedKeys,
) -> Result<Applied> {
    let _lock = Lock::try_acquire(root)?.ok_or_else(|| Error::new("別の処理が layout を使用中"))?;
    let current = read_pointer(root)?;
    // version の比較は hash より先に行う。適用しない候補のために毎回 package を
    // 読み直さない。
    if let Some(current) = &current
        && !is_newer(candidate, current)
    {
        return Err(Error::new(format!(
            "現在の version ({current}) より新しくない: {candidate}"
        )));
    }
    let source = staging_dir(root).join(candidate);
    if !source.is_dir() {
        return Err(Error::new(format!(
            "適用候補がありません: {}",
            source.display()
        )));
    }
    let verified = manifest::verify_distribution(&source, keys)?;
    if verified.manifest.version != candidate {
        return Err(Error::new(format!(
            "staging の directory 名 ({candidate}) と manifest の version ({}) が違います",
            verified.manifest.version
        )));
    }
    if !verified.packages.contains_key(platform) {
        return Err(Error::new(format!(
            "platform の package がありません: {platform}"
        )));
    }
    if verified.manifest.breaking {
        return Err(Error::new("breaking な更新は同意が必要"));
    }
    let target = version_dir(root, candidate)?;
    std::fs::create_dir_all(versions_dir(root))
        .map_err(|error| Error::new(format!("versions を作れません: {error}")))?;
    let staged = tempfile::Builder::new()
        .prefix(".extract-")
        .tempdir_in(versions_dir(root))
        .map_err(|error| Error::new(format!("一時 directory を作れません: {error}")))?;
    // 複写した staged を再検証し、以後の展開はこの検証済みコピーだけを使う。
    for asset in verified.manifest.assets.values() {
        std::fs::copy(
            source.join(&asset.source),
            staged.path().join(&asset.source),
        )
        .map_err(|error| Error::new(format!("package を複写できません: {error}")))?;
    }
    std::fs::copy(
        source.join(MANIFEST_NAME),
        staged.path().join(MANIFEST_NAME),
    )
    .map_err(|error| Error::new(format!("manifest を複写できません: {error}")))?;
    let staged_verified = manifest::verify_distribution(staged.path(), keys)?;
    let package = staged_verified.packages.get(platform).ok_or_else(|| {
        Error::new(format!(
            "複写した manifest に platform の package がありません: {platform}"
        ))
    })?;
    // 展開が manifest や package 自身を上書きしないよう、保持する file 名を渡す。
    let mut reserved: Vec<&str> = vec![MANIFEST_NAME];
    reserved.extend(
        staged_verified
            .manifest
            .assets
            .values()
            .map(|asset| asset.source.as_str()),
    );
    archive::extract(package, staged.path(), &reserved)?;
    let mismatched = archive::mismatches(staged.path(), package, &reserved)?;
    if !mismatched.is_empty() {
        return Err(Error::new(format!(
            "展開結果が package と一致しません: {}",
            mismatched.join("; ")
        )));
    }
    // 展開済み directory を version として確定してから pointer を進める。
    let staged_path = staged.keep();
    let backup = replace_directory(&staged_path, &target)?;
    if let Err(error) = write_pointer(root, candidate) {
        restore_directory(&target, backup.as_deref());
        return Err(error);
    }
    if let Some(backup) = backup {
        let _ = std::fs::remove_dir_all(backup);
    }
    prune(root, candidate, current.as_deref());
    // 適用済みの候補は「未適用」ではないため staging から片付ける。失敗しても起動には
    // 影響しない。
    let _ = std::fs::remove_dir_all(&source);
    Ok(Applied {
        version: candidate.to_owned(),
    })
}

fn replace_directory(source: &Path, target: &Path) -> Result<Option<PathBuf>> {
    if !target.exists() {
        std::fs::rename(source, target)
            .map_err(|error| Error::new(format!("version を配置できません: {error}")))?;
        return Ok(None);
    }
    let backup = tempfile::Builder::new()
        .prefix(".replace-")
        .tempdir_in(
            target
                .parent()
                .ok_or_else(|| Error::new("version の親がありません"))?,
        )
        .map_err(|error| Error::new(format!("退避先を作れません: {error}")))?
        .keep();
    std::fs::remove_dir(&backup)
        .map_err(|error| Error::new(format!("退避先を用意できません: {error}")))?;
    std::fs::rename(target, &backup)
        .map_err(|error| Error::new(format!("既存 version を退避できません: {error}")))?;
    if let Err(error) = std::fs::rename(source, target) {
        let _ = std::fs::rename(&backup, target);
        return Err(Error::new(format!("version を配置できません: {error}")));
    }
    Ok(Some(backup))
}

fn restore_directory(target: &Path, backup: Option<&Path>) {
    let Some(backup) = backup else {
        let _ = std::fs::remove_dir_all(target);
        return;
    };
    let _ = std::fs::remove_dir_all(target);
    let _ = std::fs::rename(backup, target);
}

/// 残す version を決めて directory を削除する。
///
/// 現行 + 前版の 2 世代を残す。削除するのは directory 名と manifest の `version` が
/// 一致する導入済み version だけであり、手動復旧で退避した `0.3.0.broken` のような
/// directory は残す。
fn prune(root: &Path, current: &str, previous: Option<&str>) {
    let mut keep = vec![current.to_owned()];
    match previous {
        Some(previous) if previous != current => keep.push(previous.to_owned()),
        _ => {
            // 同じ version を入れ直した場合は pointer から前版を知れないため、導入済みの
            // version を新しい順に補って 2 世代を割らない。
            for name in installed_versions_newest_first(root) {
                if keep.len() >= KEEP_VERSIONS {
                    break;
                }
                if name != current {
                    keep.push(name);
                }
            }
        }
    }
    let directory = versions_dir(root);
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if keep.contains(&name) || !path.is_dir() || name.starts_with('.') {
            continue;
        }
        if installed_version(&path).as_deref() != Some(name.as_str()) {
            continue;
        }
        let _ = std::fs::remove_dir_all(&path);
    }
}

fn installed_versions_newest_first(root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(versions_dir(root)) else {
        return Vec::new();
    };
    let mut found: Vec<(String, std::time::SystemTime)> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                return None;
            }
            if installed_version(&entry.path()).as_deref() != Some(name.as_str()) {
                return None;
            }
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            Some((name, modified))
        })
        .collect();
    found.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| right.0.cmp(&left.0)));
    found.into_iter().map(|(name, _)| name).collect()
}

fn installed_version(directory: &Path) -> Option<String> {
    let raw = std::fs::read(directory.join(MANIFEST_NAME)).ok()?;
    manifest::parse(&raw).ok().map(|manifest| manifest.version)
}

/// `candidate` が `current` より新しいか。比較できない形式では、同じでなければ適用する。
fn is_newer(candidate: &str, current: &str) -> bool {
    match (
        semver::Version::parse(candidate),
        semver::Version::parse(current),
    ) {
        (Ok(candidate), Ok(current)) => candidate > current,
        _ => candidate != current,
    }
}

/// 実行する app の file 名。package は Desktop と同梱 Host を含む。
pub(crate) fn app_executable(root: &Path, version: &str) -> Result<PathBuf> {
    Ok(version_dir(root, version)?.join(format!("lapis{}", std::env::consts::EXE_SUFFIX)))
}

/// `versions/<version>/` の manifest が pointer の version と一致するか。
///
/// 起動のたびに全 package を hash しない。展開内容の照合は `python -m scripts.release
/// verify --source <install root>` が行う。
pub(crate) fn installed_version_matches(root: &Path, version: &str) -> Result<bool> {
    let directory = version_dir(root, version)?;
    let raw = match std::fs::read(directory.join(MANIFEST_NAME)) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(Error::new(format!("manifest を読めません: {error}"))),
    };
    let manifest = manifest::parse(&raw)?;
    Ok(manifest.version == version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_round_trips_and_missing_pointer_is_none() {
        let root = tempfile::tempdir().expect("temp");
        assert_eq!(read_pointer(root.path()).expect("read"), None);
        std::fs::create_dir_all(version_dir(root.path(), "0.1.0").expect("dir")).expect("mkdir");
        write_pointer(root.path(), "0.1.0").expect("write");
        assert_eq!(
            read_pointer(root.path()).expect("read"),
            Some("0.1.0".to_owned())
        );
        let text = std::fs::read_to_string(pointer_path(root.path())).expect("pointer");
        assert_eq!(text, "{\"schema\":1,\"version\":\"0.1.0\"}\n");
    }

    #[test]
    fn corrupt_pointer_is_not_silently_replaced() {
        for payload in [
            "",
            "{}",
            "{\"schema\":2,\"version\":\"0.1.0\"}",
            "{\"schema\":1,\"version\":\"\"}",
            "{\"schema\":1,\"version\":\"0.1.0\",\"extra\":true}",
            "not json",
        ] {
            let root = tempfile::tempdir().expect("temp");
            std::fs::write(pointer_path(root.path()), payload).expect("write");
            assert!(read_pointer(root.path()).is_err(), "{payload}");
        }
    }

    #[test]
    fn pointer_is_not_written_for_a_missing_version() {
        let root = tempfile::tempdir().expect("temp");
        assert!(write_pointer(root.path(), "0.1.0").is_err());
        assert!(!pointer_path(root.path()).exists());
    }

    #[test]
    fn newer_versions_follow_semver_when_parseable() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("0.1.1", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
        assert!(!is_newer("0.1.0-alpha.1", "0.1.0"));
    }

    #[test]
    fn candidates_are_listed_newest_first_and_skip_unknown_names() {
        let root = tempfile::tempdir().expect("temp");
        for version in ["0.1.0", "0.2.0", "not-a-version"] {
            std::fs::create_dir_all(staging_dir(root.path()).join(version)).expect("mkdir");
        }
        let names = candidates(root.path()).expect("candidates");
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"0.1.0".to_owned()));
        assert!(names.contains(&"0.2.0".to_owned()));
    }

    #[test]
    fn lock_is_exclusive_within_the_process() {
        let root = tempfile::tempdir().expect("temp");
        let first = Lock::try_acquire(root.path())
            .expect("lock")
            .expect("first");
        assert!(Lock::try_acquire(root.path()).expect("lock").is_none());
        drop(first);
        assert!(Lock::try_acquire(root.path()).expect("lock").is_some());
    }

    const PLATFORM: &str = "windows-x86_64";

    fn stage(root: &Path, fixture: &str, version: &str) {
        let path = staging_dir(root).join(version);
        std::fs::create_dir_all(&path).expect("staging");
        crate::manifest::fixtures::copy(fixture, &path);
    }

    #[test]
    fn staged_distributions_are_installed_and_pointer_moves() {
        let root = tempfile::tempdir().expect("temp");
        let keys = crate::manifest::fixtures::keys();
        stage(root.path(), "distribution", "0.1.0-alpha.1");
        let applied = apply(root.path(), "0.1.0-alpha.1", PLATFORM, &keys).expect("apply");
        assert_eq!(applied.version, "0.1.0-alpha.1");
        assert_eq!(
            read_pointer(root.path()).expect("pointer"),
            Some("0.1.0-alpha.1".to_owned())
        );
        let installed = version_dir(root.path(), "0.1.0-alpha.1").expect("dir");
        // fixture の `lapis.exe` は text file。改行の有無は環境に依存する。
        let exe = std::fs::read_to_string(installed.join("lapis.exe")).expect("exe");
        assert!(exe.starts_with("fixture"), "{exe:?}");
        assert!(installed.join(MANIFEST_NAME).is_file());
        assert!(
            !staging_dir(root.path()).join("0.1.0-alpha.1").exists(),
            "適用済みの staging は残さない"
        );
        // 展開後も追加 file が無く、mismatches が空である。
        let package = installed.join("lapis-0.1.0-alpha.1-windows-x86_64.zip");
        let mismatched = archive::mismatches(
            &installed,
            &package,
            &[
                MANIFEST_NAME,
                package.file_name().expect("name").to_str().expect("utf8"),
            ],
        )
        .expect("compare");
        assert!(mismatched.is_empty(), "{mismatched:?}");
    }

    #[test]
    fn applying_a_second_version_keeps_the_previous_one() {
        let root = tempfile::tempdir().expect("temp");
        let keys = crate::manifest::fixtures::keys();
        stage(root.path(), "distribution", "0.1.0-alpha.1");
        apply(root.path(), "0.1.0-alpha.1", PLATFORM, &keys).expect("first");
        stage(root.path(), "distribution2", "0.1.0-alpha.2");
        apply(root.path(), "0.1.0-alpha.2", PLATFORM, &keys).expect("second");
        assert_eq!(
            read_pointer(root.path()).expect("pointer"),
            Some("0.1.0-alpha.2".to_owned())
        );
        assert!(
            version_dir(root.path(), "0.1.0-alpha.1")
                .expect("dir")
                .is_dir()
        );
        assert!(
            version_dir(root.path(), "0.1.0-alpha.2")
                .expect("dir")
                .is_dir()
        );
        let exe = version_dir(root.path(), "0.1.0-alpha.2")
            .expect("dir")
            .join("lapis.exe");
        let exe = std::fs::read_to_string(exe).expect("exe");
        assert!(exe.starts_with("fixture-v2"), "{exe:?}");
    }

    #[test]
    fn a_third_version_prunes_to_two_generations_and_keeps_quarantined_directories() {
        let root = tempfile::tempdir().expect("temp");
        let keys = crate::manifest::fixtures::keys();
        stage(root.path(), "distribution", "0.1.0-alpha.1");
        apply(root.path(), "0.1.0-alpha.1", PLATFORM, &keys).expect("first");
        stage(root.path(), "distribution2", "0.1.0-alpha.2");
        apply(root.path(), "0.1.0-alpha.2", PLATFORM, &keys).expect("second");
        // 手動復旧で退避した directory は削除しない。
        let quarantined = versions_dir(root.path()).join("0.1.0-alpha.2.broken");
        std::fs::create_dir(&quarantined).expect("quarantine");
        stage(root.path(), "distribution3", "0.2.0");
        apply(root.path(), "0.2.0", PLATFORM, &keys).expect("third");
        assert!(
            !version_dir(root.path(), "0.1.0-alpha.1")
                .expect("dir")
                .exists()
        );
        assert!(
            version_dir(root.path(), "0.1.0-alpha.2")
                .expect("dir")
                .is_dir()
        );
        assert!(version_dir(root.path(), "0.2.0").expect("dir").is_dir());
        assert!(quarantined.is_dir());
    }

    #[test]
    fn a_tampered_staged_distribution_does_not_move_the_pointer() {
        let root = tempfile::tempdir().expect("temp");
        let keys = crate::manifest::fixtures::keys();
        stage(root.path(), "distribution", "0.1.0-alpha.1");
        apply(root.path(), "0.1.0-alpha.1", PLATFORM, &keys).expect("first");
        stage(root.path(), "distribution2", "0.1.0-alpha.2");
        let package = staging_dir(root.path())
            .join("0.1.0-alpha.2")
            .join("lapis-0.1.0-alpha.2-windows-x86_64.zip");
        let mut bytes = std::fs::read(&package).expect("package");
        bytes[0] ^= 0xff;
        std::fs::write(&package, bytes).expect("tamper");
        assert!(apply(root.path(), "0.1.0-alpha.2", PLATFORM, &keys).is_err());
        assert_eq!(
            read_pointer(root.path()).expect("pointer"),
            Some("0.1.0-alpha.1".to_owned())
        );
        assert!(
            !version_dir(root.path(), "0.1.0-alpha.2")
                .expect("dir")
                .exists()
        );
    }

    #[test]
    fn breaking_distributions_are_not_applied_without_consent() {
        let root = tempfile::tempdir().expect("temp");
        let keys = crate::manifest::fixtures::keys();
        stage(root.path(), "distribution", "0.1.0-alpha.1");
        apply(root.path(), "0.1.0-alpha.1", PLATFORM, &keys).expect("first");
        stage(root.path(), "distribution-breaking", "0.3.0");
        assert!(apply(root.path(), "0.3.0", PLATFORM, &keys).is_err());
        assert_eq!(
            read_pointer(root.path()).expect("pointer"),
            Some("0.1.0-alpha.1".to_owned())
        );
    }

    #[test]
    fn older_candidates_do_not_move_the_pointer() {
        let root = tempfile::tempdir().expect("temp");
        let keys = crate::manifest::fixtures::keys();
        stage(root.path(), "distribution3", "0.2.0");
        apply(root.path(), "0.2.0", PLATFORM, &keys).expect("first");
        stage(root.path(), "distribution", "0.1.0-alpha.1");
        assert!(apply(root.path(), "0.1.0-alpha.1", PLATFORM, &keys).is_err());
        assert_eq!(
            read_pointer(root.path()).expect("pointer"),
            Some("0.2.0".to_owned())
        );
    }

    #[test]
    fn a_broken_newest_candidate_falls_back_to_the_next_one() {
        let root = tempfile::tempdir().expect("temp");
        let keys = crate::manifest::fixtures::keys();
        stage(root.path(), "distribution2", "0.1.0-alpha.2");
        stage(root.path(), "distribution", "0.1.0-alpha.1");
        // 0.1.0-alpha.2 が壊れている場合、launcher は 0.1.0-alpha.1 を試す。
        let package = staging_dir(root.path())
            .join("0.1.0-alpha.2")
            .join("lapis-0.1.0-alpha.2-windows-x86_64.zip");
        let mut bytes = std::fs::read(&package).expect("package");
        bytes[0] ^= 0xff;
        std::fs::write(&package, bytes).expect("tamper");
        let mut applied = None;
        for version in candidates(root.path()).expect("candidates") {
            if let Ok(result) = apply(root.path(), &version, PLATFORM, &keys) {
                applied = Some(result.version);
            }
        }
        assert_eq!(applied, Some("0.1.0-alpha.1".to_owned()));
    }

    #[test]
    fn installed_version_must_match_the_pointer() {
        let root = tempfile::tempdir().expect("temp");
        let directory = version_dir(root.path(), "0.1.0-alpha.1").expect("dir");
        std::fs::create_dir_all(&directory).expect("mkdir");
        assert!(!installed_version_matches(root.path(), "0.1.0-alpha.1").expect("check"));
        crate::manifest::fixtures::copy("distribution", &directory);
        assert!(installed_version_matches(root.path(), "0.1.0-alpha.1").expect("check"));
        assert!(!installed_version_matches(root.path(), "0.2.0").expect("check"));
    }
}
