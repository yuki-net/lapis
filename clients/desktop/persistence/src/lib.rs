//! 永続化の具体実装。機能側の Repository 契約を実装する。

use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
};

use ::workspace::WorkspaceError;
use desktop_client::{
    ConversationRecord, ConversationRepository, ConversationState, ConversationViewState, Project,
    WorkbenchSync, WorkbenchSyncResult, WorkbenchWatch, WorkspaceSnapshot, ZoneLayoutViewState,
};
use editor_core::{ConversationId, WorkspaceId};
use serde::{Deserialize, Serialize};
use settings::{
    DOCUMENT_VERSION, LEGACY_DOCUMENT_VERSION, LegacyGlobalSettings, SettingsDocument,
    SettingsError, SettingsRepository, SettingsScope,
};
use std::sync::{Arc, Mutex};
use workbench::WorkbenchConnection;

mod workspace;
pub use workspace::LocalWorkspaceStateRepository;

#[derive(Clone)]
pub struct LocalConversationRepository {
    path: PathBuf,
}

#[derive(Clone)]
pub struct LocalUserSettingsRepository {
    /// 現在の保存先。
    path: PathBuf,
    /// 旧形式の保存先。存在すれば読み替えの入力にする。
    legacy_path: Option<PathBuf>,
}

impl LocalUserSettingsRepository {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            legacy_path: None,
        }
    }

    /// 旧形式のファイルも読み替え対象にする。
    pub fn with_legacy(path: PathBuf, legacy_path: PathBuf) -> Self {
        Self {
            path,
            legacy_path: Some(legacy_path),
        }
    }

    /// 状態ディレクトリからユーザー設定の保存先を組み立てる。
    pub fn in_directory(directory: &Path) -> Self {
        Self::with_legacy(
            directory.join("settings.json"),
            directory.join("settings-v1.json"),
        )
    }

    pub fn user_default() -> Self {
        let base = env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(env::temp_dir)
            .join("Lapis");
        Self::in_directory(&base)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// version を見て、現在形式か旧形式かを振り分ける。
    fn decode(&self, bytes: &[u8]) -> Result<SettingsDocument, SettingsError> {
        let version = serde_json::from_slice::<serde_json::Value>(bytes)
            .map_err(|error| SettingsError::new(error.to_string()))?
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(u64::from(LEGACY_DOCUMENT_VERSION));
        let document = match u32::try_from(version).ok() {
            Some(DOCUMENT_VERSION) => serde_json::from_slice::<SettingsDocument>(bytes)
                .map_err(|error| SettingsError::new(error.to_string()))?,
            Some(LEGACY_DOCUMENT_VERSION) => {
                let legacy: LegacyGlobalSettings = serde_json::from_slice(bytes)
                    .map_err(|error| SettingsError::new(error.to_string()))?;
                SettingsDocument::from_legacy(legacy)
            }
            _ => {
                return Err(SettingsError::new(format!(
                    "未対応の設定ファイルversionです: {version}"
                )));
            }
        };
        if document.scope() != &SettingsScope::User {
            return Err(SettingsError::new(
                "ユーザー設定ファイルのスコープが一致しません",
            ));
        }
        Ok(document)
    }
}

