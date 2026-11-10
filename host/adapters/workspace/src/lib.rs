//! Local Host のファイルシステムを使う Workspace / Document backend。

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, mpsc},
    time::UNIX_EPOCH,
};

use backend_state::{WorkspaceEntry, WorkspaceEntryKind, WorkspaceFileBackend};
use document::{DocumentError, DocumentRepository, FileData, FileFingerprint};
use notify::Watcher;
use workspace::{FileEntry, FileEntryKind, WorkspaceError, WorkspaceRepository};

/// 監視 root を持つ Local 実装。`Default` は監視なしで使う。
pub struct LocalWorkspaceRepository {
    root: Option<PathBuf>,
    watch: Mutex<Option<WorkspaceWatch>>,
}

/// Workspace のツリーとイベント通知で無視するディレクトリ／ファイル名。
const IGNORED_NAMES: &[&str] = &[".git", "target", ".DS_Store"];

fn is_ignored_path(path: &Path) -> bool {
    path.components().any(|part| {
        IGNORED_NAMES
            .iter()
            .any(|name| part.as_os_str() == std::ffi::OsStr::new(name))
    })
}

struct WorkspaceWatch {
    _watcher: notify::RecommendedWatcher,
}

impl Default for LocalWorkspaceRepository {
    fn default() -> Self {
        Self {
            root: None,
            watch: Mutex::new(None),
        }
    }
}

impl LocalWorkspaceRepository {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root: Some(root),
            watch: Mutex::new(None),
        }
    }
}

impl DocumentRepository for LocalWorkspaceRepository {
    fn read_file(&self, path: &Path) -> Result<FileData, DocumentError> {
        let bytes = fs::read(path).map_err(|error| DocumentError::io(error.to_string()))?;
        let fingerprint = fingerprint_for(path, &bytes)?;
        Ok(FileData { bytes, fingerprint })
    }

    fn write_file(
        &self,
        path: &Path,
        content: &[u8],
        expected: Option<&FileFingerprint>,
    ) -> Result<FileFingerprint, DocumentError> {
        let actual = self.fingerprint(path)?;
        match (expected, actual.as_ref()) {
            (Some(expected), Some(actual)) if expected != actual => {
                return Err(DocumentError::conflict(
                    "保存前に外部変更を検出しました。再読み込みまたは差分確認が必要です",
                ));
            }
            (Some(_), None) => {
                return Err(DocumentError::conflict(
                    "保存前にファイルが削除されました。確認なしに再作成しません",
                ));
            }
            (None, Some(_)) => {
                return Err(DocumentError::conflict(
                    "保存先に既存ファイルがあります。確認なしに上書きしません",
                ));
            }
            _ => {}
        }

        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent).map_err(|error| DocumentError::io(error.to_string()))?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)
            .map_err(|error| DocumentError::io(error.to_string()))?;
        temporary
            .write_all(content)
            .and_then(|()| temporary.as_file().sync_all())
            .map_err(|error| DocumentError::io(error.to_string()))?;
        temporary
            .persist(path)
            .map_err(|error| DocumentError::io(error.error.to_string()))?;
        self.fingerprint(path)?
            .ok_or_else(|| DocumentError::io("保存後のファイルを確認できません"))
    }

    fn fingerprint(&self, path: &Path) -> Result<Option<FileFingerprint>, DocumentError> {
        match fs::read(path) {
            Ok(bytes) => fingerprint_for(path, &bytes).map(Some),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(DocumentError::io(error.to_string())),
        }
    }
}

impl WorkspaceFileBackend for LocalWorkspaceRepository {
    fn create_directory(&self, directory: &Path) -> Result<(), DocumentError> {
        fs::create_dir(directory).map_err(|error| DocumentError::io(error.to_string()))
    }

