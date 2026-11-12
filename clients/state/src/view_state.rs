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

/// クライアントがローカルに保持する Workspace の画面状態。
/// Host と同期する `client_api::WorkspaceSnapshot` とは別の概念。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceViewState {
    pub root: Option<PathBuf>,
    pub open_documents: Vec<DocumentViewState>,
    pub active_path: Option<PathBuf>,
    #[serde(default)]
    pub active_document_id: Option<DocumentId>,
}

impl WorkspaceViewState {
    /// Session 共有用の view を作る。Client 固有の絶対 path と未保存内容は含めない。
    pub fn to_shared_view(&self) -> Result<workbench::WorkspaceView, WorkspaceError> {
        let root = self
            .root
            .as_ref()
            .ok_or_else(|| WorkspaceError::new("Workspaceがありません"))?;
        validate_root(root)?;
        self.shared_view(root)
    }

    /// タブ構成とアクティブ文書だけを共有する。
    ///
    /// カーソル・選択・スクロールは共有しない。共有範囲をタブ構成に限る Client
    /// （Web）はこちらを使い、位置の値を送らない。
    pub fn to_shared_tabs(&self) -> Result<workbench::WorkspaceView, WorkspaceError> {
        let root = self
            .root
            .as_ref()
            .ok_or_else(|| WorkspaceError::new("Workspaceがありません"))?;
        validate_root(root)?;
        let mut view = self.shared_view(root)?;
        for document in &mut view.open_documents {
            *document = workbench::WorkspaceDocumentView {
                document_id: document.document_id.clone(),
                path: document.path.clone(),
                ..Default::default()
            };
        }
        Ok(view)
    }

    fn shared_view(
        &self,
        root: &std::path::Path,
    ) -> Result<workbench::WorkspaceView, WorkspaceError> {
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
                Ok(workbench::WorkspaceDocumentView {
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
                })
            })
            .collect::<Result<Vec<_>, WorkspaceError>>()?;
        Ok(workbench::WorkspaceView {
            open_documents: documents,
            active_path: self.active_path.as_deref().map(relative).transpose()?,
            active_document_id: self.active_document_id.clone(),
        })
    }

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

/// 正規化された root かを検証する。
///
/// 絶対 path の判定は、wasm では OS の path 意味論が無く `is_absolute` が常に偽に
/// なるため、Web の仮想 root（`/Lapis Host` など）は root を持つことで判定する。
fn validate_root(root: &std::path::Path) -> Result<(), WorkspaceError> {
    let absolute = if cfg!(target_family = "wasm") {
        root.has_root()
    } else {
        root.is_absolute()
    };
    if !absolute
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
    fn load(&self) -> Result<Option<WorkspaceViewState>, WorkspaceError>;
    fn save(&self, snapshot: &WorkspaceViewState) -> Result<(), WorkspaceError>;
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
        let snapshot = WorkspaceViewState {
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
            WorkspaceViewState::from_shared(&shared, &workspace, second_root.clone()).unwrap();
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
    fn shared_tabs_omit_cursor_selection_and_scroll_but_keep_order_and_active() {
        let root = std::env::temp_dir().join("lapis-tabs-root");
        let file = root.join("src").join("main.rs");
        let snapshot = WorkspaceViewState {
            root: Some(root),
            open_documents: vec![
                DocumentViewState {
                    document_id: Some(DocumentId::new("document-1")),
                    path: file.clone(),
                    cursor_char: 12,
                    selection_start: 3,
                    selection_end: 12,
                    scroll_x: 4.0,
                    scroll_y: 80.0,
                    ..Default::default()
                },
                DocumentViewState {
                    document_id: Some(DocumentId::new("document-2")),
                    ..Default::default()
                },
            ],
            active_path: Some(file),
            active_document_id: Some(DocumentId::new("document-1")),
        };
        let view = snapshot.to_shared_tabs().unwrap();
        assert_eq!(
            view.open_documents
                .iter()
                .map(|document| document.document_id.clone())
                .collect::<Vec<_>>(),
            vec![
                Some(DocumentId::new("document-1")),
                Some(DocumentId::new("document-2"))
            ]
        );
        assert_eq!(view.active_document_id, Some(DocumentId::new("document-1")));
        // 位置は送らない。タブ構成とアクティブ文書だけを共有する。
        for document in &view.open_documents {
            assert_eq!(document.cursor_char, 0);
            assert_eq!(document.selection_start, 0);
            assert_eq!(document.selection_end, 0);
            assert_eq!(document.scroll_x, 0.0);
            assert_eq!(document.scroll_y, 0.0);
        }
    }

    #[test]
    fn shared_workspace_rejects_foreign_paths_and_ids_without_changing_original() {
        let root = std::env::temp_dir().join("lapis-shared-root");
        let snapshot = WorkspaceViewState {
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
        let empty = WorkspaceViewState {
            root: Some(std::env::temp_dir().join("other")),
            ..Default::default()
        };
        let shared = empty.to_shared(WorkspaceId::new("workspace-1")).unwrap();
        assert!(
            WorkspaceViewState::from_shared(
                &shared,
                &WorkspaceId::new("workspace-2"),
                std::env::temp_dir()
            )
            .is_err()
        );
        assert!(
            WorkspaceViewState::from_shared(
                &shared,
                &WorkspaceId::new("workspace-1"),
                std::env::temp_dir().join("a").join("..").join("b"),
            )
            .is_err()
        );
    }
}
