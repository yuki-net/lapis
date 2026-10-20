use crate::BackendHost;
use lapis_backend_state::EmbeddedClient;
use lapis_client_api::{
    FileTreeKind, FileTreeRequest, RequestBody, ResponseBody, WorkspaceRelativePath,
};
use lapis_workspace::{FileEntry, FileEntryKind, WorkspaceError, WorkspaceRepository};
use std::path::Path;

pub(super) struct HostWorkspaces {
    host: BackendHost,
}
impl HostWorkspaces {
    pub fn new(host: BackendHost) -> Self {
        Self { host }
    }

    fn collect(
        client: &EmbeddedClient,
        root: &Path,
        directory: Option<WorkspaceRelativePath>,
        depth: usize,
        output: &mut Vec<FileEntry>,
    ) -> Result<(), WorkspaceError> {
        let response = client
            .request(RequestBody::FileTree(FileTreeRequest {
                workspace_id: client.workspace_id().clone(),
                path: directory,
            }))
            .map_err(problem)?;
        let ResponseBody::FileTree(response) = response else {
            return Err(problem("Workspace tree request rejected"));
        };
        let mut entries = response.entries;
        entries.retain(|entry| !matches!(entry.name.as_str(), ".git" | "target" | ".DS_Store"));
        entries.sort_by(|left, right| {
            let left_dir = matches!(left.kind, FileTreeKind::Directory);
            let right_dir = matches!(right.kind, FileTreeKind::Directory);
            right_dir
                .cmp(&left_dir)
                .then_with(|| left.name.cmp(&right.name))
        });
        for entry in entries {
            let is_directory = matches!(entry.kind, FileTreeKind::Directory);
            output.push(FileEntry {
                path: root.join(entry.path.as_str()),
                relative_path: entry.path.as_str().into(),
                kind: if is_directory {
                    FileEntryKind::Directory
                } else {
                    FileEntryKind::File
                },
                depth,
            });
            if is_directory {
                Self::collect(client, root, Some(entry.path), depth + 1, output)?;
            }
        }
        Ok(())
    }
}
impl WorkspaceRepository for HostWorkspaces {
    fn create_directory(&self, root: &Path, name: &str) -> Result<(), WorkspaceError> {
        let client = self.host.connect(root).map_err(problem)?;
        let path = WorkspaceRelativePath::parse(name).map_err(problem)?;
        let response = client
            .request(RequestBody::CreateDirectory(
                lapis_client_api::CreateDirectoryRequest {
                    workspace_id: client.workspace_id().clone(),
                    path,
                },
            ))
            .map_err(problem)?;
        match response {
            ResponseBody::CreateDirectory(_) => Ok(()),
            ResponseBody::Error(error) => Err(problem(error.code.as_str())),
            _ => Err(problem("Unexpected directory response")),
        }
    }

    fn list_tree(&self, root: &Path) -> Result<Vec<FileEntry>, WorkspaceError> {
        let root = root.canonicalize().map_err(problem)?;
        let client = self.host.connect(&root).map_err(problem)?;
        let mut entries = Vec::new();
        Self::collect(&client, &root, None, 0, &mut entries)?;
        Ok(entries)
    }
}
fn problem(error: impl std::fmt::Display) -> WorkspaceError {
    WorkspaceError::new(error.to_string())
}
