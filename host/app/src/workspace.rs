use crate::BackendClient;
use crate::client::Client;
use ::workspace::{
    FileEntry, FileEntryKind, WorkspaceError, WorkspaceIdentity, WorkspaceRepository,
};
use client_api::{
    FileTreeKind, FileTreeRequest, RequestBody, ResponseBody, WorkspaceConnectRequest,
    WorkspaceRelativePath,
};
use std::path::Path;

pub(super) struct HostWorkspaces {
    host: BackendClient,
}
impl HostWorkspaces {
    pub fn new(host: BackendClient) -> Self {
        Self { host }
    }

    fn collect(
        client: &Client,
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
                client_api::CreateDirectoryRequest {
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
        let root = self.host.normalize_path(root).map_err(problem)?;
        let client = self.host.connect(&root).map_err(problem)?;
        let mut entries = Vec::new();
        Self::collect(&client, &root, None, 0, &mut entries)?;
        Ok(entries)
    }

    fn identity(&self, root: &Path) -> Result<Option<WorkspaceIdentity>, WorkspaceError> {
        let root = self.host.normalize_path(root).map_err(problem)?;
        let client = self.host.connect(&root).map_err(problem)?;
        let response = client
            .request(RequestBody::WorkspaceConnect(WorkspaceConnectRequest {
                workspace_id: client.workspace_id().clone(),
            }))
            .map_err(problem)?;
        let identity = match response {
            ResponseBody::WorkspaceConnect(response) => response.identity,
            ResponseBody::Error(error) => return Err(problem(error.code.as_str())),
            _ => return Err(problem("Unexpected workspace identity response")),
        };
        Ok(Some(WorkspaceIdentity {
            id: editor_core::WorkspaceId::from(identity.workspace_id),
            name: identity.name,
            root_name: identity.root_name,
        }))
    }
}
fn problem(error: impl std::fmt::Display) -> WorkspaceError {
    WorkspaceError::new(error.to_string())
}
