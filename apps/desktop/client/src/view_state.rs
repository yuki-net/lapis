use std::path::PathBuf;

use lapis_editor_core::DocumentId;
use lapis_workspace::WorkspaceError;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DocumentViewState {
    pub document_id: Option<DocumentId>,
    pub path: PathBuf,
    pub cursor_char: usize,
    pub selection_start: usize,
    pub selection_end: usize,
    pub scroll_x: f32,
    pub scroll_y: f32,
    /// 未保存内容がある場合だけ保存する復元用スナップショット。
    pub draft_content: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceSnapshot {
    pub root: Option<PathBuf>,
    pub open_documents: Vec<DocumentViewState>,
    pub active_path: Option<PathBuf>,
    #[serde(default)]
    pub active_document_id: Option<DocumentId>,
}

pub trait WorkspaceStateRepository: Send + Sync {
    fn load(&self) -> Result<Option<WorkspaceSnapshot>, WorkspaceError>;
    fn save(&self, snapshot: &WorkspaceSnapshot) -> Result<(), WorkspaceError>;
}
