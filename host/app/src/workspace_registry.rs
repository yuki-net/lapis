//! Host上で登録済みWorkspaceの識別と復元に使う。ClientへOS pathを公開しない。

use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use client_api::WorkspaceId;
use serde::{Deserialize, Serialize};

const MAX_RECORD_BYTES: u64 = 512 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u8,
    workspace_id: String,
    root: NativePath,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "platform", content = "units", rename_all = "snake_case")]
enum NativePath {
    Windows(Vec<u16>),
    Unix(Vec<u8>),
}

#[cfg(windows)]
fn encode(path: &Path) -> NativePath {
    use std::os::windows::ffi::OsStrExt;
    NativePath::Windows(path.as_os_str().encode_wide().collect())
}

#[cfg(unix)]
fn encode(path: &Path) -> NativePath {
    use std::os::unix::ffi::OsStrExt;
    NativePath::Unix(path.as_os_str().as_bytes().to_vec())
}

#[cfg(windows)]
fn decode(path: NativePath) -> io::Result<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    match path {
        NativePath::Windows(units) => Ok(PathBuf::from(std::ffi::OsString::from_wide(&units))),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "workspace path platform mismatch",
        )),
    }
}

#[cfg(unix)]
fn decode(path: NativePath) -> io::Result<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    match path {
        NativePath::Unix(units) => Ok(PathBuf::from(std::ffi::OsString::from_vec(units))),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "workspace path platform mismatch",
        )),
    }
}

fn read(path: &Path) -> io::Result<(WorkspaceId, PathBuf)> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(MAX_RECORD_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "workspace record too large",
        ));
    }
    let record: Record = serde_json::from_slice(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if record.version != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported workspace record",
        ));
    }
    let id = WorkspaceId::try_new(record.workspace_id).map_err(io::Error::other)?;
    let root = decode(record.root)?;
    if !root.is_absolute()
        || root
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid workspace root",
        ));
    }
    let expected_name = format!("{}.json", id.as_str());
    if path.file_name().and_then(|name| name.to_str()) != Some(expected_name.as_str()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "workspace record identity mismatch",
        ));
    }
    Ok((id, root))
}

pub(crate) fn load(directory: &Path) -> io::Result<Vec<(WorkspaceId, PathBuf)>> {
    let directory = directory.join("workspaces");
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut result = Vec::new();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with("workspace-") || !name.ends_with(".json") {
            continue;
        }
        if !entry.file_type()?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "workspace record is not a regular file",
            ));
        }
        result.push(read(&entry.path())?);
    }
    result.sort_by(|left, right| left.0.as_str().cmp(right.0.as_str()));
    Ok(result)
}

pub(crate) fn save(directory: &Path, id: &WorkspaceId, root: &Path) -> io::Result<()> {
    let directory = directory.join("workspaces");
    fs::create_dir_all(&directory)?;
    let path = directory.join(format!("{}.json", id.as_str()));
    match fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if !metadata.file_type().is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "workspace record is not a regular file",
                ));
            }
            return matches_existing(&path, id, root);
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let record = Record {
        version: 1,
        workspace_id: id.as_str().to_owned(),
        root: encode(root),
    };
    let bytes = serde_json::to_vec(&record).map_err(io::Error::other)?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "workspace record too large",
        ));
    }
    let mut temporary = tempfile::NamedTempFile::new_in(&directory)?;
    temporary.write_all(&bytes)?;
    temporary.as_file().sync_all()?;
    match temporary.persist_noclobber(&path) {
        Ok(_) => Ok(()),
        Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {
            matches_existing(&path, id, root)
        }
        Err(error) => Err(error.error),
    }
}

fn matches_existing(path: &Path, id: &WorkspaceId, root: &Path) -> io::Result<()> {
    if !fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "workspace record is not a regular file",
        ));
    }
    let (existing_id, existing_root) = read(path)?;
    if &existing_id == id && existing_root == root {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "workspace record differs",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registrations_round_trip_and_do_not_replace_corrupt_records() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("workspace-画面");
        let id = WorkspaceId::try_new("workspace-test").unwrap();
        save(directory.path(), &id, &root).unwrap();
        save(directory.path(), &id, &root).unwrap();
        assert_eq!(load(directory.path()).unwrap(), vec![(id.clone(), root)]);
        let path = directory
            .path()
            .join("workspaces")
            .join("workspace-test.json");
        fs::write(&path, b"corrupt").unwrap();
        assert!(save(directory.path(), &id, directory.path()).is_err());
        assert_eq!(fs::read(path).unwrap(), b"corrupt");
        assert!(load(directory.path()).is_err());
    }

    #[test]
    fn concurrent_identical_registration_keeps_one_complete_record() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("workspace");
        let id = WorkspaceId::try_new("workspace-test").unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(5));
        let mut threads = Vec::new();
        for _ in 0..4 {
            let directory = directory.path().to_owned();
            let root = root.clone();
            let id = id.clone();
            let barrier = barrier.clone();
            threads.push(std::thread::spawn(move || {
                barrier.wait();
                save(&directory, &id, &root)
            }));
        }
        barrier.wait();
        for thread in threads {
            thread.join().unwrap().unwrap();
        }
        assert_eq!(load(directory.path()).unwrap(), vec![(id, root)]);
    }

    #[cfg(windows)]
    #[test]
    fn preserves_windows_non_unicode_path_units() {
        use std::os::windows::ffi::OsStringExt;
        let directory = tempfile::tempdir().unwrap();
        let root = directory
            .path()
            .join(std::ffi::OsString::from_wide(&[0xd800]));
        let id = WorkspaceId::try_new("workspace-unicode").unwrap();
        save(directory.path(), &id, &root).unwrap();
        assert_eq!(load(directory.path()).unwrap(), vec![(id, root)]);
    }

    #[cfg(unix)]
    #[test]
    fn preserves_unix_non_utf8_path_bytes() {
        use std::os::unix::ffi::OsStringExt;
        let directory = tempfile::tempdir().unwrap();
        let root = directory
            .path()
            .join(std::ffi::OsString::from_vec(vec![0xff]));
        let id = WorkspaceId::try_new("workspace-bytes").unwrap();
        save(directory.path(), &id, &root).unwrap();
        assert_eq!(load(directory.path()).unwrap(), vec![(id, root)]);
    }
}
