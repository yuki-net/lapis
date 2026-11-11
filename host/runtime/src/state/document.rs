//! Document の request 処理。

use crate::PathSecurityError;

use super::*;

impl BackendState {
    pub(super) fn document_untitled(
        &mut self,
        session: &BackendSession,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        let document_id = self.allocate_document_id()?;
        let workspace = self.workspace_mut(session)?;
        let resource = DocumentResource {
            document_id: document_id.clone(),
            path: None,
            document: Document::new(),
            attached_sessions: HashSet::from([session.session_id.clone()]),
            owner: None,
            last_activity: std::time::Instant::now(),
        };
        let snapshot = resource.snapshot(&workspace.summary.workspace_id);
        workspace.documents.insert(document_id, resource);
        let event = workspace.publish(EventBody::DocumentReplaced {
            document: snapshot.clone(),
        })?;
        Ok((
            ResponseBody::DocumentUntitled(DocumentHistoryResponse { document: snapshot }),
            vec![event],
        ))
    }

    pub(super) fn document_inspect(
        &mut self,
        session: &BackendSession,
        document_id: DocumentId,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        let workspace = self.workspace_mut(session)?;
        {
            let resource = workspace
                .documents
                .get_mut(&document_id)
                .ok_or(BackendStateError::DocumentNotFound)?;
            resource
                .attached_sessions
                .insert(session.session_id.clone());
        }
        let resource = workspace
            .documents
            .get(&document_id)
            .ok_or(BackendStateError::DocumentNotFound)?;
        let snapshot = document_snapshot(workspace, resource)?;
        Ok((
            ResponseBody::DocumentInspect(DocumentHistoryResponse { document: snapshot }),
            Vec::new(),
        ))
    }

    pub(super) fn document_save_as(
        &mut self,
        session: &BackendSession,
        request: client_api::DocumentSaveAsRequest,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        let absolute = self.document_target(&session.workspace_id, &request.path)?;
        let workspace = self.workspace_mut(session)?;
        let path = canonical_document_path(&workspace.resolver, &absolute)?;
        let document = workspace.require_attached_document_mut(session, &request.document_id)?;
        require_revision(
            &request.document_id,
            request.base_revision,
            document.revision(),
        )?;
        let previous = document.path.clone();
        if previous.as_ref() == Some(&path) {
            let (_, events) =
                self.document_save(session, request.document_id.clone(), request.base_revision)?;
            let (response, _) = self.document_inspect(session, request.document_id)?;
            let ResponseBody::DocumentInspect(snapshot) = response else {
                unreachable!()
            };
            return Ok((ResponseBody::DocumentSaveAs(snapshot), events));
        }
        if workspace.documents.contains_path(&absolute) {
            return Err(::document::DocumentError::conflict(
                "保存先は開いている文書と重複しています",
            )
            .into());
        }
        if workspace.files.fingerprint(&absolute)?.is_some() {
            return Err(BackendStateError::Document(
                ::document::DocumentError::conflict("保存先は既に存在します"),
            ));
        }
        let resource = workspace
            .documents
            .get_mut(&request.document_id)
            .ok_or(BackendStateError::DocumentNotFound)?;
        let fingerprint =
            workspace
                .files
                .write_file(&absolute, &resource.document.encoded_bytes(), None)?;
        let previous_absolute = resource.document.path().map(ToOwned::to_owned);
        resource.document.mark_saved(absolute.clone(), fingerprint);
        resource.path = Some(path);
        let snapshot = resource.snapshot(&workspace.summary.workspace_id);
        if let Some(previous) = previous_absolute {
            workspace.documents.remove_path(&previous);
        }
        workspace
            .documents
            .insert_path(absolute, request.document_id);
        let event = workspace.publish(EventBody::DocumentReplaced {
            document: snapshot.clone(),
        })?;
        Ok((
            ResponseBody::DocumentSaveAs(DocumentHistoryResponse { document: snapshot }),
            vec![event],
        ))
    }

    pub(super) fn document_reload(
        &mut self,
        session: &BackendSession,
        document_id: DocumentId,
        base_revision: Revision,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        let workspace = self.workspace_mut(session)?;
        let resource = workspace.require_attached_document_mut(session, &document_id)?;
        require_revision(&document_id, base_revision, resource.revision())?;
        if resource.document.is_dirty() {
            return Err(BackendStateError::Document(
                ::document::DocumentError::conflict("未保存変更がある文書は再読み込みできません"),
            ));
        }
        let path = resource
            .path
            .clone()
            .ok_or(BackendStateError::DocumentNotFound)?;
        let absolute = workspace.resolver.resolve_existing(&path)?;
        let data = workspace.files.read_file(&absolute)?;
        let resource = workspace
            .documents
            .get_mut(&document_id)
            .ok_or(BackendStateError::DocumentNotFound)?;
        resource.document.reload(data)?;
        let snapshot = resource.snapshot(&workspace.summary.workspace_id);
        let event = workspace.publish(EventBody::DocumentReplaced {
            document: snapshot.clone(),
        })?;
        Ok((
            ResponseBody::DocumentReload(DocumentHistoryResponse { document: snapshot }),
            vec![event],
        ))
    }

