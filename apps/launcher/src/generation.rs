//! data 世代の並置と切り替え。
//!
//! `data/<generation>/` を世代番号で分け、どの version がどの世代を使ったかを
//! `data/versions.json` に残す。軸A は同じ世代を使い続け、軸B でだけ新しい世代を
//! 並置して切り替える。旧世代の directory は削除しない（`docs/specs/release.md`）。
//!
//! credentials は世代の外（`credentials/`）に置くため、世代交代で再 pairing は起きない。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::{Error, Result};

const DATA_DIR: &str = "data";
const CREDENTIALS_DIR: &str = "credentials";
const GENERATION_POINTER: &str = "current.json";
const VERSION_MAP: &str = "versions.json";
/// 同意済みの breaking 更新。launcher はここにある version だけを breaking 適用する。
const CONSENT_MAP: &str = "consented.json";
/// 世代記録の read-modify-write を root 単位で排他する。
const LOCK_NAME: &str = "records.lock";
const SCHEMA: i64 = 1;

/// `<install root>/data/`。世代 directory と世代の pointer を置く。
pub fn data_dir(root: &Path) -> PathBuf {
    root.join(DATA_DIR)
}

/// `<install root>/credentials/`。data 世代の外に置く credential の置き場。
pub fn credentials_dir(root: &Path) -> PathBuf {
    root.join(CREDENTIALS_DIR)
}

/// `<install root>/data/<generation>/`。DB / data の置き場。
pub fn generation_dir(root: &Path, generation: u32) -> PathBuf {
    data_dir(root).join(generation.to_string())
}

/// 現在の世代。pointer が無ければ 1（初回起動と同じ扱い）。
pub fn read_generation(root: &Path) -> Result<u32> {
    let path = data_dir(root).join(GENERATION_POINTER);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(1),
        Err(error) => return Err(Error::new(format!("data の pointer を読めません: {error}"))),
    };
    let value: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|error| Error::new(format!("data の pointer が JSON でない: {error}")))?;
    let object = value
        .as_object()
        .filter(|object| {
            object.len() == 2 && object.contains_key("schema") && object.contains_key("generation")
        })
        .ok_or_else(|| Error::new("data の pointer の field が不正"))?;
    if object["schema"].as_i64() != Some(SCHEMA) {
        return Err(Error::new("data の pointer の schema が不正"));
    }
    let generation = object["generation"]
        .as_u64()
        .filter(|value| *value > 0 && *value <= u64::from(u32::MAX))
        .ok_or_else(|| Error::new("data の pointer の generation が不正"))?;
    Ok(generation as u32)
}

/// 世代の pointer を原子的に置き換える。directory が無ければ失敗する。
pub fn write_generation(root: &Path, generation: u32) -> Result<()> {
    if !generation_dir(root, generation).is_dir() {
        return Err(Error::new(format!(
            "data 世代の directory がありません: {}",
            generation_dir(root, generation).display()
        )));
    }
    let directory = data_dir(root);
    std::fs::create_dir_all(&directory)
        .map_err(|error| Error::new(format!("data directory を作れません: {error}")))?;
    let payload = format!("{{\"schema\":{SCHEMA},\"generation\":{generation}}}\n");
    write_atomic(&directory, GENERATION_POINTER, payload.as_bytes())
}

/// 新しい世代を用意する。現在世代の pointer は進めない。
///
/// 返すのは新しい世代番号。既にある世代の最大 + 1 を使い、directory を作るだけにする。
/// pointer の切り替えは breaking 版が実際に起動したとき（[`activate_generation`]）に行い、
/// 準備しただけで適用されなかった候補が利用者の data を別世代へ移さないようにする。
pub fn prepare_generation(root: &Path) -> Result<u32> {
    let current = read_generation(root)?;
    let next = highest_generation(root)?
        .max(current)
        .saturating_add(1)
        .max(2);
    let directory = generation_dir(root, next);
    std::fs::create_dir_all(&directory).map_err(|error| {
        Error::new(format!(
            "data 世代を作れません ({}): {error}",
            directory.display()
        ))
    })?;
    Ok(next)
}

fn highest_generation(root: &Path) -> Result<u32> {
    let directory = data_dir(root);
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(1),
        Err(error) => return Err(Error::new(format!("data を読めません: {error}"))),
    };
    let mut highest = 1;
    for entry in entries {
        let entry = entry.map_err(|error| Error::new(format!("data を読めません: {error}")))?;
        if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            continue;
        }
        if let Some(generation) = entry
            .file_name()
            .to_string_lossy()
            .parse::<u32>()
            .ok()
            .filter(|value| *value > 0)
        {
            highest = highest.max(generation);
        }
    }
    Ok(highest)
}