impl SettingsRepository for LocalUserSettingsRepository {
    fn load(&self) -> Result<SettingsDocument, SettingsError> {
        let read = |path: &Path| -> Result<Option<Vec<u8>>, SettingsError> {
            match fs::read(path) {
                Ok(bytes) => Ok(Some(bytes)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(SettingsError::new(error.to_string())),
            }
        };
        // 現在形式を優先し、無ければ旧形式から移行する。
        let bytes = match read(&self.path)? {
            Some(bytes) => Some(bytes),
            None => match &self.legacy_path {
                Some(path) => read(path)?,
                None => None,
            },
        };
        match bytes {
            Some(bytes) => self.decode(&bytes),
            None => Ok(SettingsDocument::empty(SettingsScope::User)),
        }
    }

    fn save(&self, document: &SettingsDocument) -> Result<(), SettingsError> {
        atomic_json_write(&self.path, document)
            .map_err(|error| SettingsError::new(error.to_string()))
    }
}

#[derive(Serialize, Deserialize)]
struct StoredConversations {
    version: u32,
    #[serde(default)]
    projects: Vec<Project>,
    records: Vec<ConversationRecord>,
    active: ConversationId,
}

fn decode_conversations(bytes: &[u8]) -> Result<StoredConversations, serde_json::Error> {
    let mut value: serde_json::Value = serde_json::from_slice(bytes)?;
    if let Some(records) = value
        .get_mut("records")
        .and_then(serde_json::Value::as_array_mut)
    {
        for record in records {
            let Some(view) = record
                .get_mut("view")
                .and_then(serde_json::Value::as_object_mut)
            else {
                continue;
            };
            discard_invalid_layout(view);
            if let Some(workbench) = view
                .get_mut("workbench")
                .and_then(serde_json::Value::as_object_mut)
            {
                if let Some(layout) = workbench
                    .get_mut("ide_layout")
                    .and_then(serde_json::Value::as_object_mut)
                {
                    discard_invalid_layout(layout);
                }
                if let Some(layouts) = workbench
                    .get_mut("task_layouts")
                    .and_then(serde_json::Value::as_object_mut)
                {
                    for layout in layouts
                        .values_mut()
                        .filter_map(serde_json::Value::as_object_mut)
                    {
                        discard_invalid_layout(layout);
                    }
                }
            }
        }
    }
    serde_json::from_value(value)
}

// ????????Task?Document?Workbench?????????
fn discard_invalid_layout(view: &mut serde_json::Map<String, serde_json::Value>) {
    if let Some(layout) = view.get("layout")
        && !layout.is_null()
        && !serde_json::from_value::<ZoneLayoutViewState>(layout.clone())
            .is_ok_and(|layout| layout.version == desktop_client::ZONE_LAYOUT_VERSION)
    {
        view.remove("layout");
    }
}

impl LocalConversationRepository {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn user_default() -> Self {
        let base = env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(env::temp_dir)
            .join("Lapis");
        Self::new(base.join("conversations-v1.json"))
    }
}

impl ConversationRepository for LocalConversationRepository {
    fn load(&self) -> Result<ConversationState, WorkspaceError> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Default::default());
            }
            Err(error) => return Err(WorkspaceError::new(error.to_string())),
        };
        let stored =
            decode_conversations(&bytes).map_err(|error| WorkspaceError::new(error.to_string()))?;
        if stored.version != 1 {
            return Err(WorkspaceError::new("未対応の Conversation 復元形式です"));
        }
        Ok((stored.projects, stored.records, Some(stored.active)))
    }

    fn save(
        &self,
        expected: &ConversationState,
        projects: &[Project],
        records: &[ConversationRecord],
        active: &ConversationId,
    ) -> Result<(), WorkspaceError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| WorkspaceError::new(error.to_string()))?;
        }
        let mut lock_path = self.path.as_os_str().to_owned();
        lock_path.push(".lock");
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)
            .map_err(|error| WorkspaceError::new(error.to_string()))?;
        lock.lock()
            .map_err(|error| WorkspaceError::new(error.to_string()))?;
        if &self.load()? != expected {
            return Err(WorkspaceError::new(
                "別のWindowがProject/Taskを更新しました。下書きを保持したまま、Windowを開き直してください",
            ));
        }
        atomic_json_write(
            &self.path,
            &StoredConversations {
                version: 1,
                projects: projects.to_vec(),
                records: records.to_vec(),
                active: active.clone(),
            },
        )
    }
}

/// ローカル保存に加えて、Desktop の Workbench を Host へ公開する。
pub struct HostConversationRepository {
    local: Arc<dyn ConversationRepository>,
    sync: Arc<Mutex<WorkbenchSync>>,
    workspace: WorkspaceId,
}

impl HostConversationRepository {
    pub fn new(
        local: Arc<dyn ConversationRepository>,
        connection: Arc<dyn WorkbenchConnection>,
        workspace: WorkspaceId,
        watch: WorkbenchWatch,
    ) -> Self {
        let mut sync = WorkbenchSync::new(connection.clone());
        let opened = match sync.open(workspace.clone()) {
            Ok(_) => {
                if let Some(state) = sync.confirmed()
                    && let Some(view) = state.view.clone()
                {
                    watch.publish(state.revision, view);
                }
                true
            }
            Err(error) => {
                eprintln!("HostのWorkbenchを開けません: {error}");
                false
            }
        };
        let sync = Arc::new(Mutex::new(sync));
        if opened {
            let subscription_watch = watch;
            let subscription_workspace = workspace.clone();
            let subscription_sync = sync.clone();
            std::thread::spawn(move || {
                let mut subscription = match connection.subscribe(&subscription_workspace) {
                    Ok(subscription) => subscription,
                    Err(error) => {
                        eprintln!("Workbench購読を開始できません: {error}");
                        return;
                    }
                };
                // 購読開始前に取りこぼした変更を一度だけ取り込み、以降は通知だけで動く。
                if !refresh_and_publish(&subscription_sync, &subscription_watch) {
                    return;
                }
                loop {
                    match subscription.recv_timeout(std::time::Duration::from_secs(30)) {
                        Ok(Some(_)) => {}
                        Ok(None) => continue,
                        Err(error) => {
                            eprintln!("Workbench購読が終了しました: {error}");
                            return;
                        }
                    }
                    if !refresh_and_publish(&subscription_sync, &subscription_watch) {
                        return;
                    }
                }
            });
        }
        Self {
            local,
            sync,
            workspace,
        }
    }

