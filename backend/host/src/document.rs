use crate::BackendHost;
use lapis_backend_state::EmbeddedClient;
use lapis_client_api as wire;
use lapis_document::{
    Document, DocumentCache, DocumentConnection, DocumentError, Encoding, ExternalChange, Revision,
};
use lapis_editor_core::DocumentId;
use std::{
    collections::HashMap,
    ops::Range,
    path::{Path, PathBuf},
    sync::Mutex,
};

pub(super) struct HostDocuments {
    host: BackendHost,
    resources: Mutex<HashMap<DocumentId, (EmbeddedClient, PathBuf)>>,
}

impl HostDocuments {
    pub(super) fn new(host: BackendHost) -> Self {
        Self {
            host,
            resources: Mutex::new(HashMap::new()),
        }
    }

    fn request(
        client: &EmbeddedClient,
        body: wire::RequestBody,
    ) -> Result<wire::ResponseBody, DocumentError> {
        match client
            .request(body)
            .map_err(|error| DocumentError::io(error.to_string()))?
        {
            wire::ResponseBody::Error(error) => {
                if error.code.as_str() == wire::REVISION_CONFLICT
                    || error.revision_conflict.is_some()
                {
                    Err(DocumentError::conflict(error.code.as_str()))
                } else if error.code.as_str() == "document_conflict" {
                    Err(DocumentError::conflict(
                        "Document が外部変更と競合しています",
                    ))
                } else {
                    Err(DocumentError::io(error.code.as_str()))
                }
            }
            response => Ok(response),
        }
    }

    fn resource(
        &self,
        id: &DocumentId,
    ) -> Result<(EmbeddedClient, PathBuf, wire::DocumentId), DocumentError> {
        let (client, root) = self
            .resources
            .lock()
            .map_err(|_| DocumentError::io("Document 接続 lock に失敗しました"))?
            .get(id)
            .cloned()
            .ok_or_else(|| DocumentError::io("Document 接続がありません"))?;
        let id = wire::DocumentId::try_new(id.as_str())
            .map_err(|_| DocumentError::io("Document ID が不正です"))?;
        Ok((client, root, id))
    }

    fn cache(
        &self,
        client: EmbeddedClient,
        root: PathBuf,
        snapshot: wire::DocumentSnapshot,
    ) -> Result<DocumentCache, DocumentError> {
        let id = DocumentId::new(snapshot.document_id.as_str());
        let path = snapshot.path.map(|path| root.join(path.as_str()));
        let document = Document::from_snapshot(
            path,
            snapshot.content,
            match snapshot.encoding {
                wire::DocumentEncoding::Utf8 => Encoding::Utf8,
                wire::DocumentEncoding::Utf8Bom => Encoding::Utf8Bom,
                wire::DocumentEncoding::Utf16Le => Encoding::Utf16Le,
                wire::DocumentEncoding::Utf16Be => Encoding::Utf16Be,
            },
            Revision::new(snapshot.revision.value()),
            snapshot.dirty,
        );
        self.resources
            .lock()
            .map_err(|_| DocumentError::io("Document 接続 lock に失敗しました"))?
            .insert(id.clone(), (client, root));
        Ok(DocumentCache {
            reused: true,
            id,
            document,
            can_undo: snapshot.can_undo,
            can_redo: snapshot.can_redo,
            external_change: match snapshot.external_change {
                wire::DocumentExternalChange::Unchanged => ExternalChange::Unchanged,
                wire::DocumentExternalChange::Modified => ExternalChange::Modified,
                wire::DocumentExternalChange::Deleted => ExternalChange::Deleted,
            },
        })
    }

    fn inspect(&self, id: &DocumentId) -> Result<DocumentCache, DocumentError> {
        let (client, root, document_id) = self.resource(id)?;
        let response = Self::request(
            &client,
            wire::RequestBody::DocumentInspect(wire::DocumentInspectRequest { document_id }),
        )?;
        let wire::ResponseBody::DocumentInspect(response) = response else {
            return Err(DocumentError::io("Document 応答が不正です"));
        };
        self.cache(client, root, response.document)
    }
}