/// `version` が使った世代。記録が無ければ `None`。
pub fn generation_for(root: &Path, version: &str) -> Result<Option<u32>> {
    Ok(read_version_map(root)?.get(version).copied())
}

/// `version` と世代の対応を記録する。旧 version を旧世代で起動できるようにする。
///
/// 読取から保存までを root 単位の排他で守る。複数プロセスの同時更新で、先の追加を
/// 後の保存が失わせない。`layout` の lock とは別の、世代記録専用の lock を使う
/// （layout の lock は `install` が保持したまま `record` を呼ぶ経路があるため）。
pub fn record_generation(root: &Path, version: &str, generation: u32) -> Result<()> {
    let _lock = record_lock(root)?;
    update_version_map(root, version, generation)
}

fn update_version_map(root: &Path, version: &str, generation: u32) -> Result<()> {
    let mut versions = read_version_map(root)?;
    if versions.get(version) == Some(&generation) {
        return Ok(());
    }
    versions.insert(version.to_owned(), generation);
    write_version_map(root, &versions)
}

/// 世代記録専用の排他。読取から保存までを 1 区間にし、lost update を防ぐ。
///
/// 区間は小さな JSON の読み書きだけなので、待って取得する。取得できなければ記録を
/// 落とすより、待って直列化する方が安全である。
fn record_lock(root: &Path) -> Result<RecordLock> {
    let directory = data_dir(root);
    std::fs::create_dir_all(&directory)
        .map_err(|error| Error::new(format!("data directory を作れません: {error}")))?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join(LOCK_NAME))
        .map_err(|error| Error::new(format!("世代記録の lock を開けません: {error}")))?;
    file.lock()
        .map_err(|error| Error::new(format!("世代記録の lock を取得できません: {error}")))?;
    Ok(RecordLock(file))
}

struct RecordLock(std::fs::File);

impl Drop for RecordLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

/// 同意済みの breaking 更新を記録する。launcher はこの記録がある version だけを適用する。
///
/// 世代は準備済みのものを対応付ける。適用は次回起動の launcher が行い、そのとき
/// [`activate_generation`] で現在世代を進める。
pub fn record_consent(root: &Path, version: &str, generation: u32) -> Result<()> {
    let _lock = record_lock(root)?;
    update_version_map(root, version, generation)?;
    let mut consents = read_consents(root)?;
    consents.insert(version.to_owned(), generation);
    write_version_table(root, CONSENT_MAP, "consented", &consents)
}

/// 同意済みの breaking 更新か。`Some(generation)` なら同意済み。
pub fn consented_generation(root: &Path, version: &str) -> Result<Option<u32>> {
    Ok(read_consents(root)?.get(version).copied())
}

/// 同意済み version が実際に起動したので、現在世代を進めて同意記録を消す。
///
/// ここで初めて pointer を切り替える。準備しただけの世代は、breaking 版が起動するまで
/// 現在世代にならない。
pub fn activate_generation(root: &Path, version: &str) -> Result<Option<u32>> {
    let _lock = record_lock(root)?;
    let Some(generation) = consented_generation(root, version)? else {
        return Ok(None);
    };
    write_generation(root, generation)?;
    update_version_map(root, version, generation)?;
    let mut consents = read_consents(root)?;
    consents.remove(version);
    write_version_table(root, CONSENT_MAP, "consented", &consents)?;
    Ok(Some(generation))
}

fn read_consents(root: &Path) -> Result<BTreeMap<String, u32>> {
    read_version_table(root, CONSENT_MAP, "consented", "同意の記録")
}

fn write_version_map(root: &Path, versions: &BTreeMap<String, u32>) -> Result<()> {
    write_version_table(root, VERSION_MAP, "versions", versions)
}

fn write_version_table(
    root: &Path,
    name: &str,
    field: &str,
    values: &BTreeMap<String, u32>,
) -> Result<()> {
    let directory = data_dir(root);
    std::fs::create_dir_all(&directory)
        .map_err(|error| Error::new(format!("data directory を作れません: {error}")))?;
    let sorted = values
        .iter()
        .map(|(version, generation)| (version.clone(), Value::from(*generation)))
        .collect::<serde_json::Map<_, _>>();
    let payload = serde_json::json!({ "schema": SCHEMA, field: sorted });
    let mut text = serde_json::to_string_pretty(&payload)?;
    text.push('\n');
    write_atomic(&directory, name, text.as_bytes())
}

