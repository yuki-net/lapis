//! Workspace の破棄と、外部実行資源の開始を直列化する。

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

type BusyCheck = dyn Fn(&Path) -> Result<bool, String> + Send + Sync;

#[derive(Default)]
struct State {
    removing: Vec<PathBuf>,
    checks: Vec<Arc<BusyCheck>>,
}

/// 実行資源の種類を知らずに Worktree の使用中判定を仲介する。
#[derive(Default)]
pub struct ResourceGuard(Mutex<State>);

impl ResourceGuard {
    pub fn add_check(&self, check: Arc<BusyCheck>) -> Result<(), String> {
        self.0
            .lock()
            .map_err(|error| error.to_string())?
            .checks
            .push(check);
        Ok(())
    }

    pub fn with_path<T>(
        &self,
        path: &Path,
        start: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let state = self.0.lock().map_err(|error| error.to_string())?;
        if state.removing.iter().any(|root| path.starts_with(root)) {
            return Err("Worktree is being removed".into());
        }
        start()
    }

    pub(crate) fn reserve(&self, root: &Path) -> Result<(), String> {
        let mut state = self.0.lock().map_err(|error| error.to_string())?;
        if state
            .removing
            .iter()
            .any(|other| root.starts_with(other) || other.starts_with(root))
        {
            return Err("Worktree is being removed".into());
        }
        for check in &state.checks {
            if check(root)? {
                return Err("Worktree has active resources".into());
            }
        }
        state.removing.push(root.to_owned());
        Ok(())
    }

    pub(crate) fn release(&self, root: &Path) {
        if let Ok(mut state) = self.0.lock() {
            state.removing.retain(|candidate| candidate != root);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn reservation_checks_resources_and_blocks_new_starts() {
        let guard = ResourceGuard::default();
        let busy = Arc::new(AtomicBool::new(true));
        let observed = Arc::clone(&busy);
        guard
            .add_check(Arc::new(move |_| Ok(observed.load(Ordering::Acquire))))
            .unwrap();
        let root = Path::new("worktree");
        assert!(guard.reserve(root).is_err());
        busy.store(false, Ordering::Release);
        guard.reserve(root).unwrap();
        assert!(guard.with_path(&root.join("child"), || Ok(())).is_err());
        assert!(guard.with_path(Path::new("other"), || Ok(())).is_ok());
        assert!(guard.reserve(&root.join("child")).is_err());
        guard.release(root);
        assert!(guard.with_path(&root.join("child"), || Ok(())).is_ok());
    }

    #[test]
    fn failed_check_does_not_leave_a_reservation() {
        let guard = ResourceGuard::default();
        guard
            .add_check(Arc::new(|_| Err("unavailable".into())))
            .unwrap();
        assert!(guard.reserve(Path::new("worktree")).is_err());
        assert!(guard.with_path(Path::new("worktree"), || Ok(())).is_ok());
    }

    #[test]
    fn resource_start_and_removal_check_are_atomic() {
        let guard = Arc::new(ResourceGuard::default());
        let busy = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&busy);
        guard
            .add_check(Arc::new(move |_| Ok(observed.load(Ordering::Acquire))))
            .unwrap();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let start_guard = Arc::clone(&guard);
        let start = std::thread::spawn(move || {
            start_guard
                .with_path(Path::new("worktree/child"), || {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    busy.store(true, Ordering::Release);
                    Ok(())
                })
                .unwrap();
        });
        entered_rx.recv().unwrap();
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        let removal = std::thread::spawn(move || {
            result_tx
                .send(guard.reserve(Path::new("worktree")))
                .unwrap()
        });
        assert!(result_rx.try_recv().is_err());
        release_tx.send(()).unwrap();
        start.join().unwrap();
        assert!(result_rx.recv().unwrap().is_err());
        removal.join().unwrap();
    }
}
