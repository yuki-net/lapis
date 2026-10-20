use crate::client_adapter::{ClientWorker, DocumentClient, ResponseGate};
use gpui::{Context, Timer};
use lapis_document::Revision;
use lapis_editor_core::DocumentId;
use lapis_lsp::{DefinitionTarget, Diagnostic, LspPosition};
use std::time::Duration;

pub(crate) enum ProblemsEvent {
    Status(String),
    Definition(DefinitionTarget),
}
pub(crate) struct ProblemsFeature {
    worker: ClientWorker<DocumentClient>,
    pub(super) diagnostics: Vec<Diagnostic>,
    pub(super) message: String,
    target: Option<(DocumentId, Revision)>,
    generation: u64,
    request_gate: ResponseGate,
    busy: bool,
}
impl gpui::EventEmitter<ProblemsEvent> for ProblemsFeature {}
impl ProblemsFeature {
    pub(crate) fn new(worker: ClientWorker<DocumentClient>, cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_millis(500)).await;
                if this.update(cx, |problems, cx| problems.poll(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            worker,
            diagnostics: Vec::new(),
            message: String::new(),
            target: None,
            generation: 0,
            request_gate: ResponseGate::default(),
            busy: false,
        }
    }
    pub(crate) fn set_target(
        &mut self,
        target: Option<(DocumentId, Revision)>,
        cx: &mut Context<Self>,
    ) {
        if self.target == target {
            return;
        }
        self.target = target;
        self.generation += 1;
        self.request_gate.invalidate();
        self.diagnostics.clear();
        cx.notify();
    }
    fn poll(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        let generation = self.generation;
        let admitted = self.worker.submit(
            cx,
            |client| {
                let result = client
                    .lsp
                    .sync_active(&client.editor)
                    .and_then(|_| client.lsp.refresh(&client.editor));
                result.map_err(|error| error.to_string())?;
                Ok(client.lsp.diagnostics().to_vec())
            },
            move |problems, result, cx| {
                problems.busy = false;
                if generation != problems.generation {
                    return;
                }
                match result {
                    Ok(diagnostics) => {
                        problems.diagnostics = diagnostics;
                        problems.message.clear();
                    }
                    Err(error) => problems.message = error,
                }
                cx.notify();
            },
        );
        if let Err(error) = admitted {
            self.busy = false;
            self.message = error;
            cx.notify();
        }
    }
    pub(crate) fn request(
        &mut self,
        definition: bool,
        position: LspPosition,
        cx: &mut Context<Self>,
    ) {
        let target = self.target.clone();
        let token = self.request_gate.issue();
        let admitted = self.worker.submit(
            cx,
            move |client| {
                if target.as_ref()
                    != client
                        .editor
                        .active_document_id()
                        .map(|id| (id.clone(), client.editor.active_revision()))
                        .as_ref()
                {
                    return Err("文書が変更されました".to_owned());
                }
                client
                    .lsp
                    .sync_active(&client.editor)
                    .map_err(|error| error.to_string())?;
                if definition {
                    client
                        .lsp
                        .definition(&client.editor, position)
                        .map(|target| (target, String::new()))
                        .map_err(|error| error.to_string())
                } else {
                    client
                        .lsp
                        .completion(&client.editor, position)
                        .map(|items| {
                            (
                                None,
                                items
                                    .iter()
                                    .take(6)
                                    .map(|item| item.label.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", "),
                            )
                        })
                        .map_err(|error| error.to_string())
                }
            },
            move |problems, result, cx| {
                if !problems.request_gate.accept(token) {
                    return;
                }
                match result {
                    Ok((Some(target), _)) => cx.emit(ProblemsEvent::Definition(target)),
                    Ok((None, message)) => cx.emit(ProblemsEvent::Status(message)),
                    Err(error) => cx.emit(ProblemsEvent::Status(error)),
                }
            },
        );
        if let Err(error) = admitted {
            cx.emit(ProblemsEvent::Status(error));
        }
    }
}
