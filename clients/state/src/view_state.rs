use std::path::PathBuf;

use editor_core::{DocumentId, WorkspaceId};
use serde::{Deserialize, Serialize};
use workbench::{SharedDocumentView, SharedWorkspaceView};
use workspace::WorkspaceError;

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

impl WorkspaceSnapshot {
    /// 既存のローカル保存値をHost共有用に変換する。失敗時も元の値は変更しない。
    pub fn to_shared(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<SharedWorkspaceView, WorkspaceError> {
        let root = self
            .root
            .as_ref()
            .ok_or_else(|| WorkspaceError::new("Workspaceがありません"))?;
        validate_root(root)?;
        let relative =
            |path: &std::path::Path| -> Result<workspace::WorkspaceRelativePath, WorkspaceError> {
                let path = path
                    .strip_prefix(root)
                    .map_err(|_| WorkspaceError::new("Workspace外のpathは共有できません"))?;
                workspace::WorkspaceRelativePath::from_path(path)
                    .map_err(|error| WorkspaceError::new(error.to_string()))
            };
        let documents = self
            .open_documents
            .iter()
            .map(|view| {
                Ok(SharedDocumentView {
                    document_id: view.document_id.clone(),
                    path: (!view.path.as_os_str().is_empty())
                        .then(|| relative(&view.path))
                        .transpose()?,
                    cursor_char: u64::try_from(view.cursor_char)
                        .map_err(|_| WorkspaceError::new("カーソル位置が範囲外です"))?,
                    selection_start: u64::try_from(view.selection_start)
                        .map_err(|_| WorkspaceError::new("選択位置が範囲外です"))?,
                    selection_end: u64::try_from(view.selection_end)
                        .map_err(|_| WorkspaceError::new("選択位置が範囲外です"))?,
                    scroll_x: view.scroll_x,
                    scroll_y: view.scroll_y,
                    draft_content: view.draft_content.clone(),
                })
            })
            .collect::<Result<Vec<_>, WorkspaceError>>()?;
        Ok(SharedWorkspaceView {
            workspace_id,
            open_documents: documents,
            active_path: self.active_path.as_deref().map(relative).transpose()?,
            active_document_id: self.active_document_id.clone(),
        })
    }

    /// Hostの相対pathをこのClientのWorkspace名前空間へ復元する。
    pub fn from_shared(
        shared: &SharedWorkspaceView,
        expected: &WorkspaceId,
        root: PathBuf,
    ) -> Result<Self, WorkspaceError> {
        if &shared.workspace_id != expected {
            return Err(WorkspaceError::new("Workspace IDが一致しません"));
        }
        validate_root(&root)?;
        let documents = shared
            .open_documents
            .iter()
            .map(|view| {
                Ok(DocumentViewState {
                    document_id: view.document_id.clone(),
                    path: view
                        .path
                        .as_ref()
                        .map(|path| root.join(path.as_str()))
                        .unwrap_or_default(),
                    cursor_char: usize::try_from(view.cursor_char)
                        .map_err(|_| WorkspaceError::new("カーソル位置が範囲外です"))?,
                    selection_start: usize::try_from(view.selection_start)
                        .map_err(|_| WorkspaceError::new("選択位置が範囲外です"))?,
                    selection_end: usize::try_from(view.selection_end)
                        .map_err(|_| WorkspaceError::new("選択位置が範囲外です"))?,
                    scroll_x: view.scroll_x,
                    scroll_y: view.scroll_y,
                    draft_content: view.draft_content.clone(),
                })
            })
            .collect::<Result<Vec<_>, WorkspaceError>>()?;
        Ok(Self {
            active_path: shared
                .active_path
                .as_ref()
                .map(|path| root.join(path.as_str())),
            active_document_id: shared.active_document_id.clone(),
            root: Some(root),
            open_documents: documents,
        })
    }
}

fn validate_root(root: &std::path::Path) -> Result<(), WorkspaceError> {
    if !root.is_absolute()
        || root
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(WorkspaceError::new(
            "Workspaceのpathが正規化された絶対pathではありません",
        ));
    }
    Ok(())
}

pub trait WorkspaceStateRepository: Send + Sync {
    fn load(&self) -> Result<Option<WorkspaceSnapshot>, WorkspaceError>;
    fn save(&self, snapshot: &WorkspaceSnapshot) -> Result<(), WorkspaceError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_workspace_restores_under_a_different_client_root_without_losing_drafts() {
        let first_root = std::env::temp_dir().join("lapis-client-one");
        let second_root = std::env::temp_dir().join("lapis-client-two");
        let workspace = WorkspaceId::new("workspace-1");
        let file = first_root.join("src").join("画面.rs");
        let snapshot = WorkspaceSnapshot {
            root: Some(first_root.clone()),
            open_documents: vec![
                DocumentViewState {
                    document_id: Some(DocumentId::new("document-1")),
                    path: file.clone(),
                    cursor_char: 7,
                    selection_start: 2,
                    selection_end: 7,
                    scroll_x: 1.0,
                    scroll_y: 2.0,
                    draft_content: Some("未保存😀".into()),
                },
                DocumentViewState {
                    draft_content: Some("untitled".into()),
                    ..Default::default()
                },
            ],
            active_path: Some(file),
            active_document_id: Some(DocumentId::new("document-1")),
        };
        let shared = snapshot.to_shared(workspace.clone()).unwrap();
        assert_eq!(
            shared.open_documents[0].path.as_ref().unwrap().as_str(),
            "src/画面.rs"
        );
        assert!(shared.open_documents[1].path.is_none());
        let restored =
            WorkspaceSnapshot::from_shared(&shared, &workspace, second_root.clone()).unwrap();
        assert_eq!(
            restored.open_documents[0].path,
            second_root.join("src").join("画面.rs")
        );
        assert_eq!(
            restored.open_documents[0].draft_content.as_deref(),
            Some("未保存😀")
        );
        assert_eq!(
            restored.open_documents[1].draft_content.as_deref(),
            Some("untitled")
        );
        assert_eq!(
            restored.active_path,
            Some(second_root.join("src").join("画面.rs"))
        );
        assert_eq!(snapshot.root, Some(first_root));
    }

    #[test]
    fn shared_workspace_rejects_foreign_paths_and_ids_without_changing_original() {
        let root = std::env::temp_dir().join("lapis-shared-root");
        let snapshot = WorkspaceSnapshot {
            root: Some(root),
            open_documents: vec![DocumentViewState {
                path: std::env::temp_dir().join("other").join("draft.rs"),
                draft_content: Some("keep me".into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let original = snapshot.clone();
        assert!(snapshot.to_shared(WorkspaceId::new("workspace-1")).is_err());
        assert_eq!(snapshot, original);
        let empty = WorkspaceSnapshot {
            root: Some(std::env::temp_dir().join("other")),
            ..Default::default()
        };
        let shared = empty.to_shared(WorkspaceId::new("workspace-1")).unwrap();
        assert!(
            WorkspaceSnapshot::from_shared(
                &shared,
                &WorkspaceId::new("workspace-2"),
                std::env::temp_dir()
            )
            .is_err()
        );
        assert!(
            WorkspaceSnapshot::from_shared(
                &shared,
                &WorkspaceId::new("workspace-1"),
                std::env::temp_dir().join("a").join("..").join("b"),
            )
            .is_err()
        );
    }
}
