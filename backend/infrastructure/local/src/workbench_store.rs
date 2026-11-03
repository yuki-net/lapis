//! HostのWorkspaceごとの作業画面状態を原子的に保存する。

use lapis_backend_state::{WorkbenchPersistence, WorkbenchStoreResult};
use lapis_workbench::{ConversationViewState, VersionedViewState, ViewUpdateError};
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone)]
pub struct LocalWorkbenchStore {
    path: PathBuf,
}

impl LocalWorkbenchStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn load(&self) -> io::Result<VersionedViewState> {
        let mut file = match fs::File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(VersionedViewState::default());
            }
            Err(error) => return Err(error),
        };
        let mut bytes = Vec::new();
        (&mut file)
            .take(MAX_FILE_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "workbench state too large",
            ));
        }
        let state: VersionedViewState = serde_json::from_slice(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if !state.is_consistent() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "inconsistent workbench state",
            ));
        }
        Ok(state)
    }

    /// 保存中はWorkspace単位のlockを保持し、古いrevisionの更新を拒否する。
    pub fn compare_and_save(
        &self,
        expected: u64,
        view: ConversationViewState,
    ) -> io::Result<WorkbenchStoreResult> {
        let parent = self
            .path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let mut lock_path = self.path.as_os_str().to_owned();
        lock_path.push(".lock");
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)?;
        lock.lock()?;
        let mut state = self.load()?;
        match state.update(expected, view) {
            Ok(_) => {}
            Err(ViewUpdateError::Conflict { .. }) => {
                return Ok(WorkbenchStoreResult::Conflict(state));
            }
            Err(ViewUpdateError::RevisionOverflow) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "workbench revision overflow",
                ));
            }
        }
        let bytes = serde_json::to_vec(&state).map_err(io::Error::other)?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "workbench state too large",
            ));
        }
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        temporary.persist(&self.path).map_err(|error| error.error)?;
        Ok(WorkbenchStoreResult::Updated(state))
    }
}

impl WorkbenchPersistence for LocalWorkbenchStore {
    fn load(&self) -> io::Result<VersionedViewState> {
        LocalWorkbenchStore::load(self)
    }

    fn compare_and_save(
        &self,
        expected: u64,
        view: ConversationViewState,
    ) -> io::Result<WorkbenchStoreResult> {
        LocalWorkbenchStore::compare_and_save(self, expected, view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn saved_state_survives_reopening_and_stale_writes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        let store = LocalWorkbenchStore::new(path.clone());
        let view = ConversationViewState {
            show_panel_controls: false,
            ..Default::default()
        };
        let WorkbenchStoreResult::Updated(saved) = store.compare_and_save(0, view.clone()).unwrap()
        else {
            panic!("save")
        };
        let reopened = LocalWorkbenchStore::new(path);
        assert_eq!(reopened.load().unwrap(), saved);
        let WorkbenchStoreResult::Conflict(current) = reopened
            .compare_and_save(0, ConversationViewState::default())
            .unwrap()
        else {
            panic!("conflict")
        };
        assert_eq!(current, saved);
        assert_eq!(reopened.load().unwrap().view, Some(view));
    }

    #[test]
    fn concurrent_stores_do_not_overwrite_each_other() {
        let directory = tempfile::tempdir().unwrap();
        let store = LocalWorkbenchStore::new(directory.path().join("state.json"));
        let barrier = Arc::new(Barrier::new(3));
        let run = |enabled| {
            let store = store.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                store
                    .compare_and_save(
                        0,
                        ConversationViewState {
                            show_panel_controls: enabled,
                            ..Default::default()
                        },
                    )
                    .unwrap()
            })
        };
        let first = run(true);
        let second = run(false);
        barrier.wait();
        let results = [first.join().unwrap(), second.join().unwrap()];
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, WorkbenchStoreResult::Updated(_)))
                .count(),
            1
        );
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, WorkbenchStoreResult::Conflict(_)))
                .count(),
            1
        );
        assert_eq!(store.load().unwrap().revision, 1);
    }

    #[test]
    fn malformed_file_is_not_overwritten() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        fs::write(&path, b"broken").unwrap();
        let store = LocalWorkbenchStore::new(path.clone());
        assert!(
            store
                .compare_and_save(0, ConversationViewState::default())
                .is_err()
        );
        assert_eq!(fs::read(path).unwrap(), b"broken");
    }

    #[test]
    fn inconsistent_revision_is_not_overwritten() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        let bytes = br#"{"revision":2,"view":null}"#;
        fs::write(&path, bytes).unwrap();
        let store = LocalWorkbenchStore::new(path.clone());
        assert_eq!(store.load().unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert!(
            store
                .compare_and_save(0, ConversationViewState::default())
                .is_err()
        );
        assert_eq!(fs::read(path).unwrap(), bytes);
    }

    #[test]
    fn oversized_update_preserves_the_last_saved_state() {
        let directory = tempfile::tempdir().unwrap();
        let store = LocalWorkbenchStore::new(directory.path().join("state.json"));
        let WorkbenchStoreResult::Updated(saved) = store
            .compare_and_save(0, ConversationViewState::default())
            .unwrap()
        else {
            panic!("initial save")
        };
        let mut large = ConversationViewState::default();
        large
            .workbench
            .bindings
            .push(lapis_workbench::ViewBindingState {
                view: "x".repeat(MAX_FILE_BYTES as usize),
                scope: lapis_workbench::ViewScopeState::SharedWorkbench,
            });
        assert!(store.compare_and_save(saved.revision, large).is_err());
        assert_eq!(store.load().unwrap(), saved);
    }
}