    pub(super) fn document_discard(
        &mut self,
        session: &BackendSession,
        request: client_api::DocumentDiscardRequest,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        let workspace = self.workspace_mut(session)?;
        let workspace_id = workspace.summary.workspace_id.clone();
        let resource = workspace.require_attached_document_mut(session, &request.document_id)?;
        if resource
            .owner
            .as_ref()
            .is_some_and(|owner| owner != &session.session_id)
        {
            return Err(BackendStateError::DocumentNotOwner);
        }
        require_revision(
            &request.document_id,
            request.base_revision,
            resource.revision(),
        )?;
        resource.touch();
        let previous_path = resource.document.path().map(ToOwned::to_owned);
        let saved_path = resource
            .document
            .saved_fingerprint()
            .and(resource.path.clone());
        let saved = saved_path
            .map(|path| {
                let absolute = workspace.resolver.resolve_existing(&path).map_err(|_| {
                    ::document::DocumentError::conflict(
                        "保存済み内容を読み込めないため未保存変更を保持します",
                    )
                })?;
                workspace.files.read_file(&absolute).map_err(|_| {
                    ::document::DocumentError::conflict(
                        "保存済み内容を読み込めないため未保存変更を保持します",
                    )
                })
            })
            .transpose()?;
        let resource = workspace.require_attached_document_mut(session, &request.document_id)?;
        resource.document.discard(saved)?;
        if resource.document.path().is_none() {
            resource.path = None;
        }
        let snapshot = resource.snapshot(&workspace_id);
        if snapshot.path.is_none()
            && let Some(previous) = previous_path
        {
            workspace.documents.remove_path(&previous);
        }
        let event = workspace.publish(EventBody::DocumentReplaced {
            document: snapshot.clone(),
        })?;
        Ok((
            ResponseBody::DocumentDiscard(client_api::DocumentDiscardResponse {
                document: snapshot,
            }),
            vec![event],
        ))
    }

    pub(super) fn document_target(
        &self,
        workspace_id: &WorkspaceId,
        path: &WorkspaceRelativePath,
    ) -> Result<PathBuf, BackendStateError> {
        let workspace = self
            .workspaces
            .get(workspace_id)
            .ok_or(BackendStateError::WorkspaceNotFound)?;
        let absolute = workspace.resolver.resolve_new_file(path)?;
        if self.workspaces.iter().any(|(owner, workspace)| {
            owner != workspace_id && workspace.documents.contains_path(&absolute)
        }) {
            return Err(::document::DocumentError::conflict(
                "別の Workspace が同じ Document を所有しています",
            )
            .into());
        }
        Ok(absolute)
    }

    pub(super) fn document_open(
        &mut self,
        session: &BackendSession,
        path: WorkspaceRelativePath,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        let allocated_document_id = self.allocate_document_id()?;
        let absolute = self.document_target(&session.workspace_id, &path)?;
        let workspace = self.workspace_mut(session)?;
        let path = canonical_document_path(&workspace.resolver, &absolute)?;
        let existing = workspace.documents.get_by_path(&absolute).cloned();
        if let Some(document_id) = existing {
            let document = workspace
                .documents
                .get_mut(&document_id)
                .ok_or(BackendStateError::DocumentNotFound)?;
            document
                .attached_sessions
                .insert(session.session_id.clone());
            let snapshot = document.snapshot(&workspace.summary.workspace_id);
            return Ok((
                ResponseBody::DocumentOpen(DocumentOpenResponse {
                    document: snapshot,
                    reused: true,
                }),
                Vec::new(),
            ));
        }
        let data = workspace.files.read_file(&absolute)?;
        let document = Document::from_file(absolute.clone(), data)?;
        let document_id = allocated_document_id;
        let mut resource = DocumentResource::new(document_id.clone(), path.clone(), document);
        resource
            .attached_sessions
            .insert(session.session_id.clone());
        let snapshot = resource.snapshot(&workspace.summary.workspace_id);
        workspace
            .documents
            .insert_path(absolute, document_id.clone());
        workspace.documents.insert(document_id, resource);
        let event = workspace.publish(EventBody::DocumentReplaced {
            document: snapshot.clone(),
        })?;
        Ok((
            ResponseBody::DocumentOpen(DocumentOpenResponse {
                document: snapshot,
                reused: false,
            }),
            vec![event],
        ))
    }

