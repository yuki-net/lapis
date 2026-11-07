//! Workspace の request 処理。

use super::*;

impl BackendState {
    pub(super) fn workspace_list(
        &self,
        session: &BackendSession,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        if session.list_user_workspaces {
            let mut workspaces: Vec<_> = self
                .workspaces
                .values()
                .map(|workspace| workspace.summary.clone())
                .collect();
            workspaces
                .sort_by(|left, right| left.workspace_id.as_str().cmp(right.workspace_id.as_str()));
            return Ok((
                ResponseBody::WorkspaceList(WorkspaceListResponse { workspaces }),
                Vec::new(),
            ));
        }
        let workspace = self
            .workspaces
            .get(&session.workspace_id)
            .ok_or(BackendStateError::WorkspaceNotFound)?;
        Ok((
            ResponseBody::WorkspaceList(WorkspaceListResponse {
                workspaces: vec![workspace.summary.clone()],
            }),
            Vec::new(),
        ))
    }

    pub(super) fn workspace_connect(
        &mut self,
        session: &BackendSession,
        workspace_id: WorkspaceId,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        self.require_workspace(session, &workspace_id)?;
        let summary = self
            .workspaces
            .get(&workspace_id)
            .ok_or(BackendStateError::WorkspaceNotFound)?
            .summary
            .clone();
        if self
            .connected_sessions
            .get(&session.session_id)
            .is_some_and(|connected| connected != &workspace_id)
        {
            return Err(BackendStateError::WorkspaceDenied);
        }
        self.connected_sessions
            .insert(session.session_id.clone(), workspace_id);
        Ok((
            ResponseBody::WorkspaceConnect(WorkspaceConnectResponse { workspace: summary }),
            Vec::new(),
        ))
    }

    pub(super) fn workspace_close(
        &mut self,
        session: &BackendSession,
        workspace_id: WorkspaceId,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        self.require_workspace(session, &workspace_id)?;
        self.disconnect(&session.session_id);
        Ok((
            ResponseBody::WorkspaceClose(WorkspaceCloseResponse { workspace_id }),
            Vec::new(),
        ))
    }

    pub(super) fn file_tree(
        &self,
        session: &BackendSession,
        path: Option<WorkspaceRelativePath>,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        let workspace = self.workspace(session)?;
        let directory = workspace.resolver.resolve_directory(path.as_ref())?;
        let mut entries = workspace
            .files
            .list_children(&directory)?
            .into_iter()
            .map(|entry| {
                let relative = match path.as_ref() {
                    Some(parent) => format!("{}/{}", parent.as_str(), entry.name),
                    None => entry.name.clone(),
                };
                Ok(FileTreeEntry {
                    name: entry.name,
                    path: WorkspaceRelativePath::parse(relative)
                        .map_err(|_| crate::PathSecurityError::InvalidEntry)?,
                    kind: match entry.kind {
                        WorkspaceEntryKind::File => FileTreeKind::File,
                        WorkspaceEntryKind::Directory => FileTreeKind::Directory,
                        WorkspaceEntryKind::Symlink => FileTreeKind::Symlink,
                    },
                })
            })
            .collect::<Result<Vec<_>, BackendStateError>>()?;
        entries.sort_by_key(|entry| entry.name.to_lowercase());
        Ok((
            ResponseBody::FileTree(FileTreeResponse {
                workspace_id: session.workspace_id.clone(),
                path,
                entries,
            }),
            Vec::new(),
        ))
    }
}