impl DocumentConnection for HostDocuments {
    fn fork(&self) -> std::sync::Arc<dyn DocumentConnection> {
        std::sync::Arc::new(Self::new(self.host.clone()))
    }
    fn workspace(&self, root: &Path) -> Result<lapis_editor_core::WorkspaceId, DocumentError> {
        self.host
            .register_workspace(root)
            .map(|id| lapis_editor_core::WorkspaceId::new(id.as_str()))
            .map_err(|error| DocumentError::io(error.to_string()))
    }
    fn create(&self, root: Option<&Path>) -> Result<DocumentCache, DocumentError> {
        let root = match root {
            Some(root) => root.to_owned(),
            None => {
                std::env::current_dir().map_err(|error| DocumentError::io(error.to_string()))?
            }
        };
        let root = root
            .canonicalize()
            .map_err(|error| DocumentError::io(error.to_string()))?;
        let client = self
            .host
            .connect(&root)
            .map_err(|error| DocumentError::io(error.to_string()))?;
        let response = Self::request(
            &client,
            wire::RequestBody::DocumentUntitled(wire::DocumentUntitledRequest {
                workspace_id: client.workspace_id().clone(),
            }),
        )?;
        let wire::ResponseBody::DocumentUntitled(response) = response else {
            return Err(DocumentError::io("Document 応答が不正です"));
        };
        let mut cache = self.cache(client, root, response.document)?;
        cache.reused = false;
        Ok(cache)
    }

