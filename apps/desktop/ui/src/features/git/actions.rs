use super::*;

impl Editor {
    pub(super) fn select_git_diff(&mut self, path: std::path::PathBuf, cx: &mut Context<Self>) {
        match self.git.session.select_diff(&path) {
            Ok(()) => self.status = format!("Diff: {}", path.display()),
            Err(error) => self.status = format!("Git diff失敗: {error}"),
        }
        cx.notify();
    }

    pub(super) fn select_worktree_diff(
        &mut self,
        task_id: TaskId,
        path: std::path::PathBuf,
        cx: &mut Context<Self>,
    ) {
        match self.git.session.select_worktree_diff(&task_id, &path) {
            Ok(()) => {
                self.status = format!("Worktree diff: {}", path.display());
            }
            Err(error) => self.status = format!("Worktree diff失敗: {error}"),
        }
        cx.notify();
    }

    pub(super) fn import_worktree_file(
        &mut self,
        task_id: TaskId,
        path: std::path::PathBuf,
        cx: &mut Context<Self>,
    ) {
        match self.git.session.import_file(&task_id, &path) {
            Ok(()) => self.status = format!("Worktreeから取込: {}", path.display()),
            Err(error) => self.status = format!("取込競合: {error}"),
        }
        cx.notify();
    }

    pub(super) fn discard_worktree(&mut self, task_id: TaskId, cx: &mut Context<Self>) {
        match self.git.session.discard_worktree(&task_id) {
            Ok(()) => self.status = "Task worktreeを破棄しました".to_owned(),
            Err(error) => self.status = format!("Worktree破棄失敗: {error}"),
        }
        cx.notify();
    }
}
