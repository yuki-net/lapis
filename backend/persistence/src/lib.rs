//! 永続化の具体実装。機能側の Repository 契約を実装する。

use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
};

use lapis_app_services::{ConversationRecord, ConversationRepository};
use lapis_editor_core::ConversationId;
use lapis_settings::{GlobalSettings, GlobalSettingsRepository, SettingsError};
use lapis_workspace::WorkspaceError;
use serde::{Deserialize, Serialize};

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
    fn load(&self) -> Result<(Vec<ConversationRecord>, Option<ConversationId>), WorkspaceError> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok((Vec::new(), None));
            }
            Err(error) => return Err(WorkspaceError::new(error.to_string())),
        };
        let stored: StoredConversations = serde_json::from_slice(&bytes)
            .map_err(|error| WorkspaceError::new(error.to_string()))?;
        if stored.version != 1 {
            return Err(WorkspaceError::new("未対応の Conversation 復元形式です"));
        }
        Ok((stored.records, Some(stored.active)))
    }

    fn save(
        &self,
        records: &[ConversationRecord],
        active: &ConversationId,
    ) -> Result<(), WorkspaceError> {
        atomic_json_write(
            &self.path,
            &StoredConversations {
                version: 1,
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
    use lapis_app_services::{ConversationViewState, RestoredTerminal};
    use lapis_editor_core::ExecutionId;
    use lapis_localization::LocaleId;
    use lapis_settings::GlobalSettingsRepository;
    use lapis_terminal::TerminalStatus;
    use lapis_workspace::{DocumentViewState, WorkspaceSnapshot};

    use super::*;

    #[test]
    fn global_settings_repository_round_trips_locale() {
        let directory = tempfile::tempdir().unwrap();
        let repository = LocalGlobalSettingsRepository::new(directory.path().join("settings.json"));
        let settings = GlobalSettings {
            version: 1,
            locale: LocaleId::new("ja-JP"),
            theme: "lapis.white".to_owned(),
        };

        repository.save(&settings).unwrap();

        assert_eq!(repository.load().unwrap(), settings);
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
            terminals: vec![RestoredTerminal {
                cwd: PathBuf::from("C:/作業/lapis"),
                status: TerminalStatus::Running,
                columns: 132,
                rows: 40,
            }],
        }];

        repository.save(&records, &active).unwrap();
        let restored = repository.load().unwrap();
        assert_eq!(restored, (records, Some(active)));
    }
}
