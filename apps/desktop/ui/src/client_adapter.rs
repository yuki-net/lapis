use std::{sync::mpsc, time::Duration};

use gpui::{Context, Timer};
use lapis_desktop_client::{
    ConversationRecord, ConversationSession, ConversationViewState, DocumentTab, EditorSession,
    LspSession,
};
use lapis_document::DocumentCache;
use lapis_editor_core::{ConversationId, DocumentId, WorkspaceId};
use lapis_workspace::FileEntry;
use std::path::PathBuf;

pub(crate) struct DocumentClient {
    pub editor: EditorSession,
    pub conversation: ConversationSession,
    pub lsp: LspSession,
    pub edit_chain: EditChain,
}

#[derive(Clone)]
pub(crate) struct ClientSnapshot {
    pub tabs: Vec<DocumentTab>,
    pub documents: Vec<DocumentCache>,
    pub active: Option<DocumentId>,
    pub workspace: Option<WorkspaceId>,
    pub root: Option<PathBuf>,
    pub workspace_name: String,
    pub files: Vec<FileEntry>,
    pub conversations: Vec<ConversationRecord>,
    pub projects: Vec<lapis_desktop_client::Project>,
    pub project_error: Option<String>,
    pub conversation: ConversationId,
    pub view: ConversationViewState,
    pub edit_epoch: EditEpoch,
    pub workspace_view: lapis_desktop_client::WorkspaceSnapshot,
}

impl DocumentClient {
    pub fn snapshot(&mut self) -> Result<ClientSnapshot, String> {
        self.editor.poll_external_changes();
        let tabs = self.editor.tabs();
        let connection = self.editor.document_connection();
        let documents = tabs
            .iter()
            .map(|tab| connection.attach(&tab.id))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        Ok(ClientSnapshot {
            tabs,
            documents,
            active: self.editor.active_document_id().cloned(),
            workspace: self.editor.workspace_id().cloned(),
            root: self.editor.workspace_root().map(ToOwned::to_owned),
            workspace_name: self.editor.workspace_name().to_owned(),
            files: self.editor.file_tree().to_vec(),
            conversations: self.conversation.records().to_vec(),
            projects: self.conversation.projects().to_vec(),
            project_error: self.conversation.load_error().map(ToOwned::to_owned),
            conversation: self.conversation.active_id().clone(),
            view: self.conversation.active_view(),
            edit_epoch: self.edit_chain.epoch(),
            workspace_view: self.editor.snapshot(),
        })
    }
}

#[derive(Clone)]
pub(crate) struct PersistedView {
    pub document: Option<(DocumentId, std::ops::Range<usize>, usize, f32, f32)>,
    pub layout: ConversationViewState,
    pub execution: Option<lapis_editor_core::ExecutionId>,
    pub terminals: Vec<lapis_terminal::TerminalSnapshot>,
    pub input: Option<String>,
}

impl DocumentClient {
    pub(crate) fn persist_view(&mut self, view: PersistedView) -> Result<(), String> {
        let epoch = self.edit_chain.epoch();
        let recovered = self.edit_chain.apply(epoch, || Ok(())).is_err();
        if recovered {
            let text = view.input.as_deref().ok_or("復旧する入力がありません")?;
            self.editor
                .recover_draft(text)
                .map_err(|error| error.to_string())?;
            self.edit_chain.restart();
        }
        if let Some((id, selection, cursor, scroll_x, scroll_y)) = view.document
            && (recovered || self.editor.activate_document(&id))
        {
            self.editor
                .update_active_view(selection, cursor, scroll_x, scroll_y);
        }
        if self.editor.workspace_root().is_none() && self.editor.tabs().is_empty() {
            return Ok(());
        }
        self.conversation
            .capture(&self.editor, view.layout, view.execution, &view.terminals)
            .map_err(|error| error.to_string())
    }
}

pub(crate) use crate::client_worker::{ClientWorker, EditChain, EditEpoch, ResponseGate};

impl<State: Send + 'static> ClientWorker<State> {
    pub(crate) fn submit<Entity: 'static, Output: Send + 'static>(
        &self,
        cx: &mut Context<Entity>,
        operation: impl FnOnce(&mut State) -> Result<Output, String> + Send + 'static,
        complete: impl FnOnce(&mut Entity, Result<Output, String>, &mut Context<Entity>) + 'static,
    ) -> Result<(), String> {
        let receiver = self.try_request(operation)?;
        cx.spawn(async move |this, cx| {
            loop {
                let result = match receiver.try_recv() {
                    Ok(result) => result,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        Err("Client workerが終了しました".to_owned())
                    }
                    Err(mpsc::TryRecvError::Empty) => {
                        if this.upgrade().is_none() {
                            break;
                        }
                        Timer::after(Duration::from_millis(16)).await;
                        continue;
                    }
                };
                let _ = this.update(cx, |entity, cx| complete(entity, result, cx));
                break;
            }
        })
        .detach();
        Ok(())
    }
}
