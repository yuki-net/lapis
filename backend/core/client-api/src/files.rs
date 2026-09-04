use serde::{Deserialize, Serialize};

use crate::{WorkspaceId, WorkspaceRelativePath};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileTreeKind {
    File,
    Directory,
    Symlink,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileTreeEntry {
    pub name: String,
    pub path: WorkspaceRelativePath,
    pub kind: FileTreeKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileTreeRequest {
    pub workspace_id: WorkspaceId,
    /// NoneはWorkspace root、Someはそのdirectory直下を表す。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<WorkspaceRelativePath>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileTreeResponse {
    pub workspace_id: WorkspaceId,
    /// Requestと同じ対象directory。Workspace rootはNone。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<WorkspaceRelativePath>,
    pub entries: Vec<FileTreeEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_root_is_represented_without_an_empty_relative_path() {
        let response = FileTreeResponse {
            workspace_id: WorkspaceId::try_new("workspace-1").unwrap(),
            path: None,
            entries: vec![FileTreeEntry {
                name: "src".to_owned(),
                path: WorkspaceRelativePath::parse("src").unwrap(),
                kind: FileTreeKind::Directory,
            }],
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(!json.contains(r#""path":"""#));
        assert_eq!(
            serde_json::from_str::<FileTreeResponse>(&json).unwrap(),
            response
        );
    }
}