fn read_version_map(root: &Path) -> Result<BTreeMap<String, u32>> {
    read_version_table(root, VERSION_MAP, "versions", "世代の記録")
}

fn read_version_table(
    root: &Path,
    name: &str,
    field: &str,
    label: &str,
) -> Result<BTreeMap<String, u32>> {
    let path = data_dir(root).join(name);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(Error::new(format!("{label}を読めません: {error}"))),
    };
    let value: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|error| Error::new(format!("{label}が JSON でない: {error}")))?;
    let object = value
        .as_object()
        .filter(|object| {
            object.len() == 2 && object.contains_key("schema") && object.contains_key(field)
        })
        .ok_or_else(|| Error::new(format!("{label}の field が不正")))?;
    if object["schema"].as_i64() != Some(SCHEMA) {
        return Err(Error::new(format!("{label}の schema が不正")));
    }
    let mut versions = BTreeMap::new();
    for (version, generation) in object[field]
        .as_object()
        .ok_or_else(|| Error::new(format!("{label}の {field} が object でない")))?
    {
        let generation = generation
            .as_u64()
            .filter(|value| *value > 0 && *value <= u64::from(u32::MAX))
            .ok_or_else(|| Error::new(format!("{version} の世代が不正")))?;
        versions.insert(version.clone(), generation as u32);
    }
    Ok(versions)
}