    fn open(&self, path: &Path) -> Result<DocumentCache, DocumentError> {
        let path = match path.canonicalize() {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let name = path
                    .file_name()
                    .ok_or_else(|| DocumentError::io("ファイル名がありません"))?;
                let parent = path
                    .parent()
                    .filter(|parent| !parent.as_os_str().is_empty())
                    .unwrap_or(Path::new("."));
                parent
                    .canonicalize()
                    .map_err(|error| DocumentError::io(error.to_string()))?
                    .join(name)
            }
            Err(error) => return Err(DocumentError::io(error.to_string())),
        };
        let parent = path
            .parent()
            .ok_or_else(|| DocumentError::io("親ディレクトリがありません"))?;
        let (client, _) = self
            .host
            .connect_directory(parent)
            .map_err(|error| DocumentError::io(error.to_string()))?;
        let root = self
            .host
            .inner
            .workspaces
            .lock()
            .map_err(|_| DocumentError::io("Workspace lock に失敗しました"))?
            .iter()
            .find(|(_, id)| *id == client.workspace_id())
            .map(|(root, _)| root.clone())
            .ok_or_else(|| DocumentError::io("Workspace がありません"))?;
        let relative = relative(&root, &path)?;
        let response = Self::request(
            &client,
            wire::RequestBody::DocumentOpen(wire::DocumentOpenRequest {
                workspace_id: client.workspace_id().clone(),
                path: relative,
            }),
        )?;
        let wire::ResponseBody::DocumentOpen(response) = response else {
            return Err(DocumentError::io("Document 応答が不正です"));
        };
        let mut cache = self.cache(client, root, response.document)?;
        cache.reused = response.reused;
        Ok(cache)
    }

    fn attach(&self, id: &DocumentId) -> Result<DocumentCache, DocumentError> {
        if self.resource(id).is_ok()
            && let Ok(cache) = self.inspect(id)
        {
            return Ok(cache);
        }
        let mut roots = self
            .host
            .inner
            .workspaces
            .lock()
            .map_err(|_| DocumentError::io("Workspace lock に失敗しました"))?
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        roots.sort();
        for root in roots {
            let Ok(client) = self.host.connect(&root) else {
                continue;
            };
            let document_id = wire::DocumentId::try_new(id.as_str())
                .map_err(|_| DocumentError::io("Document ID が不正です"))?;
            if let Ok(wire::ResponseBody::DocumentInspect(response)) = Self::request(
                &client,
                wire::RequestBody::DocumentInspect(wire::DocumentInspectRequest { document_id }),
            ) {
                return self.cache(client, root, response.document);
            }
        }
        Err(DocumentError::io("Document がありません"))
    }

    fn edit(
        &self,
        id: &DocumentId,
        revision: Revision,
        range: Range<usize>,
        replacement: &str,
    ) -> Result<DocumentCache, DocumentError> {
        let (client, _, document_id) = self.resource(id)?;
        let edit =
            wire::DocumentTextEdit::try_new(range.start as u64, range.end as u64, replacement)
                .map_err(|error| DocumentError::io(error.to_string()))?;
        let transaction = wire::DocumentTransaction::try_new(vec![edit])
            .map_err(|error| DocumentError::io(error.to_string()))?;
        Self::request(
            &client,
            wire::RequestBody::DocumentEdit(wire::DocumentEditRequest {
                document_id,
                base_revision: wire::Revision::new(revision.number()),
                transaction,
            }),
        )?;
        self.inspect(id)
    }

    fn history(
        &self,
        id: &DocumentId,
        revision: Revision,
        undo: bool,
    ) -> Result<DocumentCache, DocumentError> {
        let (client, _, document_id) = self.resource(id)?;
        let request = wire::DocumentHistoryRequest {
            document_id,
            base_revision: wire::Revision::new(revision.number()),
        };
        Self::request(
            &client,
            if undo {
                wire::RequestBody::DocumentUndo(request)
            } else {
                wire::RequestBody::DocumentRedo(request)
            },
        )?;
        self.inspect(id)
    }

    fn save(
        &self,
        id: &DocumentId,
        revision: Revision,
        path: Option<PathBuf>,
    ) -> Result<DocumentCache, DocumentError> {
        let (client, root, document_id) = self.resource(id)?;
        let base_revision = wire::Revision::new(revision.number());
        let request = if let Some(path) = path {
            let path = if path.is_absolute() {
                path
            } else {
                std::env::current_dir()
                    .map_err(|error| DocumentError::io(error.to_string()))?
                    .join(path)
            };
            let parent = path
                .parent()
                .ok_or_else(|| DocumentError::io("親ディレクトリがありません"))?;
            let name = path
                .file_name()
                .ok_or_else(|| DocumentError::io("ファイル名がありません"))?;
            let path = parent
                .canonicalize()
                .map_err(|error| DocumentError::io(error.to_string()))?
                .join(name);
            if !path.starts_with(&root) {
                let parent = path
                    .parent()
                    .ok_or_else(|| DocumentError::io("親ディレクトリがありません"))?;
                let (destination, _) = self
                    .host
                    .connect_directory(parent)
                    .map_err(|error| DocumentError::io(error.to_string()))?;
                let destination_root = self
                    .host
                    .inner
                    .workspaces
                    .lock()
                    .map_err(|_| DocumentError::io("Workspace lock に失敗しました"))?
                    .iter()
                    .find(|(_, workspace_id)| *workspace_id == destination.workspace_id())
                    .map(|(root, _)| root.clone())
                    .ok_or_else(|| DocumentError::io("Workspace がありません"))?;
                self.host
                    .service()
                    .relocate_document(
                        client.workspace_id().clone(),
                        destination.workspace_id().clone(),
                        document_id.clone(),
                        base_revision,
                        relative(&destination_root, &path)?,
                    )
                    .map_err(|error| match error {
                        lapis_backend_state::BackendServiceError::State(
                            lapis_backend_state::BackendStateError::Document(error),
                        ) => error,
                        lapis_backend_state::BackendServiceError::State(
                            lapis_backend_state::BackendStateError::RevisionConflict(_),
                        ) => DocumentError::conflict("Document revision が競合しています"),
                        error => DocumentError::io(error.to_string()),
                    })?;
                return self.attach(id);
            }
            wire::RequestBody::DocumentSaveAs(wire::DocumentSaveAsRequest {
                document_id,
                base_revision,
                path: relative(&root, &path)?,
            })
        } else {
            wire::RequestBody::DocumentSave(wire::DocumentSaveRequest {
                document_id,
                base_revision,
            })
        };
        Self::request(&client, request)?;
        self.inspect(id)
    }

    fn reload(&self, id: &DocumentId, revision: Revision) -> Result<DocumentCache, DocumentError> {
        let (client, _, document_id) = self.resource(id)?;
        Self::request(
            &client,
            wire::RequestBody::DocumentReload(wire::DocumentHistoryRequest {
                document_id,
                base_revision: wire::Revision::new(revision.number()),
            }),
        )?;
        self.inspect(id)
    }

    fn discard(&self, id: &DocumentId, revision: Revision) -> Result<DocumentCache, DocumentError> {
        let (client, root, document_id) = self.resource(id)?;
        let response = Self::request(
            &client,
            wire::RequestBody::DocumentDiscard(wire::DocumentDiscardRequest {
                document_id,
                base_revision: wire::Revision::new(revision.number()),
            }),
        )?;
        let wire::ResponseBody::DocumentDiscard(response) = response else {
            return Err(DocumentError::io("Document 応答が不正です"));
        };
        self.cache(client, root, response.document)
    }

    fn close(&self, id: &DocumentId) -> Result<(), DocumentError> {
        self.attach(id)?;
        let (client, _, document_id) = self.resource(id)?;
        Self::request(
            &client,
            wire::RequestBody::DocumentClose(wire::DocumentCloseRequest { document_id }),
        )?;
        self.resources
            .lock()
            .map_err(|_| DocumentError::io("Document 接続 lock に失敗しました"))?
            .remove(id);
        Ok(())
    }
}

fn relative(root: &Path, path: &Path) -> Result<wire::WorkspaceRelativePath, DocumentError> {
    wire::WorkspaceRelativePath::parse(
        path.strip_prefix(root)
            .map_err(|_| DocumentError::io("保存先は Workspace 内に指定してください"))?
            .to_string_lossy()
            .replace('\\', "/"),
    )
    .map_err(|error| DocumentError::io(error.to_string()))
}
