//! Task の操作。状態と遷移は `task_state` が所有する。

use super::*;

impl WorkspaceOperations {
    pub(super) fn records(&self, root: &Path) -> Result<Vec<TaskSnapshot>, BackendStateError> {
        let trees = self.trees_for_tasks(root);
        Ok(self.tasks.records(root, &trees)?)
    }

    pub(super) fn start(
        &self,
        input: TaskStart,
        root: &Path,
    ) -> Result<Outcome, BackendStateError> {
        task_state::validate_start(&input)?;
        if input.isolated {
            self.require_repository(root)?;
        }
        let snapshot = self.tasks.start(
            input,
            root,
            || self.trees_for_tasks(root),
            |task_id| {
                let git = self
                    .git()
                    .map_err(|_| task_state::TaskStateError::Unsupported)?;
                git.create_worktree(root, task_id).map_err(|_| {
                    task_state::TaskStateError::Operation("Git command failed".to_owned())
                })
            },
        )?;
        Ok(Outcome::TaskStarted(Box::new(snapshot)))
    }

    pub(super) fn control(
        &self,
        execution_id: &ExecutionId,
        control: &TaskControl,
        root: &Path,
    ) -> Result<(), BackendStateError> {
        if !self
            .records(root)?
            .iter()
            .any(|record| record.execution.id == *execution_id && task_state::is_running(record))
        {
            return Err(BackendStateError::WorkspaceDenied);
        }
        self.tasks.control(execution_id, control)?;
        Ok(())
    }
}