    pub(super) fn document_create(
        &mut self,
        session: &BackendSession,
        path: WorkspaceRelativePath,
        encoding: DocumentEncoding,
        content: String,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        let document_id = self.allocate_document_id()?;
        let absolute = self.document_target(&session.workspace_id, &path)?;
        let workspace = self.workspace_mut(session)?;
        let path = canonical_document_path(&workspace.resolver, &absolute)?;
        if workspace.documents.contains_path(&absolute) {
            return Err(::document::DocumentError::conflict(
                "document already exists in backend state",
            )
            .into());
        }
        if workspace.files.fingerprint(&absolute)?.is_some() {
            return Err(BackendStateError::Document(
                ::document::DocumentError::conflict("document already exists on disk"),
            ));
        }
        let document = Document::draft(absolute.clone(), content, encoding.into());
        let mut resource = DocumentResource::new(document_id.clone(), path.clone(), document);
        resource
            .attached_sessions
            .insert(session.session_id.clone());
        let snapshot = resource.snapshot(&workspace.summary.workspace_id);
        workspace
            .documents
            .insert_path(absolute, document_id.clone());
        workspace.documents.insert(document_id, resource);
        let event = workspace.publish(EventBody::DocumentReplaced {
            document: snapshot.clone(),
        })?;
        Ok((
            ResponseBody::DocumentCreate(DocumentCreateResponse { document: snapshot }),
            vec![event],
        ))
    }

    pub(super) fn document_claim(
        &mut self,
        session: &BackendSession,
        request: client_api::DocumentClaimRequest,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        let workspace = self.workspace_mut(session)?;
        let document = workspace.require_attached_document_mut(session, &request.document_id)?;
        document.owner = Some(session.session_id.clone());
        document.touch();
        let event = workspace.publish(EventBody::DocumentOwner {
            document_id: request.document_id,
            owner: Some(session.session_id.clone()),
        })?;
        Ok((
            ResponseBody::DocumentClaim(client_api::DocumentCommandResponse::default()),
            vec![event],
        ))
    }

    pub(super) fn document_release(
        &mut self,
        session: &BackendSession,
        request: client_api::DocumentReleaseRequest,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        let workspace = self.workspace_mut(session)?;
        let document = workspace.require_attached_document_mut(session, &request.document_id)?;
        if document
            .owner
            .as_ref()
            .is_some_and(|owner| owner != &session.session_id)
        {
            return Err(BackendStateError::DocumentNotOwner);
        }
        document.owner = None;
        let event = workspace.publish(EventBody::DocumentOwner {
            document_id: request.document_id,
            owner: None,
        })?;
        Ok((
            ResponseBody::DocumentRelease(client_api::DocumentCommandResponse::default()),
            vec![event],
        ))
    }

    pub(super) fn document_edit(
        &mut self,
        session: &BackendSession,
        request: client_api::DocumentEditRequest,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        let workspace = self.workspace_mut(session)?;
        let document = workspace.require_attached_document_mut(session, &request.document_id)?;
        if document
            .owner
            .as_ref()
            .is_some_and(|owner| owner != &session.session_id)
        {
            return Err(BackendStateError::DocumentNotOwner);
        }
        require_revision(
            &request.document_id,
            request.base_revision,
            document.revision(),
        )?;
        document.touch();
        let edits = request
            .transaction
            .edits()
            .iter()
            .map(|edit| {
                let start = usize::try_from(edit.start_char())
                    .map_err(|_| BackendStateError::CounterOverflow)?;
                let end = usize::try_from(edit.end_char())
                    .map_err(|_| BackendStateError::CounterOverflow)?;
                Ok(TextEdit::new(start..end, edit.replacement()))
            })
            .collect::<Result<Vec<_>, BackendStateError>>()?;
        document.document.apply_transaction(edits)?;
        let revision = document.revision();
        let event_body = EventBody::DocumentEdited {
            document_id: request.document_id.clone(),
            base_revision: request.base_revision,
            revision,
            transaction: request.transaction,
        };
        let event = workspace.publish(event_body)?;
        Ok((
            ResponseBody::DocumentEdit(DocumentEditResponse {
                document_id: request.document_id,
                revision,
            }),
            vec![event],
        ))
    }