    fn shared_view(
        &self,
        view: &ConversationViewState,
        workspace: &WorkspaceSnapshot,
    ) -> Option<ConversationViewState> {
        let shared = workspace.to_shared(self.workspace.clone()).ok()?;
        // Hostの共有状態にはClient固有のアクティブタブを残さない。
        // ローカル保存は元のviewを使うため、自Clientの復元用には残る。
        Some(ConversationViewState {
            workspace: Some(shared),
            ..view.clone().for_shared_workbench()
        })
    }

    /// 同じ内容の再送でrevisionを進めず、Hostの購読者を不要に起床させない。
    fn publish(&self, view: ConversationViewState) -> Result<(), WorkspaceError> {
        let mut sync = self
            .sync
            .lock()
            .map_err(|_| WorkspaceError::new("Workbench同期のlockに失敗しました"))?;
        if sync.confirmed().and_then(|state| state.view.as_ref()) == Some(&view) {
            return Ok(());
        }
        match sync.save(view) {
            Ok(WorkbenchSyncResult::Updated(_)) | Ok(WorkbenchSyncResult::Conflict(_)) => Ok(()),
            Err(error) => Err(WorkspaceError::new(error)),
        }
    }
}

/// Hostの正本を取り直し、変化があればUIへ公開する。lock異常だけを中断条件にする。
fn refresh_and_publish(sync: &Mutex<WorkbenchSync>, watch: &WorkbenchWatch) -> bool {
    let Ok(mut sync) = sync.lock() else {
        eprintln!("Workbench同期のlockに失敗しました");
        return false;
    };
    match sync.refresh() {
        Ok(Some(state)) => {
            if let Some(view) = state.view.clone() {
                watch.publish(state.revision, view);
            }
        }
        Ok(None) => {}
        Err(error) => eprintln!("Workbenchを再取得できません: {error}"),
    }
    true
}

impl ConversationRepository for HostConversationRepository {
    fn load(&self) -> Result<ConversationState, WorkspaceError> {
        self.local.load()
    }

    fn save(
        &self,
        expected: &ConversationState,
        projects: &[Project],
        records: &[ConversationRecord],
        active: &ConversationId,
    ) -> Result<(), WorkspaceError> {
        let record = records
            .iter()
            .find(|record| &record.id == active)
            .or_else(|| records.first());
        if let Some(record) = record
            && let Some(view) = self.shared_view(&record.view, &record.workspace)
        {
            // 反映を保存より先に行い、ディスク保存の完了を他Clientへ待たせない。
            if let Err(error) = self.publish(view) {
                eprintln!("WorkbenchをHostへ通知できません: {error}");
            }
        }
        self.local.save(expected, projects, records, active)?;
        Ok(())
    }

    fn publish_workbench(
        &self,
        view: &ConversationViewState,
        workspace: &WorkspaceSnapshot,
    ) -> Result<(), WorkspaceError> {
        match self.shared_view(view, workspace) {
            Some(view) => self.publish(view),
            None => Ok(()),
        }
    }

    fn accept_workbench_remote(&self) -> Result<Option<ConversationViewState>, WorkspaceError> {
        let mut sync = self
            .sync
            .lock()
            .map_err(|_| WorkspaceError::new("Workbench同期のlockに失敗しました"))?;
        let state = sync.accept_remote().map_err(WorkspaceError::new)?;
        Ok(state.view.map(|mut view| {
            view.workspace = None;
            view
        }))
    }

    fn retry_workbench_local(&self) -> Result<(), WorkspaceError> {
        let mut sync = self
            .sync
            .lock()
            .map_err(|_| WorkspaceError::new("Workbench同期のlockに失敗しました"))?;
        sync.retry_local().map_err(WorkspaceError::new)?;
        Ok(())
    }
}

