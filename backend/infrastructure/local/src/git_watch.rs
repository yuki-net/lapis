use lapis_git::{GitError, GitErrorKind, GitWatch};
use notify::{EventKind, RecursiveMode, Watcher};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub(super) struct LocalGitWatch {
    _watcher: notify::RecommendedWatcher,
    changed: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
}

impl LocalGitWatch {
    pub(super) fn new(
        repository: &Path,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self, GitError> {
        let root = repository
            .canonicalize()
            .map_err(|error| GitError::new(GitErrorKind::Io, error.to_string()))?;
        let git_dir = super::git_text(&root, &["rev-parse", "--absolute-git-dir"])?;
        let git_dir = Path::new(git_dir.trim())
            .canonicalize()
            .map_err(|error| GitError::new(GitErrorKind::Io, error.to_string()))?;
        let common_dir = super::git_common_directory(&root)?;
        let changed = Arc::new(AtomicBool::new(false));
        let failed = Arc::new(AtomicBool::new(false));
        let changed_event = changed.clone();
        let failed_event = failed.clone();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| match event {
                Ok(event) if !matches!(event.kind, EventKind::Access(_)) => {
                    changed_event.store(true, Ordering::Release);
                    notify();
                }
                Ok(_) => {}
                Err(_) => {
                    failed_event.store(true, Ordering::Release);
                    notify();
                }
            })
            .map_err(|error| GitError::new(GitErrorKind::Io, error.to_string()))?;
        watcher
            .watch(&root, RecursiveMode::Recursive)
            .map_err(|error| GitError::new(GitErrorKind::Io, error.to_string()))?;
        if !common_dir.starts_with(&root) {
            watcher
                .watch(&common_dir, RecursiveMode::Recursive)
                .map_err(|error| GitError::new(GitErrorKind::Io, error.to_string()))?;
        }
        if !git_dir.starts_with(&root) && !git_dir.starts_with(&common_dir) {
            watcher
                .watch(&git_dir, RecursiveMode::Recursive)
                .map_err(|error| GitError::new(GitErrorKind::Io, error.to_string()))?;
        }
        Ok(Self {
            _watcher: watcher,
            changed,
            failed,
        })
    }
}

impl GitWatch for LocalGitWatch {
    fn take_changed(&self) -> bool {
        self.changed.swap(false, Ordering::AcqRel)
    }
    fn is_healthy(&self) -> bool {
        !self.failed.load(Ordering::Acquire)
    }
}