    pub(super) fn document_save(
        &mut self,
        session: &BackendSession,
        document_id: DocumentId,
        base_revision: Revision,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        let workspace = self.workspace_mut(session)?;
        let files = workspace.files.clone();
        let relative = workspace
            .require_attached_document_mut(session, &document_id)?
            .path
            .clone()
            .ok_or(BackendStateError::DocumentNotFound)?;
        let path = workspace.resolver.resolve_new_file(&relative)?;
        let document = workspace.require_attached_document_mut(session, &document_id)?;
        require_revision(&document_id, base_revision, document.revision())?;
        let fingerprint = files.write_file(
            &path,
            &document.document.encoded_bytes(),
            document.document.saved_fingerprint(),
        )?;
        document.document.mark_saved(path, fingerprint);
        let revision = document.revision();
        let event = workspace.publish(EventBody::DocumentSaved {
            document_id: document_id.clone(),
            revision,
        })?;
        Ok((
            ResponseBody::DocumentSave(DocumentSaveResponse {
                document_id,
                revision,
            }),
            vec![event],
        ))
    }

    pub(super) fn document_history(
        &mut self,
        session: &BackendSession,
        document_id: DocumentId,
        base_revision: Revision,
        undo: bool,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        let workspace = self.workspace_mut(session)?;
        let workspace_id = workspace.summary.workspace_id.clone();
        let document = workspace.require_attached_document_mut(session, &document_id)?;
        if document
            .owner
            .as_ref()
            .is_some_and(|owner| owner != &session.session_id)
        {
            return Err(BackendStateError::DocumentNotOwner);
        }
        require_revision(&document_id, base_revision, document.revision())?;
        document.touch();
        let changed = if undo {
            document.document.undo()
        } else {
            document.document.redo()
        };
        let snapshot = document.snapshot(&workspace_id);
        let events = if changed {
            vec![workspace.publish(EventBody::DocumentReplaced {
                document: snapshot.clone(),
            })?]
        } else {
            Vec::new()
        };
        let response = DocumentHistoryResponse { document: snapshot };
        Ok((
            if undo {
                ResponseBody::DocumentUndo(response)
            } else {
                ResponseBody::DocumentRedo(response)
            },
            events,
        ))
    }

    pub(super) fn document_close(
        &mut self,
        session: &BackendSession,
        document_id: DocumentId,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        let workspace = self.workspace_mut(session)?;
        let document = workspace.require_attached_document_mut(session, &document_id)?;
        document.attached_sessions.remove(&session.session_id);
        Ok((
            ResponseBody::DocumentClose(DocumentCloseResponse { document_id }),
            Vec::new(),
        ))
    }
}

/// Document の Snapshot を作り、実ファイルとの差分を外部変更として反映する。
pub(super) fn document_snapshot(
    workspace: &WorkspaceState,
    resource: &DocumentResource,
) -> Result<client_api::DocumentSnapshot, BackendStateError> {
    let mut snapshot = resource.snapshot(&workspace.summary.workspace_id);
    let missing_root = std::fs::symlink_metadata(workspace.resolver.root())
        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound);
    let actual = if missing_root {
        Ok(None)
    } else if let Some(path) = &resource.path {
        match workspace.resolver.resolve_scoped_path(path) {
            Ok(absolute) => workspace
                .files
                .metadata(&absolute)
                .map_err(|error| error.to_string()),
            Err(error) if matches!(&error, PathSecurityError::WorkspaceEscape) => {
                return Err(error.into());
            }
            // Workspace 内でも親ディレクトリの置換などで解決できない path は、
            // その文書だけ読取不能として扱い、Snapshot 全体を失敗させない。
            Err(error) => Err(error.to_string()),
        }
    } else {
        Ok(None)
    };
    let external_change = match actual {
        Ok(metadata) => resource.document.external_change_metadata(
            metadata.map(|metadata| (metadata.byte_len, metadata.modified_nanos)),
        ),
        Err(error) => {
            // 読み取りエラーは削除と区別できるよう記録し、復旧自体は継続する。
            eprintln!(
                "Documentの外部変更を確認できません: {}: {error}",
                resource
                    .path
                    .as_ref()
                    .map_or_else(|| "<untitled>".to_owned(), |path| path.as_str().to_owned())
            );
            if resource.document.has_saved_fingerprint() {
                ::document::ExternalChange::Modified
            } else {
                ::document::ExternalChange::Unchanged
            }
        }
    };
    snapshot.external_change = match external_change {
        ::document::ExternalChange::Unchanged => client_api::DocumentExternalChange::Unchanged,
        ::document::ExternalChange::Modified => client_api::DocumentExternalChange::Modified,
        ::document::ExternalChange::Deleted => client_api::DocumentExternalChange::Deleted,
    };
    Ok(snapshot)
}
