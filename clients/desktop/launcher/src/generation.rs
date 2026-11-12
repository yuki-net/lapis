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

/// 次の世代を並置して切り替える。旧世代の directory は残す。
///
/// 返すのは新しい世代番号。既にある世代の最大 + 1 を使い、directory を先に作ってから
/// pointer を進める。pointer の書き込みに失敗した場合は、空のまま残った世代を消さない
/// （判断できない directory を消さない方針に合わせる）。
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
    write_generation(root, next)?;
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
pub fn record_generation(root: &Path, version: &str, generation: u32) -> Result<()> {
    let mut versions = read_version_map(root)?;
    if versions.get(version) == Some(&generation) {
        return Ok(());
    }
    versions.insert(version.to_owned(), generation);
    let directory = data_dir(root);
    std::fs::create_dir_all(&directory)
        .map_err(|error| Error::new(format!("data directory を作れません: {error}")))?;
    let sorted = versions
        .iter()
        .map(|(version, generation)| (version.clone(), Value::from(*generation)))
        .collect::<serde_json::Map<_, _>>();
    let payload = serde_json::json!({ "schema": SCHEMA, "versions": sorted });
    let mut text = serde_json::to_string_pretty(&payload)?;
    text.push('\n');
    write_atomic(&directory, VERSION_MAP, text.as_bytes())
}

fn read_version_map(root: &Path) -> Result<BTreeMap<String, u32>> {
    let path = data_dir(root).join(VERSION_MAP);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(Error::new(format!("世代の記録を読めません: {error}"))),
    };
    let value: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|error| Error::new(format!("世代の記録が JSON でない: {error}")))?;
    let object = value
        .as_object()
        .filter(|object| {
            object.len() == 2 && object.contains_key("schema") && object.contains_key("versions")
        })
        .ok_or_else(|| Error::new("世代の記録の field が不正"))?;
    if object["schema"].as_i64() != Some(SCHEMA) {
        return Err(Error::new("世代の記録の schema が不正"));
    }
    let mut versions = BTreeMap::new();
    for (version, generation) in object["versions"]
        .as_object()
        .ok_or_else(|| Error::new("世代の記録の versions が object でない"))?
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
    fn preparing_a_generation_places_it_side_by_side_and_keeps_old_data() {
        let root = tempfile::tempdir().expect("temp");
        let first = generation_dir(root.path(), 1);
        std::fs::create_dir_all(&first).expect("mkdir");
        std::fs::write(first.join("db"), "old").expect("data");
        let second = prepare_generation(root.path()).expect("prepare");
        assert_eq!(second, 2);
        assert_eq!(read_generation(root.path()).expect("read"), 2);
        assert!(generation_dir(root.path(), 2).is_dir());
        assert_eq!(
            std::fs::read_to_string(first.join("db")).expect("old data"),
            "old",
            "旧世代は削除しない"
        );
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
        // 新しい世代へ移っても旧 version の記録は残る。
        assert_eq!(read_generation(root.path()).expect("read"), 2);
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