    fn list_children(&self, directory: &Path) -> Result<Vec<WorkspaceEntry>, DocumentError> {
        let entries = fs::read_dir(directory).map_err(|error| {
            DocumentError::io(format!(
                "Workspaceディレクトリの項目を読み込めません: {}: {error}",
                directory.display()
            ))
        })?;
        let mut children = Vec::new();

        for entry in entries {
            let entry = entry.map_err(|error| {
                DocumentError::io(format!(
                    "Workspaceディレクトリの項目を読み込めません: {}: {error}",
                    directory.display()
                ))
            })?;
            let file_name = entry.file_name();
            let name = file_name.into_string().map_err(|file_name| {
                DocumentError::io(format!(
                    "Workspace項目名をUTF-8へ変換できません: {file_name:?}"
                ))
            })?;
            let metadata = fs::symlink_metadata(entry.path()).map_err(|error| {
                DocumentError::io(format!(
                    "Workspace項目の種類を確認できません: {}: {error}",
                    entry.path().display()
                ))
            })?;
            let file_type = metadata.file_type();
            let kind = if file_type.is_symlink() {
                WorkspaceEntryKind::Symlink
            } else if file_type.is_dir() {
                WorkspaceEntryKind::Directory
            } else if file_type.is_file() {
                WorkspaceEntryKind::File
            } else {
                return Err(DocumentError::io(format!(
                    "Workspace項目の種類に対応していません: {}",
                    entry.path().display()
                )));
            };

            children.push(WorkspaceEntry { name, kind });
        }

        children.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(children)
    }

    fn changes(&self) -> Option<mpsc::Receiver<()>> {
        let root = self.root.as_ref()?;
        let mut slot = self.watch.lock().ok()?;
        if slot.is_some() {
            return None;
        }
        // 通知は1件で足りる。監視callbackを塞がないよう同期channelを使う。
        let (sender, receiver) = mpsc::sync_channel(1);
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let Ok(event) = event else {
                    // 取りこぼしの可能性があるため、再同期を促す。
                    let _ = sender.try_send(());
                    return;
                };
                if matches!(event.kind, notify::EventKind::Access(_))
                    || event.paths.iter().all(|path| is_ignored_path(path))
                {
                    return;
                }
                let _ = sender.try_send(());
            })
            .ok()?;
        watcher.watch(root, notify::RecursiveMode::Recursive).ok()?;
        *slot = Some(WorkspaceWatch { _watcher: watcher });
        Some(receiver)
    }
}

impl WorkspaceRepository for LocalWorkspaceRepository {
    fn create_directory(&self, root: &Path, name: &str) -> Result<(), WorkspaceError> {
        let mut components = Path::new(name).components();
        if !matches!(components.next(), Some(std::path::Component::Normal(_)))
            || components.next().is_some()
        {
            return Err(WorkspaceError::new("フォルダ名が不正です"));
        }
        let root = root
            .canonicalize()
            .map_err(|error| WorkspaceError::new(error.to_string()))?;
        fs::create_dir(root.join(name)).map_err(|error| WorkspaceError::new(error.to_string()))
    }

    fn list_tree(&self, root: &Path) -> Result<Vec<FileEntry>, WorkspaceError> {
        if !root.is_dir() {
            return Err(WorkspaceError::new(format!(
                "Workspaceフォルダーが存在しません: {}",
                root.display()
            )));
        }
        let mut entries = Vec::new();
        collect_tree(root, root, 0, &mut entries)?;
        Ok(entries)
    }
}

fn collect_tree(
    root: &Path,
    directory: &Path,
    depth: usize,
    output: &mut Vec<FileEntry>,
) -> Result<(), WorkspaceError> {
    let read_dir =
        fs::read_dir(directory).map_err(|error| WorkspaceError::new(error.to_string()))?;
    let mut children = read_dir
        .filter_map(Result::ok)
        .filter(|entry| {
            !entry
                .file_name()
                .to_str()
                .is_some_and(|name| IGNORED_NAMES.contains(&name))
        })
        .collect::<Vec<_>>();
    children.sort_by(|left, right| {
        let left_dir = left.file_type().is_ok_and(|kind| kind.is_dir());
        let right_dir = right.file_type().is_ok_and(|kind| kind.is_dir());
        right_dir
            .cmp(&left_dir)
            .then_with(|| left.file_name().cmp(&right.file_name()))
    });

    for child in children {
        let path = child.path();
        let file_type = child
            .file_type()
            .map_err(|error| WorkspaceError::new(error.to_string()))?;
        let kind = if file_type.is_dir() {
            FileEntryKind::Directory
        } else {
            FileEntryKind::File
        };
        output.push(FileEntry {
            relative_path: path.strip_prefix(root).unwrap_or(&path).to_owned(),
            path: path.clone(),
            kind,
            depth,
        });
        if file_type.is_dir() && !file_type.is_symlink() {
            collect_tree(root, &path, depth.saturating_add(1), output)?;
        }
    }
    Ok(())
}

