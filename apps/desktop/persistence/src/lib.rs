//! 永続化の具体実装。機能側の Repository 契約を実装する。

use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
};

use lapis_desktop_client::{
    ConversationRecord, ConversationRepository, ConversationState, Project,
};
use lapis_editor_core::ConversationId;
use lapis_settings::{GlobalSettings, GlobalSettingsRepository, SettingsError};
use lapis_workspace::WorkspaceError;
use serde::{Deserialize, Serialize};

mod workspace;
pub use workspace::LocalWorkspaceStateRepository;

#[derive(Clone)]
pub struct LocalConversationRepository {
    path: PathBuf,
}

#[derive(Clone)]
pub struct LocalGlobalSettingsRepository {
    path: PathBuf,
}

impl LocalGlobalSettingsRepository {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn user_default() -> Self {
        let base = env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(env::temp_dir)
            .join("Lapis");
        Self::new(base.join("settings-v1.json"))
    }
}

impl GlobalSettingsRepository for LocalGlobalSettingsRepository {
    fn load(&self) -> Result<GlobalSettings, SettingsError> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(GlobalSettings::default());
            }
            Err(error) => return Err(SettingsError::new(error.to_string())),
        };
        let settings: GlobalSettings = serde_json::from_slice(&bytes)
            .map_err(|error| SettingsError::new(error.to_string()))?;
        if settings.version != 1 {
            return Err(SettingsError::new("Unsupported global settings version"));
        }
        Ok(settings)
    }

    fn save(&self, settings: &GlobalSettings) -> Result<(), SettingsError> {
        atomic_json_write(&self.path, settings)
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
        let stored: StoredConversations = serde_json::from_slice(&bytes)
            .map_err(|error| WorkspaceError::new(error.to_string()))?;
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
    use lapis_desktop_client::{
        ConversationViewState, DocumentViewState, RestoredTerminal, WorkspaceSnapshot,
    };
    use lapis_editor_core::ExecutionId;
    use lapis_localization::LocaleId;
    use lapis_settings::GlobalSettingsRepository;
    use lapis_terminal::TerminalStatus;

    use super::*;

    #[test]
    fn global_settings_repository_round_trips_locale() {
        let directory = tempfile::tempdir().unwrap();
        let repository = LocalGlobalSettingsRepository::new(directory.path().join("settings.json"));
        let settings = GlobalSettings {
            version: 1,
            locale: LocaleId::new("ja-JP"),
            theme: "lapis.white".to_owned(),
            syntax_theme: Some(lapis_settings::SyntaxTheme::Zed),
        };

        repository.save(&settings).unwrap();

        assert_eq!(repository.load().unwrap(), settings);
    }

    #[test]
    fn syntax_family_survives_ui_changes_and_restart() {
        use lapis_desktop_client::SettingsSession;
        use lapis_settings::SyntaxTheme;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings-v1.json");
        fs::write(
            &path,
            r#"{"version":1,"locale":"ja-JP","theme":"lapis.dark","syntax_theme":"zed.one-dark"}"#,
        )
        .unwrap();
        let session = SettingsSession::load(std::sync::Arc::new(
            LocalGlobalSettingsRepository::new(path.clone()),
        ))
        .unwrap();
        assert_eq!(session.settings().syntax_theme, Some(SyntaxTheme::Zed));
        session.set_theme("lapis.white".into()).unwrap();
        let restarted = SettingsSession::load(std::sync::Arc::new(
            LocalGlobalSettingsRepository::new(path.clone()),
        ))
        .unwrap();
        assert_eq!(restarted.settings().theme, "lapis.white");
        assert_eq!(restarted.settings().syntax_theme, Some(SyntaxTheme::Zed));
        let stored: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(stored["syntax_theme"], "zed");

        for family in [SyntaxTheme::Intellij, SyntaxTheme::IntellijClassic] {
            session.set_syntax_theme(family).unwrap();
            let restarted = SettingsSession::load(std::sync::Arc::new(
                LocalGlobalSettingsRepository::new(directory.path().join("settings-v1.json")),
            ))
            .unwrap();
            assert_eq!(restarted.settings().syntax_theme, Some(family));
            assert_eq!(restarted.settings().theme, "lapis.white");
        }
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
                active_tool: "search".to_owned(),
                bottom_panel: Some("terminal".to_owned()),
                ..ConversationViewState::default()
            },
            selected_execution: Some(ExecutionId::new("execution-1")),
            environment: None,
            source_role: Default::default(),
            other_workspace: None,
            other_terminals: Vec::new(),
            other_execution: None,
            terminals: vec![RestoredTerminal {
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
}
