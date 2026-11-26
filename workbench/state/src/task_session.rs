use editor_core::{ConversationId, ExecutionId, WorkspaceId};
use std::sync::Arc;
use task::{TaskControl, TaskError, TaskMode};

pub struct TaskSession {
    backend: Arc<dyn task::TaskConnection>,
    conversation: ConversationId,
    workspace_id: Option<WorkspaceId>,
    records: Vec<task::TaskSnapshot>,
    restore_warning: Option<String>,
}

impl TaskSession {
    pub fn new(backend: Arc<dyn task::TaskConnection>) -> Self {
        Self {
            backend,
            conversation: ConversationId::new("conversation-local"),
            workspace_id: None,
            records: Vec::new(),
            restore_warning: None,
        }
    }

    pub fn records(&self) -> &[task::TaskSnapshot] {
        &self.records
    }
    pub fn restore_warning(&self) -> Option<&str> {
        self.restore_warning.as_deref()
    }

    pub fn refresh(&mut self, workspace: &WorkspaceId) -> Result<bool, TaskError> {
        let records = self.backend.load(workspace)?;
        let changed = self.workspace_id.as_ref() != Some(workspace) || self.records != records;
        self.workspace_id = Some(workspace.clone());
        self.records = records;
        Ok(changed)
    }

    pub fn start_codex(
        &mut self,
        workspace: WorkspaceId,
        prompt: impl Into<String>,
    ) -> Result<ExecutionId, TaskError> {
        self.start_codex_with_mode(workspace, prompt, TaskMode::Default)
    }

    pub fn start_codex_with_mode(
        &mut self,
        workspace: WorkspaceId,
        prompt: impl Into<String>,
        mode: TaskMode,
    ) -> Result<ExecutionId, TaskError> {
        self.start(workspace, prompt.into(), mode, false, None)
    }

    /// ACP 経由のハーネスを開始する。ACP 固有の型は渡さず runner 識別子だけを選ぶ。
    ///
    /// ACP の安定面にはモード切替が無いため、`TaskMode::Default` 以外は実行前に拒否する。
    /// 記録と実際の実行条件を一致させるため。
    pub fn start_agent(
        &mut self,
        workspace: WorkspaceId,
        prompt: impl Into<String>,
        mode: TaskMode,
    ) -> Result<ExecutionId, TaskError> {
        if mode != TaskMode::Default {
            return Err(TaskError::new(
                "ACP runner は TaskMode::Default のみ対応しています",
            ));
        }
        self.start(
            workspace,
            prompt.into(),
            mode,
            false,
            Some(acp_contract::ACP_RUNNER.to_owned()),
        )
    }

    pub fn start_codex_in_worktree(
        &mut self,
        workspace: WorkspaceId,
        prompt: impl Into<String>,
        mode: TaskMode,
    ) -> Result<ExecutionId, TaskError> {
        self.start(workspace, prompt.into(), mode, true, None)
    }

    fn start(
        &mut self,
        workspace: WorkspaceId,
        prompt: String,
        mode: TaskMode,
        isolated: bool,
        runner: Option<String>,
    ) -> Result<ExecutionId, TaskError> {
        let record = self.backend.start(
            &workspace,
            task::TaskStart {
                prompt,
                mode,
                isolated,
                conversation_id: self.conversation.clone(),
                runner,
            },
        )?;
        let execution_id = record.execution.id.clone();
        if self.workspace_id.as_ref() != Some(&workspace) {
            self.records.clear();
        }
        self.workspace_id = Some(workspace);
        self.records.insert(0, record);
        Ok(execution_id)
    }

    pub fn control(
        &mut self,
        execution_id: &ExecutionId,
        control: TaskControl,
    ) -> Result<(), TaskError> {
        let record = self
            .records
            .iter()
            .find(|record| &record.execution.id == execution_id)
            .ok_or_else(|| TaskError::new("Task Execution が見つかりません"))?;
        self.backend
            .control(&record.execution.workspace_id, execution_id, &control)
    }

    pub fn backend(&self) -> Arc<dyn task::TaskConnection> {
        self.backend.clone()
    }

    pub fn set_conversation(&mut self, conversation: ConversationId) {
        self.conversation = conversation;
    }
}
