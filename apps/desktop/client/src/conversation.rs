use super::*;
use lapis_editor_core::ProjectId;
use lapis_git::GitConnection;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConversationRecord {
    pub id: ConversationId,
    pub title: String,
    pub workspace: WorkspaceSnapshot,
    pub view: ConversationViewState,
    pub selected_execution: Option<ExecutionId>,
    pub terminals: Vec<RestoredTerminal>,
    #[serde(default)]
    pub environment: Option<TaskEnvironment>,
    #[serde(default)]
    pub source_role: SourceRole,
    #[serde(default)]
    pub other_workspace: Option<WorkspaceSnapshot>,
    #[serde(default)]
    pub other_terminals: Vec<RestoredTerminal>,
    #[serde(default)]
    pub other_execution: Option<ExecutionId>,
    #[serde(default)]
    pub parked_sources: Vec<TaskSourceState>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskSourceState {
    pub workspace: WorkspaceSnapshot,
    pub terminals: Vec<RestoredTerminal>,
    pub selected_execution: Option<ExecutionId>,
}

pub type ConversationState = (
    Vec<Project>,
    Vec<ConversationRecord>,
    Option<ConversationId>,
);

pub trait ConversationRepository: Send + Sync {
    fn load(&self) -> Result<ConversationState, WorkspaceError>;
    fn save(
        &self,
        expected: &ConversationState,
        projects: &[Project],
        records: &[ConversationRecord],
        active: &ConversationId,
    ) -> Result<(), WorkspaceError>;
}

#[derive(Clone)]
pub struct ConversationSession {
    repository: Arc<dyn ConversationRepository>,
    records: Vec<ConversationRecord>,
    active: ConversationId,
    next_id: u64,
    projects: Vec<Project>,
    load_error: Option<String>,
    persisted: ConversationState,
}

impl ConversationSession {
    pub fn new(repository: Arc<dyn ConversationRepository>, initial: WorkspaceSnapshot) -> Self {
        let loaded = repository.load();
        let load_error = loaded.as_ref().err().map(ToString::to_string);
        let persisted = loaded.as_ref().cloned().unwrap_or_default();
        let (mut projects, mut records, active) = loaded.unwrap_or_default();
        if records.is_empty() {
            records.push(ConversationRecord {
                id: ConversationId::new("conversation-1"),
                title: "Task 1".to_owned(),
                workspace: initial,
                view: ConversationViewState::default(),
                selected_execution: None,
                terminals: Vec::new(),
                environment: None,
                source_role: SourceRole::Primary,
                other_workspace: None,
                other_terminals: Vec::new(),
                other_execution: None,
                parked_sources: Vec::new(),
            });
        }
        let active = active
            .filter(|id| records.iter().any(|record| &record.id == id))
            .unwrap_or_else(|| records[0].id.clone());
        // 既存の会話をrootごとのProjectへ移行し、draftとTerminalの参照を保持する。
        for record in &mut records {
            if record.environment.is_none()
                && let Some(root) = record.workspace.root.clone()
            {
                let index = projects
                    .iter()
                    .position(|project| project.root() == root)
                    .unwrap_or_else(|| {
                        let mut number = projects.len() + 1;
                        while projects
                            .iter()
                            .any(|project| project.id().as_str() == format!("project-{number}"))
                        {
                            number += 1;
                        }
                        projects.push(Project::new(
                            ProjectId::new(format!("project-{number}")),
                            root.clone(),
                        ));
                        projects.len() - 1
                    });
                record.environment = Some(
                    TaskEnvironment::new(&projects[index], root, None).expect("registered root"),
                );
            }
        }
        let invalid = projects.iter().enumerate().any(|(index, project)| {
            project.root().as_os_str().is_empty()
                || projects[..index]
                    .iter()
                    .any(|other| other.id() == project.id())
        }) || records.iter().enumerate().any(|(index, record)| {
            records[..index].iter().any(|other| other.id == record.id)
                || record
                    .parked_sources
                    .iter()
                    .enumerate()
                    .any(|(index, source)| {
                        source.workspace.root.is_none()
                            || source.workspace.root == record.workspace.root
                            || record
                                .other_workspace
                                .as_ref()
                                .is_some_and(|other| other.root == source.workspace.root)
                            || record.parked_sources[..index]
                                .iter()
                                .any(|other| other.workspace.root == source.workspace.root)
                    })
                || record.environment.as_ref().is_some_and(|environment| {
                    !projects.iter().any(|project| environment.validate(project))
                        || environment.root(record.source_role) != record.workspace.root.as_deref()
                        || record.other_workspace.as_ref().is_some_and(|other| {
                            let other_role = match record.source_role {
                                SourceRole::Primary => SourceRole::Sub,
                                SourceRole::Sub => SourceRole::Primary,
                            };
                            other.root.as_deref() != environment.root(other_role)
                        })
                })
        });
        let load_error = load_error.or_else(|| {
            invalid.then(|| "Project/Taskの保存状態が不正です。元の保存ファイルを保持します".into())
        });
        let mut next_id = records.len() as u64 + 1;
        while records
            .iter()
            .any(|record| record.id.as_str() == format!("conversation-{next_id}"))
        {
            next_id += 1;
        }
        Self {
            repository,
            records,
            active,
            next_id,
            projects,
            load_error,
            persisted,
        }
    }

    pub fn records(&self) -> &[ConversationRecord] {
        &self.records
    }
    pub fn projects(&self) -> &[Project] {
        &self.projects
    }
    pub fn load_error(&self) -> Option<&str> {
        self.load_error.as_deref()
    }
    fn save(&mut self, editor: &mut EditorSession) -> Result<(), WorkspaceError> {
        let mut restore = None;
        if let Some(error) = &self.load_error {
            return Err(WorkspaceError::new(error));
        }
        if let Err(error) =
            self.repository
                .save(&self.persisted, &self.projects, &self.records, &self.active)
        {
            let mut latest = Self::new(self.repository.clone(), Default::default());
            if latest.load_error.is_some() || latest.persisted == self.persisted {
                return Err(error);
            }
            latest.active = self.active.clone();
            let mut projects = Vec::new();
            for project in &self.projects {
                let index = if let Some(index) = latest
                    .projects
                    .iter()
                    .position(|item| item.root() == project.root())
                {
                    index
                } else {
                    let mut number = latest.projects.len() + 1;
                    while latest
                        .projects
                        .iter()
                        .any(|item| item.id().as_str() == format!("project-{number}"))
                    {
                        number += 1;
                    }
                    latest.projects.push(Project::new(
                        ProjectId::new(format!("project-{number}")),
                        project.root().to_owned(),
                    ));
                    latest.projects.len() - 1
                };
                for source in project.sources() {
                    latest.projects[index].register_source(source.clone());
                }
                projects.push((project.id().clone(), latest.projects[index].clone()));
            }
            for record in &self.records {
                let base = self.persisted.1.iter().find(|item| item.id == record.id);
                if base == Some(record) {
                    continue;
                }
                let mut local = record.clone();
                if let Some(environment) = &mut local.environment {
                    let project = projects
                        .iter()
                        .find(|(id, _)| id == environment.project())
                        .ok_or_else(|| WorkspaceError::new("復旧するTaskのProjectがありません"))?;
                    environment
                        .reassign_project(&project.1)
                        .map_err(WorkspaceError::new)?;
                }
                let index = latest.records.iter().position(|item| item.id == record.id);
                if index.is_some_and(|index| latest.records[index] == local) {
                    continue;
                }
                if index.is_some_and(|index| Some(&latest.records[index]) != base) {
                    // 同じTaskを両Windowが変更した場合は、どちらの下書きも上書きしない。
                    while latest
                        .records
                        .iter()
                        .any(|item| item.id.as_str() == format!("conversation-{}", latest.next_id))
                    {
                        latest.next_id += 1;
                    }
                    local.id = ConversationId::new(format!("conversation-{}", latest.next_id));
                    latest.next_id += 1;
                    local.title.push_str("（競合から復旧）");
                    if record.id == self.active {
                        latest.active = local.id.clone();
                    }
                    latest.records.push(local);
                } else if let Some(index) = index {
                    latest.records[index] = local;
                } else {
                    latest.records.push(local);
                }
            }
            latest.repository.save(
                &latest.persisted,
                &latest.projects,
                &latest.records,
                &latest.active,
            )?;
            let before = self.active_record().map(|record| &record.workspace);
            let after = latest.active_record().map(|record| &record.workspace);
            if before != after {
                restore = after.cloned();
            }
            *self = latest;
        }
        self.persisted = (
            self.projects.clone(),
            self.records.clone(),
            Some(self.active.clone()),
        );
        if let Some(workspace) = restore {
            editor.restore_snapshot(workspace)?;
        }
        Ok(())
    }

    pub fn close_workspace(&mut self, editor: &mut EditorSession) -> Result<(), WorkspaceError> {
        self.transaction(editor, |session, editor| {
            editor.close_workspace()?;
            session.create(editor, Default::default())?;
            Ok(())
        })
    }
    pub fn active_id(&self) -> &ConversationId {
        &self.active
    }

    /// Task表示が参照するWorkspaceを解決する。Editorの作業環境は切り替えない。
    /// Document接続を呼ぶためClient workerから実行する。
    pub fn task_workspace_id(
        &self,
        id: &ConversationId,
        editor: &EditorSession,
    ) -> Result<Option<WorkspaceId>, WorkspaceError> {
        let record = self
            .records
            .iter()
            .find(|record| &record.id == id)
            .ok_or_else(|| WorkspaceError::new("Taskが見つかりません"))?;
        record
            .workspace
            .root
            .as_ref()
            .map(|root| {
                editor
                    .document_connection
                    .workspace(root)
                    .map_err(|error| WorkspaceError::new(error.to_string()))
            })
            .transpose()
    }
    pub fn active_record(&self) -> Option<&ConversationRecord> {
        self.records.iter().find(|record| record.id == self.active)
    }

    pub fn active_view(&self) -> ConversationViewState {
        self.records
            .iter()
            .find(|record| record.id == self.active)
            .map(|record| record.view.clone())
            .unwrap_or_default()
    }

    pub fn restore_active(
        &self,
        editor: &mut EditorSession,
    ) -> Result<ConversationViewState, WorkspaceError> {
        let record = self
            .records
            .iter()
            .find(|record| record.id == self.active)
            .ok_or_else(|| WorkspaceError::new("Conversation が見つかりません"))?;
        editor.restore_snapshot(record.workspace.clone())?;
        Ok(record.view.clone())
    }

    /// 開いた Project が最後に保存した Conversation と同じ場合だけ、その状態を復元する。
    /// アプリ起動直後に無条件で以前の Project を開かないための境界でもある。
    pub fn restore_matching_workspace(
        &self,
        editor: &mut EditorSession,
    ) -> Result<Option<ConversationViewState>, WorkspaceError> {
        let Some(current_root) = editor.workspace_root() else {
            return Ok(None);
        };
        let Some(record) = self.active_record() else {
            return Ok(None);
        };
        if record.workspace.root.as_deref() != Some(current_root) {
            return Ok(None);
        }
        editor.restore_snapshot(record.workspace.clone())?;
        Ok(Some(record.view.clone()))
    }

    pub fn capture(
        &mut self,
        editor: &mut EditorSession,
        view: ConversationViewState,
        selected_execution: Option<ExecutionId>,
        terminals: &[TerminalSnapshot],
    ) -> Result<(), WorkspaceError> {
        self.capture_with_terminals(editor, view, selected_execution, Some(terminals))
    }

    /// 未確定のTerminal表示は空の確定状態として保存せず、既存Resource参照を維持します。
    pub fn capture_with_terminals(
        &mut self,
        editor: &mut EditorSession,
        view: ConversationViewState,
        selected_execution: Option<ExecutionId>,
        terminals: Option<&[TerminalSnapshot]>,
    ) -> Result<(), WorkspaceError> {
        let owner = self.active.clone();
        self.capture_bound(editor, view, selected_execution, Some(&owner), terminals)
    }

    /// 表示中Terminalの参照はEditorのTaskへ移さず、実際の所有Taskに保存します。
    pub fn capture_bound(
        &mut self,
        editor: &mut EditorSession,
        view: ConversationViewState,
        selected_execution: Option<ExecutionId>,
        terminal_owner: Option<&ConversationId>,
        terminals: Option<&[TerminalSnapshot]>,
    ) -> Result<(), WorkspaceError> {
        if let Some(record) = self
            .records
            .iter_mut()
            .find(|record| record.id == self.active)
        {
            let workspace = editor.snapshot();
            if let Some(environment) = &record.environment
                && environment.root(record.source_role) != workspace.root.as_deref()
            {
                return Err(WorkspaceError::new(
                    "Taskの操作対象が変わっています。ProjectのTaskを選び直してください",
                ));
            }
            record.workspace = workspace;
            record.view = view;
            record.selected_execution = selected_execution;
        }
        if let Some(terminals) = terminals
            && let Some(record) = self
                .records
                .iter_mut()
                .find(|record| Some(&record.id) == terminal_owner)
        {
            record.terminals = terminals
                .iter()
                .map(|terminal| RestoredTerminal {
                    id: Some(terminal.id.clone()),
                    cwd: terminal.cwd.clone(),
                    status: terminal.status,
                    columns: terminal.screen.size.columns,
                    rows: terminal.screen.size.rows,
                })
                .collect();
        }
        self.save(editor)
    }

    pub fn create(
        &mut self,
        editor: &mut EditorSession,
        view: ConversationViewState,
    ) -> Result<ConversationId, WorkspaceError> {
        let environment = editor
            .workspace_root()
            .map(|root| {
                let project = self
                    .projects
                    .iter()
                    .find(|project| project.sources().any(|source| source == root))
                    .ok_or_else(|| WorkspaceError::new("Projectに既存環境を登録してください"))?;
                TaskEnvironment::new(project, root.to_owned(), None).map_err(WorkspaceError::new)
            })
            .transpose()?;
        let previous = self.clone();
        while self
            .records
            .iter()
            .any(|record| record.id.as_str() == format!("conversation-{}", self.next_id))
        {
            self.next_id += 1;
        }
        let id = ConversationId::new(format!("conversation-{}", self.next_id));
        self.next_id += 1;
        self.records.push(ConversationRecord {
            id: id.clone(),
            title: format!("Conversation {}", self.records.len() + 1),
            workspace: editor.snapshot(),
            view,
            selected_execution: None,
            terminals: Vec::new(),
            environment,
            source_role: SourceRole::Primary,
            other_workspace: None,
            other_terminals: Vec::new(),
            other_execution: None,
            parked_sources: Vec::new(),
        });
        self.active = id.clone();
        if let Err(error) = self.save(editor) {
            *self = previous;
            return Err(error);
        }
        Ok(self.active.clone())
    }

    pub fn switch(
        &mut self,
        id: &ConversationId,
        editor: &mut EditorSession,
    ) -> Result<ConversationViewState, WorkspaceError> {
        let record = self
            .records
            .iter()
            .find(|record| &record.id == id)
            .cloned()
            .ok_or_else(|| WorkspaceError::new("Conversation が見つかりません"))?;
        self.transaction(editor, |session, editor| {
            editor.restore_snapshot(record.workspace)?;
            session.active = id.clone();
            session.save(editor)?;
            Ok(record.view)
        })
    }

    pub fn repository(&self) -> Arc<dyn ConversationRepository> {
        self.repository.clone()
    }

    fn validate_source(editor: &EditorSession, root: &Path) -> Result<WorkspaceId, WorkspaceError> {
        let id = editor
            .document_connection
            .workspace(root)
            .map_err(|error| WorkspaceError::new(error.to_string()))?;
        editor.repository.list_tree(root)?;
        Ok(id)
    }

    pub fn register_project(
        &mut self,
        editor: &mut EditorSession,
        root: PathBuf,
    ) -> Result<ProjectId, WorkspaceError> {
        let workspace = Self::validate_source(editor, &root)?;
        for project in &self.projects {
            if editor
                .document_connection
                .workspace(project.root())
                .ok()
                .as_ref()
                == Some(&workspace)
            {
                return Ok(project.id().clone());
            }
        }
        let mut number = self.projects.len() + 1;
        while self
            .projects
            .iter()
            .any(|project| project.id().as_str() == format!("project-{number}"))
        {
            number += 1;
        }
        let id = ProjectId::new(format!("project-{number}"));
        self.projects.push(Project::new(id.clone(), root.clone()));
        if let Err(error) = self.save(editor) {
            self.projects.pop();
            return Err(error);
        }
        Ok(self
            .projects
            .iter()
            .find(|project| project.root() == root)
            .expect("saved project")
            .id()
            .clone())
    }

    /// 既存Taskを再利用する。複数候補のときは明示的なTask選択を求める。
    pub fn open_project(
        &mut self,
        editor: &mut EditorSession,
        root: PathBuf,
    ) -> Result<(), WorkspaceError> {
        let project = self.register_project(editor, root.clone())?;
        let candidates: Vec<_> = self
            .records
            .iter()
            .filter(|record| {
                record
                    .environment
                    .as_ref()
                    .is_some_and(|environment| environment.project() == &project)
            })
            .map(|record| record.id.clone())
            .collect();
        match candidates.as_slice() {
            [] => {
                self.create_task(editor, &project, root, None)?;
            }
            [id] => {
                self.switch(id, editor)?;
            }
            _ => {
                return Err(WorkspaceError::new(
                    "このProjectには複数のTaskがあります。左側のProject / Taskから選択してください",
                ));
            }
        }
        Ok(())
    }

    pub fn register_source(
        &mut self,
        editor: &mut EditorSession,
        id: &ProjectId,
        root: PathBuf,
    ) -> Result<(), WorkspaceError> {
        let workspace = Self::validate_source(editor, &root)?;
        let project = self
            .projects
            .iter()
            .find(|project| project.id() == id)
            .ok_or_else(|| WorkspaceError::new("Projectが見つかりません"))?;
        if project.sources().any(|source| {
            editor.document_connection.workspace(source).ok().as_ref() == Some(&workspace)
        }) {
            return Ok(());
        }
        let previous = self.projects.clone();
        self.projects
            .iter_mut()
            .find(|project| project.id() == id)
            .unwrap()
            .register_source(root);
        if let Err(error) = self.save(editor) {
            self.projects = previous;
            return Err(error);
        }
        Ok(())
    }

    fn transaction<T>(
        &mut self,
        editor: &mut EditorSession,
        operation: impl FnOnce(&mut Self, &mut EditorSession) -> Result<T, WorkspaceError>,
    ) -> Result<T, WorkspaceError> {
        let previous = self.clone();
        let workspace = editor.snapshot();
        let result = operation(self, editor);
        if result.is_err() {
            *self = previous;
            if editor.snapshot() != workspace {
                editor.restore_snapshot(workspace)?;
            }
        }
        result
    }

    pub fn create_task(
        &mut self,
        editor: &mut EditorSession,
        project: &ProjectId,
        primary: PathBuf,
        sub: Option<PathBuf>,
    ) -> Result<ConversationId, WorkspaceError> {
        let project = self
            .projects
            .iter()
            .find(|candidate| candidate.id() == project)
            .ok_or_else(|| WorkspaceError::new("Projectが見つかりません"))?;
        let environment = TaskEnvironment::new(project, primary.clone(), sub.clone())
            .map_err(WorkspaceError::new)?;
        let primary_id = Self::validate_source(editor, &primary)?;
        if let Some(sub) = &sub
            && Self::validate_source(editor, sub)? == primary_id
        {
            return Err(WorkspaceError::new(
                "PrimaryとSubは同じ環境を選択できません",
            ));
        }
        self.transaction(editor, move |session, editor| {
            editor.open_workspace(primary)?;
            while session
                .records
                .iter()
                .any(|record| record.id.as_str() == format!("conversation-{}", session.next_id))
            {
                session.next_id += 1;
            }
            let id = ConversationId::new(format!("conversation-{}", session.next_id));
            session.next_id += 1;
            session.records.push(ConversationRecord {
                id: id.clone(),
                title: format!("Task {}", session.next_id - 1),
                workspace: editor.snapshot(),
                view: ConversationViewState {
                    active_tool: "projects".into(),
                    ..Default::default()
                },
                selected_execution: None,
                terminals: Vec::new(),
                environment: Some(environment),
                source_role: SourceRole::Primary,
                other_workspace: None,
                other_terminals: Vec::new(),
                other_execution: None,
                parked_sources: Vec::new(),
            });
            session.active = id.clone();
            session.save(editor)?;
            Ok(session.active.clone())
        })
    }

    pub fn discard_worktree(
        &mut self,
        editor: &mut EditorSession,
        git: &dyn GitConnection,
        id: &TaskId,
    ) -> Result<(), WorkspaceError> {
        let workspace = editor
            .workspace_id()
            .cloned()
            .ok_or_else(|| WorkspaceError::new("Workspaceがありません"))?;
        let root = git
            .open_worktree(&workspace, id)
            .map_err(|error| WorkspaceError::new(error.to_string()))?;
        // 別Windowが保存したTaskと、まだ保存していない手元の状態の両方を保護する。
        let (_, saved_records, _) = self.repository.load()?;
        let in_use = |record: &ConversationRecord| {
            std::iter::once(&record.workspace)
                .chain(record.other_workspace.iter())
                .any(|workspace| {
                    workspace
                        .root
                        .as_ref()
                        .is_some_and(|path| path.starts_with(&root))
                })
                || record.parked_sources.iter().any(|source| {
                    source
                        .workspace
                        .root
                        .as_ref()
                        .is_some_and(|path| path.starts_with(&root))
                        && (source
                            .workspace
                            .open_documents
                            .iter()
                            .any(|document| document.draft_content.is_some())
                            || source.terminals.iter().any(|terminal| {
                                !matches!(
                                    terminal.status,
                                    lapis_terminal::TerminalStatus::Exited
                                        | lapis_terminal::TerminalStatus::Failed
                                )
                            }))
                })
                || record.environment.as_ref().is_some_and(|environment| {
                    [SourceRole::Primary, SourceRole::Sub]
                        .into_iter()
                        .any(|role| {
                            environment
                                .root(role)
                                .is_some_and(|path| path.starts_with(&root))
                        })
                })
        };
        for record in self.records.iter().chain(&saved_records) {
            if in_use(record) {
                return Err(WorkspaceError::new(format!(
                    "Task「{}」が作業状態を保持しています。Worktreeは削除できません",
                    record.title
                )));
            }
        }
        let mut previous = self.clone();
        let mut next = self.clone();
        // 保存を先に確定する。失敗した場合はGitの削除を開始しない。
        for record in &mut next.records {
            record.parked_sources.retain(|source| {
                !source
                    .workspace
                    .root
                    .as_ref()
                    .is_some_and(|path| path.starts_with(&root))
            });
        }
        next.save(editor)?;
        *self = next;
        if editor.workspace_id() != Some(&workspace)
            || self.records.iter().any(|record| {
                in_use(record)
                    || record.parked_sources.iter().any(|source| {
                        source
                            .workspace
                            .root
                            .as_ref()
                            .is_some_and(|path| path.starts_with(&root))
                    })
            })
        {
            return Err(WorkspaceError::new(
                "Taskの保存状態が更新されました。Worktree削除をやり直してください",
            ));
        }
        if let Err(error) = git.discard_worktree(&workspace, id) {
            previous.persisted = self.persisted.clone();
            let restored = previous.save(editor);
            *self = previous;
            return Err(WorkspaceError::new(match restored {
                Ok(()) => error.to_string(),
                Err(restore) => format!(
                    "{error}。退避状態は手元に保持していますが、保存の復旧に失敗しました: {restore}"
                ),
            }));
        }
        Ok(())
    }

    pub fn set_worktree(
        &mut self,
        editor: &mut EditorSession,
        git: &dyn GitConnection,
        worktree: Option<&TaskId>,
    ) -> Result<(), WorkspaceError> {
        let record = self
            .active_record()
            .ok_or_else(|| WorkspaceError::new("Taskが見つかりません"))?;
        let environment = record
            .environment
            .as_ref()
            .ok_or_else(|| WorkspaceError::new("作業Taskを選択してください"))?;
        let role = record.source_role;
        if environment.root(role) != editor.workspace_root() {
            return Err(WorkspaceError::new("Taskの操作対象が変わっています"));
        }
        if worktree.is_some() && environment.worktree(role).map(|binding| &binding.id) == worktree {
            return Ok(());
        }
        let source = environment
            .source_root(role)
            .ok_or_else(|| WorkspaceError::new("Taskの作業環境がありません"))?
            .to_owned();
        let binding = worktree
            .map(|id| {
                let source_id = Self::validate_source(editor, &source)?;
                let workspace = Self::validate_source(editor, editor.workspace_root().unwrap())?;
                let root = git
                    .open_worktree(&workspace, id)
                    .map_err(|error| WorkspaceError::new(error.to_string()))?;
                if Self::validate_source(editor, &root)? == source_id {
                    return Ok(None);
                }
                Ok::<_, WorkspaceError>(Some(lapis_project::WorktreeBinding {
                    id: id.clone(),
                    root,
                }))
            })
            .transpose()?
            .flatten();
        let mut environment = environment.clone();
        environment
            .set_worktree(role, binding)
            .map_err(WorkspaceError::new)?;
        let root = environment.root(role).unwrap().to_owned();
        if editor.workspace_root() == Some(root.as_path()) {
            return Ok(());
        }
        self.transaction(editor, move |session, editor| {
            let record = session
                .records
                .iter_mut()
                .find(|record| record.id == session.active)
                .unwrap();
            let next = record
                .parked_sources
                .iter()
                .position(|source| source.workspace.root.as_deref() == Some(root.as_path()))
                .map(|index| record.parked_sources.remove(index))
                .unwrap_or_else(|| TaskSourceState {
                    workspace: WorkspaceSnapshot {
                        root: Some(root),
                        ..Default::default()
                    },
                    terminals: Vec::new(),
                    selected_execution: None,
                });
            let previous = TaskSourceState {
                workspace: editor.snapshot(),
                terminals: record.terminals.clone(),
                selected_execution: record.selected_execution.clone(),
            };
            editor.restore_snapshot(next.workspace)?;
            record.workspace = editor.snapshot();
            record.terminals = next.terminals;
            record.selected_execution = next.selected_execution;
            record.environment = Some(environment);
            record.parked_sources.push(previous);
            session.save(editor)
        })
    }

    pub fn switch_source(
        &mut self,
        editor: &mut EditorSession,
        role: SourceRole,
    ) -> Result<(), WorkspaceError> {
        self.transaction(editor, |session, editor| {
            let record = session
                .records
                .iter_mut()
                .find(|record| record.id == session.active)
                .ok_or_else(|| WorkspaceError::new("Taskが見つかりません"))?;
            if record.source_role == role {
                return Ok(());
            }
            let root = record
                .environment
                .as_ref()
                .and_then(|environment| environment.root(role))
                .ok_or_else(|| WorkspaceError::new("このTaskにSubはありません"))?
                .to_owned();
            let next = record
                .other_workspace
                .take()
                .unwrap_or_else(|| WorkspaceSnapshot {
                    root: Some(root.clone()),
                    ..Default::default()
                });
            if next.root.as_deref() != Some(&root) {
                return Err(WorkspaceError::new("Taskの復元先が一致しません"));
            }
            record.other_workspace = Some(editor.snapshot());
            editor.restore_snapshot(next)?;
            record.workspace = editor.snapshot();
            record.source_role = role;
            std::mem::swap(&mut record.terminals, &mut record.other_terminals);
            std::mem::swap(&mut record.selected_execution, &mut record.other_execution);
            session.save(editor)
        })
    }
}