fn atomic_json_write(path: &Path, value: &impl Serialize) -> Result<(), WorkspaceError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|error| WorkspaceError::new(error.to_string()))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| WorkspaceError::new(error.to_string()))?;
    serde_json::to_writer(&mut temporary, value)
        .map_err(|error| WorkspaceError::new(error.to_string()))?;
    temporary
        .flush()
        .map_err(|error| WorkspaceError::new(error.to_string()))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| WorkspaceError::new(error.to_string()))?;
    temporary
        .persist(path)
        .map_err(|error| WorkspaceError::new(error.error.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use desktop_client::{
        ConversationViewState, DocumentViewState, RestoredTerminal, WorkspaceSnapshot,
    };
    use editor_core::DocumentId;
    use editor_core::ExecutionId;
    use terminal::TerminalStatus;
    use workbench::VersionedViewState;

    use super::*;

    #[test]
    fn user_settings_repository_round_trips_values() {
        let directory = tempfile::tempdir().unwrap();
        let repository = LocalUserSettingsRepository::in_directory(directory.path());
        let mut document = SettingsDocument::empty(SettingsScope::User);
        document.insert(settings::UI_LOCALE, settings::SettingValue::text("ja-JP"));
        document.insert(
            settings::UI_THEME,
            settings::SettingValue::text("lapis.white"),
        );
        document.insert(
            settings::UI_SYNTAX_THEME,
            settings::SettingValue::text("intellij-classic"),
        );
        document.insert(
            settings::TERMINAL_FONT_SIZE,
            settings::SettingValue::integer(15),
        );

        repository.save(&document).unwrap();

        assert_eq!(repository.load().unwrap(), document);
        assert!(directory.path().join("settings.json").is_file());
    }

    #[test]
    fn legacy_settings_file_is_read_and_rewritten_in_the_new_format() {
        use desktop_client::SettingsSession;
        use settings::SyntaxTheme;
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("settings-v1.json"),
            r#"{"version":1,"locale":"ja-JP","theme":"lapis.dark","syntax_theme":"zed.one-dark","terminal_font_size":15}"#,
        )
        .unwrap();
        let repository =
            || std::sync::Arc::new(LocalUserSettingsRepository::in_directory(directory.path()));

        let session = SettingsSession::load(repository()).unwrap();
        assert_eq!(session.settings().syntax_theme(), Some(SyntaxTheme::Zed));
        assert_eq!(session.settings().locale().as_str(), "ja-JP");
        assert_eq!(session.settings().terminal_font_size(), Some(15));

        session.set_theme("lapis.white".into()).unwrap();
        let restarted = SettingsSession::load(repository()).unwrap();
        assert_eq!(restarted.settings().theme(), "lapis.white");
        assert_eq!(restarted.settings().syntax_theme(), Some(SyntaxTheme::Zed));

        let stored: serde_json::Value =
            serde_json::from_slice(&fs::read(directory.path().join("settings.json")).unwrap())
                .unwrap();
        assert_eq!(stored["version"], 2);
        assert_eq!(stored["scope"]["kind"], "user");
        assert_eq!(stored["values"]["ui.syntax-theme"], "zed");
        // 旧ファイルは残したまま読み替える。
        assert!(directory.path().join("settings-v1.json").is_file());
    }

    #[test]
    fn unknown_version_is_rejected_instead_of_silently_reset() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        fs::write(&path, r#"{"version":9,"locale":"ja-JP"}"#).unwrap();
        let repository = LocalUserSettingsRepository::new(path);
        assert!(repository.load().is_err());
    }

    #[test]
    fn conversation_repository_round_trips_active_view_draft_and_terminal_summary() {
        let directory = tempfile::tempdir().unwrap();
        let repository =
            LocalConversationRepository::new(directory.path().join("nested/state.json"));
        let active = ConversationId::new("conversation-日本語");
        let records = vec![ConversationRecord {
            id: active.clone(),
            title: "調査 😀".to_owned(),
            workspace: WorkspaceSnapshot {
                root: Some(PathBuf::from("C:/作業/lapis")),
                active_path: Some(PathBuf::from("C:/作業/lapis/メモ.md")),
                active_document_id: None,
                open_documents: vec![DocumentViewState {
                    path: PathBuf::from("C:/作業/lapis/メモ.md"),
                    cursor_char: 7,
                    draft_content: Some("未保存の内容😀".to_owned()),
                    ..DocumentViewState::default()
                }],
            },
            view: ConversationViewState {
                show_panel_controls: false,
                ..ConversationViewState::default()
            },
            selected_execution: Some(ExecutionId::new("execution-1")),
            environment: None,
            source_role: Default::default(),
            other_workspace: None,
            other_terminals: Vec::new(),
            other_execution: None,
            parked_sources: Vec::new(),
            terminals: vec![RestoredTerminal {
                id: None,
                cwd: PathBuf::from("C:/作業/lapis"),
                status: TerminalStatus::Running,
                columns: 132,
                rows: 40,
            }],
        }];

        repository
            .save(&Default::default(), &[], &records, &active)
            .unwrap();
        let restored = repository.load().unwrap();
        assert_eq!(restored, (Vec::new(), records, Some(active)));

        let mut with_layout = restored.1.clone();
        with_layout[0].view.layout = Some(desktop_client::ZoneLayoutViewState {
            version: desktop_client::ZONE_LAYOUT_VERSION,
            zones: Vec::new(),
            bottom_span_left: false,
            bottom_span_right: false,
        });
        let original = fs::read(&repository.path).unwrap();
        let mut current: serde_json::Value = serde_json::from_slice(&original).unwrap();
        current["records"][0]["view"]["layout"] =
            serde_json::to_value(&with_layout[0].view.layout).unwrap();
        fs::write(&repository.path, serde_json::to_vec(&current).unwrap()).unwrap();
        assert_eq!(repository.load().unwrap().1, with_layout);
        fs::write(&repository.path, original).unwrap();

        let saved: serde_json::Value =
            serde_json::from_slice(&fs::read(&repository.path).unwrap()).unwrap();
        let mut legacy = saved.clone();
        legacy["records"][0]["view"]["layout"] = serde_json::json!({"version":1,"root":{"Tabs":{"group":"group-1"}},"groups":[],"anchors":{}});
        legacy["records"][0]["view"]["panels"] =
            serde_json::json!([{"position":"left-panel","tabs":["old"]}]);
        legacy["records"][0]["view"]["active_tool"] = serde_json::json!("projects");
        fs::write(&repository.path, serde_json::to_vec(&legacy).unwrap()).unwrap();
        assert_eq!(repository.load().unwrap(), restored);
        let mut old_workbench = saved.clone();
        old_workbench["records"][0]["view"]["workbench"]["mode"] = serde_json::json!("agent");
        old_workbench["records"][0]["view"]["workbench"]["ide_layout"] =
            serde_json::json!({"layout":{"version":1}, "panels":[], "show_panel_controls":true});
        old_workbench["records"][0]["view"]["workbench"]["task_layouts"] = serde_json::json!({"task-a":{"layout":{"version":1}, "panels":[], "show_panel_controls":true}});
        fs::write(
            &repository.path,
            serde_json::to_vec(&old_workbench).unwrap(),
        )
        .unwrap();
        let recovered = repository.load().unwrap();
        assert_eq!(recovered.1[0].workspace, restored.1[0].workspace);
        assert_eq!(
            recovered.1[0].view.workbench.mode,
            desktop_client::WorkbenchModeState::Agent
        );
        assert!(
            recovered.1[0]
                .view
                .workbench
                .ide_layout
                .as_ref()
                .unwrap()
                .layout
                .is_none()
        );
        assert!(
            recovered.1[0]
                .view
                .workbench
                .task_layouts
                .values()
                .next()
                .unwrap()
                .layout
                .is_none()
        );
        for invalid in [
            serde_json::json!("invalid"),
            serde_json::json!({}),
            serde_json::json!({"root":{"invalid":0},"groups":[]}),
        ] {
            let mut damaged = saved.clone();
            damaged["records"][0]["view"]["layout"] = invalid;
            fs::write(&repository.path, serde_json::to_vec(&damaged).unwrap()).unwrap();
            assert_eq!(repository.load().unwrap(), restored);
        }
        let mut structural = saved;
        let layout = serde_json::json!({"version":999,"root":{"Tabs":{"group":"missing"}},"groups":[],"anchors":{}});
        structural["records"][0]["view"]["layout"] = layout.clone();
        fs::write(&repository.path, serde_json::to_vec(&structural).unwrap()).unwrap();
        assert_eq!(repository.load().unwrap(), restored);
    }

    #[test]
    fn concurrent_windows_cannot_overwrite_each_others_task_catalog() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("tasks.json");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let results = std::thread::scope(|scope| {
            let handles = ["first", "second"].map(|name| {
                let repository = LocalConversationRepository::new(path.clone());
                let barrier = barrier.clone();
                scope.spawn(move || {
                    let expected = repository.load().unwrap();
                    barrier.wait();
                    let active = ConversationId::new(name);
                    let result = repository.save(&expected, &[], &[], &active);
                    (active, result)
                })
            });
            handles.map(|handle| handle.join().unwrap())
        });
        assert_eq!(
            results.iter().filter(|(_, result)| result.is_ok()).count(),
            1
        );
        let winner = results
            .into_iter()
            .find(|(_, result)| result.is_ok())
            .unwrap()
            .0;
        assert_eq!(
            LocalConversationRepository::new(path).load().unwrap().2,
            Some(winner)
        );
    }

    struct SharedWorkbench {
        state: Mutex<VersionedViewState>,
        subscribers: Mutex<Vec<std::sync::mpsc::Sender<u64>>>,
        subscription_observer: Mutex<Option<std::sync::mpsc::Sender<()>>>,
    }

    impl Default for SharedWorkbench {
        fn default() -> Self {
            Self {
                state: Mutex::new(VersionedViewState::default()),
                subscribers: Mutex::new(Vec::new()),
                subscription_observer: Mutex::new(None),
            }
        }
    }

    impl workbench::WorkbenchConnection for SharedWorkbench {
        fn get(&self, _: &WorkspaceId) -> Result<VersionedViewState, String> {
            Ok(self.state.lock().unwrap().clone())
        }

        fn update(
            &self,
            _: &WorkspaceId,
            expected: u64,
            view: ConversationViewState,
        ) -> Result<workbench::WorkbenchUpdate, String> {
            let result = {
                let mut state = self.state.lock().unwrap();
                match state.update(expected, view) {
                    Ok(_) => Ok(workbench::WorkbenchUpdate::Updated(state.clone())),
                    Err(workbench::ViewUpdateError::Conflict { .. }) => {
                        Ok(workbench::WorkbenchUpdate::Conflict(state.clone()))
                    }
                    Err(_) => Err("overflow".into()),
                }
            };
            if let Ok(workbench::WorkbenchUpdate::Updated(state)) = &result {
                self.subscribers
                    .lock()
                    .unwrap()
                    .retain(|subscriber| subscriber.send(state.revision).is_ok());
            }
            result
        }

        fn subscribe(
            &self,
            _: &WorkspaceId,
        ) -> Result<Box<dyn workbench::WorkbenchSubscription>, String> {
            let (sender, receiver) = std::sync::mpsc::channel();
            self.subscribers.lock().unwrap().push(sender);
            if let Some(observer) = self.subscription_observer.lock().unwrap().take() {
                let _ = observer.send(());
            }
            Ok(Box::new(SharedSubscription(receiver)))
        }
    }

    struct SharedSubscription(std::sync::mpsc::Receiver<u64>);

    impl workbench::WorkbenchSubscription for SharedSubscription {
        fn recv_timeout(&mut self, timeout: std::time::Duration) -> Result<Option<u64>, String> {
            match self.0.recv_timeout(timeout) {
                Ok(revision) => Ok(Some(revision)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Ok(None),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    Err("subscription closed".into())
                }
            }
        }
    }

    fn tabs_view(tabs: &[&str]) -> ConversationViewState {
        use workbench::{TabRef, ZoneLayoutViewState, ZonePanelViewState, ZoneViewState};
        let tabs = tabs
            .iter()
            .map(|id| TabRef::Document {
                id: DocumentId::new(*id),
            })
            .collect::<Vec<_>>();
        ConversationViewState {
            layout: Some(ZoneLayoutViewState {
                version: workbench::ZONE_LAYOUT_VERSION,
                zones: vec![ZoneViewState {
                    position: "main".to_owned(),
                    open: true,
                    extent_ratio: 1.0,
                    layout: None,
                    panels: vec![ZonePanelViewState {
                        id: "panel".to_owned(),
                        active_tab: tabs.first().cloned(),
                        tabs,
                        pinned_tabs: Vec::new(),
                        open: true,
                    }],
                }],
                bottom_span_left: false,
                bottom_span_right: false,
            }),
            ..Default::default()
        }
    }

    fn tab_ids(view: &ConversationViewState) -> Vec<&str> {
        use workbench::TabRef;
        view.layout
            .as_ref()
            .and_then(|layout| layout.zones.first())
            .and_then(|zone| zone.panels.first())
            .map(|panel| {
                panel
                    .tabs
                    .iter()
                    .map(|tab| match tab {
                        TabRef::Document { id } => id.as_str(),
                        _ => "",
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn wait_for_workbench(watch: &WorkbenchWatch, previous: u64) -> (u64, ConversationViewState) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            if let Some(found) = watch.take_if_changed(previous) {
                return found;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("Workbenchの変更が公開されませんでした");
    }

    fn conversation_record(root: &Path) -> ConversationRecord {
        ConversationRecord {
            id: ConversationId::new("conversation-1"),
            title: "Web".to_owned(),
            workspace: WorkspaceSnapshot {
                root: Some(root.to_owned()),
                open_documents: Vec::new(),
                active_path: None,
                active_document_id: None,
            },
            view: ConversationViewState::default(),
            selected_execution: None,
            terminals: Vec::new(),
            environment: None,
            source_role: desktop_client::SourceRole::Primary,
            other_workspace: None,
            other_terminals: Vec::new(),
            other_execution: None,
            parked_sources: Vec::new(),
        }
    }

    fn view_with_active_tabs() -> ConversationViewState {
        let mut view = tabs_view(&["doc-a", "doc-b"]);
        let layout = view.layout.clone();
        view.workbench.ide_layout = Some(workbench::WorkbenchLayoutState {
            layout: layout.clone(),
            show_panel_controls: true,
        });
        view.workbench.task_layouts.insert(
            ConversationId::new("task-a"),
            workbench::WorkbenchLayoutState {
                layout,
                show_panel_controls: true,
            },
        );
        view
    }

    fn active_tab_id(view: &ZoneLayoutViewState) -> Option<&str> {
        use workbench::TabRef;
        match view.zones[0].panels[0].active_tab.as_ref() {
            Some(TabRef::Document { id }) => Some(id.as_str()),
            _ => None,
        }
    }

    fn assert_shared_view_has_no_active_tabs(view: &ConversationViewState) {
        let layout = view.layout.as_ref().unwrap();
        assert_eq!(active_tab_id(layout), None);
        assert_eq!(tab_ids(view), ["doc-a", "doc-b"]);
        assert_eq!(
            active_tab_id(
                view.workbench
                    .ide_layout
                    .as_ref()
                    .unwrap()
                    .layout
                    .as_ref()
                    .unwrap()
            ),
            None
        );
        assert_eq!(
            active_tab_id(
                view.workbench.task_layouts[&ConversationId::new("task-a")]
                    .layout
                    .as_ref()
                    .unwrap()
            ),
            None
        );
    }

    #[test]
    fn host_shared_view_excludes_client_active_tabs_while_local_save_keeps_them() {
        let directory = tempfile::tempdir().unwrap();
        let local = Arc::new(LocalConversationRepository::new(
            directory.path().join("conversations.json"),
        ));
        let workbench = Arc::new(SharedWorkbench::default());
        let workspace = WorkspaceId::new("workspace-1");
        let repository = HostConversationRepository::new(
            local.clone(),
            workbench.clone(),
            workspace.clone(),
            WorkbenchWatch::new(),
        );
        let mut record = conversation_record(directory.path());
        record.view = view_with_active_tabs();

        // debounceのsave経路でも共有状態にアクティブタブを残さない。
        repository
            .save(
                &Default::default(),
                &[],
                std::slice::from_ref(&record),
                &record.id,
            )
            .unwrap();
        {
            let state = workbench.state.lock().unwrap();
            let view = state.view.as_ref().unwrap();
            assert_eq!(view.workspace.as_ref().unwrap().workspace_id, workspace);
            assert_shared_view_has_no_active_tabs(view);
        }

        // Repositoryのpublish_workbench経路でも同様。
        let mut changed = record.view.clone();
        changed.show_panel_controls = false;
        repository
            .publish_workbench(&changed, &record.workspace)
            .unwrap();
        assert_shared_view_has_no_active_tabs(
            workbench.state.lock().unwrap().view.as_ref().unwrap(),
        );

        // ローカル保存は自Clientの復元用なのでアクティブタブを残す。
        let restored = local.load().unwrap();
        let restored = &restored.1[0].view;
        assert_eq!(
            active_tab_id(restored.layout.as_ref().unwrap()),
            Some("doc-a")
        );
        assert_eq!(
            active_tab_id(
                restored.workbench.task_layouts[&ConversationId::new("task-a")]
                    .layout
                    .as_ref()
                    .unwrap()
            ),
            Some("doc-a")
        );
    }

    #[test]
    fn host_conversation_repository_publishes_the_workbench_view() {
        let directory = tempfile::tempdir().unwrap();
        let local = Arc::new(LocalConversationRepository::new(
            directory.path().join("conversations.json"),
        ));
        let workbench = Arc::new(SharedWorkbench::default());
        let workspace = WorkspaceId::new("workspace-1");
        let repository = HostConversationRepository::new(
            local,
            workbench.clone(),
            workspace.clone(),
            WorkbenchWatch::new(),
        );
        let record = conversation_record(directory.path());
        repository
            .save(
                &Default::default(),
                &[],
                std::slice::from_ref(&record),
                &record.id,
            )
            .unwrap();
        let state = workbench.state.lock().unwrap();
        assert_eq!(state.revision, 1);
        let view = state.view.as_ref().unwrap();
        assert_eq!(view.workspace.as_ref().unwrap().workspace_id, workspace);
    }

    #[test]
    fn workbench_changes_reach_the_host_before_local_persistence() {
        let directory = tempfile::tempdir().unwrap();
        let local_path = directory.path().join("conversations.json");
        let local = Arc::new(LocalConversationRepository::new(local_path.clone()));
        let workbench = Arc::new(SharedWorkbench::default());
        let workspace = WorkspaceId::new("workspace-1");
        let repository = HostConversationRepository::new(
            local,
            workbench.clone(),
            workspace.clone(),
            WorkbenchWatch::new(),
        );
        let record = conversation_record(directory.path());
        let mut view = record.view.clone();
        view.show_panel_controls = false;
        let mut subscriber = workbench.subscribe(&workspace).unwrap();

        repository
            .publish_workbench(&view, &record.workspace)
            .unwrap();

        assert_eq!(
            subscriber
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap(),
            Some(1),
            "ローカル保存前に他のClientへ通知されません"
        );
        assert!(
            !local_path.exists(),
            "通知だけの経路でローカル保存が発生しています"
        );
        let state = workbench.state.lock().unwrap();
        assert_eq!(state.revision, 1);
        assert_eq!(
            state.view.as_ref().map(|view| view.show_panel_controls),
            Some(false)
        );

        // 同じ表示の再通知はrevisionを進めない。
        drop(state);
        repository
            .publish_workbench(&view, &record.workspace)
            .unwrap();
        assert_eq!(workbench.state.lock().unwrap().revision, 1);
    }

    struct ObservingLocal {
        workbench: Arc<SharedWorkbench>,
        revision_during_save: Mutex<Option<u64>>,
    }

    impl ConversationRepository for ObservingLocal {
        fn load(&self) -> Result<ConversationState, WorkspaceError> {
            Ok(Default::default())
        }

        fn save(
            &self,
            _: &ConversationState,
            _: &[Project],
            _: &[ConversationRecord],
            _: &ConversationId,
        ) -> Result<(), WorkspaceError> {
            *self.revision_during_save.lock().unwrap() =
                Some(self.workbench.state.lock().unwrap().revision);
            Ok(())
        }
    }

    #[test]
    fn repository_save_notifies_the_host_before_writing_local_state() {
        let directory = tempfile::tempdir().unwrap();
        let workbench = Arc::new(SharedWorkbench::default());
        let local = Arc::new(ObservingLocal {
            workbench: workbench.clone(),
            revision_during_save: Mutex::new(None),
        });
        let workspace = WorkspaceId::new("workspace-1");
        let repository = HostConversationRepository::new(
            local.clone(),
            workbench,
            workspace,
            WorkbenchWatch::new(),
        );
        let record = conversation_record(directory.path());

        repository
            .save(
                &Default::default(),
                &[],
                std::slice::from_ref(&record),
                &record.id,
            )
            .unwrap();

        assert_eq!(
            *local.revision_during_save.lock().unwrap(),
            Some(1),
            "ローカル保存の開始前にHostへ通知されていません"
        );
    }

    #[test]
    fn remote_tab_changes_are_published_and_advance_the_next_local_save() {
        let directory = tempfile::tempdir().unwrap();
        let local = Arc::new(LocalConversationRepository::new(
            directory.path().join("conversations.json"),
        ));
        let workbench = Arc::new(SharedWorkbench::default());
        let workspace = WorkspaceId::new("workspace-1");
        let watch = WorkbenchWatch::new();
        let (subscribed, subscription) = std::sync::mpsc::channel();
        *workbench.subscription_observer.lock().unwrap() = Some(subscribed);
        let repository = HostConversationRepository::new(
            local,
            workbench.clone(),
            workspace.clone(),
            watch.clone(),
        );
        subscription
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("Workbench購読が開始されませんでした");

        let mut remote = WorkbenchSync::new(workbench.clone());
        remote.open(workspace).unwrap();

        for (expected_revision, tabs) in [
            (1, vec!["doc-a", "doc-b"]),
            (2, vec!["doc-b", "doc-a"]),
            (3, vec!["doc-b"]),
        ] {
            assert!(matches!(
                remote.save(tabs_view(&tabs)).unwrap(),
                desktop_client::WorkbenchSyncResult::Updated(_)
            ));
            let (revision, view) = wait_for_workbench(&watch, expected_revision - 1);
            assert_eq!(revision, expected_revision);
            assert_eq!(tab_ids(&view), tabs);
        }

        // リモート変更を取り込んだrevisionでローカル保存が通り、競合しない。
        let record = conversation_record(directory.path());
        repository
            .save(
                &Default::default(),
                &[],
                std::slice::from_ref(&record),
                &record.id,
            )
            .unwrap();
        assert_eq!(workbench.state.lock().unwrap().revision, 4);
    }
}