fn write_atomic(directory: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let mut temporary = tempfile::Builder::new()
        .prefix(".data-")
        .suffix(".tmp")
        .tempfile_in(directory)
        .map_err(|error| Error::new(format!("{name} を書けません: {error}")))?;
    use std::io::Write;
    temporary
        .write_all(bytes)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|error| Error::new(format!("{name} を書けません: {error}")))?;
    temporary
        .persist(directory.join(name))
        .map_err(|error| Error::new(format!("{name} を置き換えられません: {error}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_pointer_starts_at_the_first_generation() {
        let root = tempfile::tempdir().expect("temp");
        assert_eq!(read_generation(root.path()).expect("read"), 1);
        assert!(!data_dir(root.path()).exists());
    }

    #[test]
    fn preparing_a_generation_places_it_side_by_side_without_moving_the_pointer() {
        let root = tempfile::tempdir().expect("temp");
        let first = generation_dir(root.path(), 1);
        std::fs::create_dir_all(&first).expect("mkdir");
        std::fs::write(first.join("db"), "old").expect("data");
        let second = prepare_generation(root.path()).expect("prepare");
        assert_eq!(second, 2);
        // 準備しただけでは現在世代を進めない。適用されなかった候補が利用者の data を
        // 別世代へ移さないため。
        assert_eq!(read_generation(root.path()).expect("read"), 1);
        assert!(generation_dir(root.path(), 2).is_dir());
        assert_eq!(
            std::fs::read_to_string(first.join("db")).expect("old data"),
            "old",
            "旧世代は削除しない"
        );
    }

    #[test]
    fn activating_a_generation_advances_the_pointer_only_for_the_consented_version() {
        let root = tempfile::tempdir().expect("temp");
        std::fs::create_dir_all(generation_dir(root.path(), 1)).expect("mkdir");
        std::fs::create_dir_all(generation_dir(root.path(), 2)).expect("mkdir");
        write_generation(root.path(), 1).expect("pointer");
        record_consent(root.path(), "0.3.0", 2).expect("consent");
        // 同意しただけでは pointer を進めない。
        assert_eq!(read_generation(root.path()).expect("read"), 1);
        assert_eq!(
            consented_generation(root.path(), "0.3.0").expect("consent"),
            Some(2)
        );

        assert_eq!(
            activate_generation(root.path(), "0.3.0").expect("activate"),
            Some(2)
        );
        assert_eq!(read_generation(root.path()).expect("read"), 2);
        assert_eq!(generation_for(root.path(), "0.3.0").expect("read"), Some(2));
        // 切り替え後は同意記録を残さない。
        assert_eq!(
            consented_generation(root.path(), "0.3.0").expect("consent"),
            None
        );
    }

    #[test]
    fn a_version_without_consent_does_not_advance_the_generation() {
        let root = tempfile::tempdir().expect("temp");
        std::fs::create_dir_all(generation_dir(root.path(), 1)).expect("mkdir");
        write_generation(root.path(), 1).expect("pointer");
        assert_eq!(
            activate_generation(root.path(), "0.9.0").expect("activate"),
            None
        );
        assert_eq!(read_generation(root.path()).expect("read"), 1);
    }

    /// 記録の排他は root ごとに独立し、保持中は別の更新が同じ root で待つ。
    #[test]
    fn generation_records_are_guarded_by_a_root_lock() {
        let first = tempfile::tempdir().expect("temp");
        let second = tempfile::tempdir().expect("temp");
        let held = record_lock(first.path()).expect("lock");

        // 別の root は独立して取得できる。
        assert!(record_lock(second.path()).is_ok());

        // 同じ root の 2 つ目は、解放されるまで待つ。
        let root_path = first.path().to_owned();
        let started = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let observed = started.clone();
        let waiter = std::thread::spawn(move || {
            let _lock = record_lock(&root_path).expect("lock after release");
            observed.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(
            !started.load(std::sync::atomic::Ordering::SeqCst),
            "保持中は同じ root の記録を進めない"
        );
        drop(held);
        waiter.join().expect("join");
        assert!(started.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn concurrent_record_keeps_every_version() {
        let root = tempfile::tempdir().expect("temp");
        let root_path = root.path().to_owned();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let results = std::thread::scope(|scope| {
            let handles = ["0.1.0", "0.3.0"].map(|version| {
                let root = root_path.clone();
                let barrier = barrier.clone();
                scope.spawn(move || {
                    barrier.wait();
                    record_generation(&root, version, 1)
                })
            });
            handles.map(|handle| handle.join().expect("join"))
        });
        // 排他により、どちらも失われない（片方が待つ場合も含めて全件残る）。
        assert!(results.iter().all(|result| result.is_ok()), "{results:?}");
        assert_eq!(generation_for(&root_path, "0.1.0").expect("read"), Some(1));
        assert_eq!(generation_for(&root_path, "0.3.0").expect("read"), Some(1));
    }

    #[test]
    fn preparing_uses_the_highest_existing_generation() {
        let root = tempfile::tempdir().expect("temp");
        std::fs::create_dir_all(generation_dir(root.path(), 5)).expect("mkdir");
        assert_eq!(prepare_generation(root.path()).expect("prepare"), 6);
    }

    #[test]
    fn a_version_keeps_the_generation_it_recorded() {
        let root = tempfile::tempdir().expect("temp");
        std::fs::create_dir_all(generation_dir(root.path(), 1)).expect("mkdir");
        write_generation(root.path(), 1).expect("pointer");
        record_generation(root.path(), "0.1.0", 1).expect("record");
        prepare_generation(root.path()).expect("prepare");
        record_generation(root.path(), "0.3.0", 2).expect("record");
        assert_eq!(generation_for(root.path(), "0.1.0").expect("read"), Some(1));
        assert_eq!(generation_for(root.path(), "0.3.0").expect("read"), Some(2));
        assert_eq!(generation_for(root.path(), "0.9.0").expect("read"), None);
        // 記録は現在世代の pointer とは独立に残る。
        assert_eq!(read_generation(root.path()).expect("read"), 1);
    }

    #[test]
    fn corrupt_generation_pointers_are_rejected() {
        let root = tempfile::tempdir().expect("temp");
        std::fs::create_dir_all(data_dir(root.path())).expect("mkdir");
        for payload in [
            "",
            "{}",
            "{\"schema\":2,\"generation\":1}",
            "{\"schema\":1,\"generation\":0}",
            "{\"schema\":1,\"generation\":1,\"version\":\"x\"}",
            "not json",
        ] {
            std::fs::write(data_dir(root.path()).join(GENERATION_POINTER), payload).expect("write");
            assert!(read_generation(root.path()).is_err(), "{payload}");
        }
    }

    #[test]
    fn credentials_live_outside_the_data_generation() {
        let root = tempfile::tempdir().expect("temp");
        assert_eq!(
            credentials_dir(root.path()),
            root.path().join("credentials")
        );
        assert!(
            !credentials_dir(root.path()).starts_with(data_dir(root.path())),
            "credential を世代の内側に置かない"
        );
    }

    #[test]
    fn a_generation_without_a_directory_is_not_pointed_at() {
        let root = tempfile::tempdir().expect("temp");
        assert!(write_generation(root.path(), 4).is_err());
    }
}