fn fingerprint_for(path: &Path, bytes: &[u8]) -> Result<FileFingerprint, DocumentError> {
    let metadata = fs::metadata(path).map_err(|error| DocumentError::io(error.to_string()))?;
    let modified_nanos = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos());
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    Ok(FileFingerprint::new(metadata.len(), modified_nanos, hash))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_repository_round_trips_and_detects_conflicts() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        let repository = LocalWorkspaceRepository::default();

        let saved = repository.write_file(&path, b"# Hello\n", None).unwrap();
        assert_eq!(repository.read_file(&path).unwrap().bytes, b"# Hello\n");

        fs::write(&path, b"external").unwrap();
        assert!(
            repository
                .write_file(&path, b"local", Some(&saved))
                .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), b"external");
    }

    #[test]
    fn list_children_returns_sorted_immediate_entries() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("z-directory")).unwrap();
        fs::create_dir_all(directory.path().join("nested").join("ignored")).unwrap();
        fs::write(directory.path().join("a-file"), b"file").unwrap();

        let entries = LocalWorkspaceRepository::default()
            .list_children(directory.path())
            .unwrap();

        assert_eq!(
            entries,
            vec![
                WorkspaceEntry {
                    name: "a-file".to_owned(),
                    kind: WorkspaceEntryKind::File,
                },
                WorkspaceEntry {
                    name: "nested".to_owned(),
                    kind: WorkspaceEntryKind::Directory,
                },
                WorkspaceEntry {
                    name: "z-directory".to_owned(),
                    kind: WorkspaceEntryKind::Directory,
                },
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn list_children_classifies_symlinks_without_following_them() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("target"), b"file").unwrap();
        symlink(
            directory.path().join("target"),
            directory.path().join("link"),
        )
        .unwrap();

        let entries = LocalWorkspaceRepository::default()
            .list_children(directory.path())
            .unwrap();

        assert_eq!(entries[0].name, "link");
        assert_eq!(entries[0].kind, WorkspaceEntryKind::Symlink);
        assert_eq!(entries[1].kind, WorkspaceEntryKind::File);
    }

    #[cfg(unix)]
    #[test]
    fn list_children_rejects_non_utf8_names_as_document_errors() {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};

        let directory = tempfile::tempdir().unwrap();
        let name = OsString::from_vec(vec![b'i', b'n', b'v', b'a', b'l', b'i', b'd', 0xff]);
        fs::write(directory.path().join(name), b"file").unwrap();

        let error = LocalWorkspaceRepository::default()
            .list_children(directory.path())
            .unwrap_err();
        assert_eq!(error.kind(), document::DocumentErrorKind::Io);
    }

    #[test]
    fn list_children_maps_directory_io_errors_to_document_errors() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("file");
        fs::write(&file, b"not a directory").unwrap();

        let error = LocalWorkspaceRepository::default()
            .list_children(&file)
            .unwrap_err();
        assert_eq!(error.kind(), document::DocumentErrorKind::Io);
    }

    #[test]
    fn file_tree_uses_real_paths_and_skips_internal_directories() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir_all(directory.path().join("src")).unwrap();
        fs::create_dir_all(directory.path().join("target")).unwrap();
        fs::write(directory.path().join("src").join("main.rs"), "fn main() {}").unwrap();
        fs::write(directory.path().join("target").join("hidden"), "x").unwrap();

        let tree = LocalWorkspaceRepository::default()
            .list_tree(directory.path())
            .unwrap();
        assert!(
            tree.iter()
                .any(|entry| entry.relative_path == Path::new("src/main.rs"))
        );
        assert!(
            !tree
                .iter()
                .any(|entry| entry.relative_path.starts_with("target"))
        );
    }
}
