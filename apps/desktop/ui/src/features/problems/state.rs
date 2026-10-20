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
    pub(super) state: ProblemsState,
    pub(super) message: String,
    busy: bool,
}
#[derive(Default)]
pub(super) struct ProblemsState {
    pub(super) diagnostics: Vec<Diagnostic>,
    target: Option<(DocumentId, Revision)>,
    generation: u64,
    request_gate: ResponseGate,
}

impl ProblemsState {
    fn set_target(&mut self, target: Option<(DocumentId, Revision)>) -> bool {
        if self.target == target {
            return false;
        }
        let document_changed =
            self.target.as_ref().map(|(id, _)| id) != target.as_ref().map(|(id, _)| id);
        self.target = target;
        self.request_gate.invalidate();
        if document_changed {
            self.generation += 1;
            self.diagnostics.clear();
        }
        true
    }

    fn apply_diagnostics(
        &mut self,
        generation: u64,
        target: Option<(DocumentId, Revision)>,
        diagnostics: Vec<Diagnostic>,
    ) -> bool {
        if generation != self.generation || target != self.target {
            return false;
        }
        self.diagnostics = diagnostics;
        true
    }
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
            state: ProblemsState::default(),
            message: String::new(),
            busy: false,
        }
    }
    pub(crate) fn set_target(
        &mut self,
        target: Option<(DocumentId, Revision)>,
        cx: &mut Context<Self>,
    ) {
        if self.state.set_target(target) {
            cx.notify();
        }
    }
    fn poll(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        let generation = self.state.generation;
        let admitted = self.worker.submit(
            cx,
            |client| {
                let result = client
                    .lsp
                    .sync_active(&client.editor)
                    .and_then(|_| client.lsp.refresh(&client.editor));
                result.map_err(|error| error.to_string())?;
                let target = client
                    .editor
                    .active_document_id()
                    .map(|id| (id.clone(), client.editor.active_revision()));
                let diagnostics = client.lsp.diagnostics().to_vec();
                let current = diagnostics.iter().all(|diagnostic| {
                    Some(diagnostic.path.as_path()) != client.editor.active_path()
                        || diagnostic.revision == client.editor.active_revision()
                });
                Ok((target, current.then_some(diagnostics)))
            },
            move |problems, result, cx| {
                problems.busy = false;
                if generation != problems.state.generation {
                    return;
                }
                match result {
                    Ok((target, Some(diagnostics))) => {
                        if !problems
                            .state
                            .apply_diagnostics(generation, target, diagnostics)
                        {
                            return;
                        }
                        problems.message.clear();
                    }
                    Ok((_, None)) => return,
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
        let target = self.state.target.clone();
        let token = self.state.request_gate.issue();
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
                if !problems.state.request_gate.accept(token) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use lapis_lsp::{DiagnosticSeverity, LspRange};

    fn target(name: &str, revision: u64) -> Option<(DocumentId, Revision)> {
        Some((DocumentId::new(name), Revision::new(revision)))
    }

    fn diagnostic(revision: u64) -> Diagnostic {
        Diagnostic {
            path: "file.rs".into(),
            range: LspRange {
                start: LspPosition {
                    line: 0,
                    utf16_column: 0,
                },
                end: LspPosition {
                    line: 0,
                    utf16_column: 1,
                },
            },
            severity: DiagnosticSeverity::Error,
            message: "diagnostic".into(),
            revision: Revision::new(revision),
        }
    }

    #[test]
    fn revisions_preserve_visible_diagnostics_but_invalidate_definition_requests() {
        let mut state = ProblemsState::default();
        state.set_target(target("document", 1));
        let generation = state.generation;
        assert!(state.apply_diagnostics(generation, target("document", 1), vec![diagnostic(1)]));
        let request = state.request_gate.issue();
        assert!(state.set_target(target("document", 2)));
        assert_eq!(state.generation, generation);
        assert_eq!(state.diagnostics, vec![diagnostic(1)]);
        assert!(!state.request_gate.accept(request));
        assert!(!state.apply_diagnostics(generation, target("document", 1), vec![]));
        assert_eq!(state.diagnostics, vec![diagnostic(1)]);
        assert!(state.apply_diagnostics(generation, target("document", 2), vec![diagnostic(2)]));
        assert_eq!(state.diagnostics, vec![diagnostic(2)]);
    }

    #[test]
    fn document_switch_clears_diagnostics_and_rejects_previous_poll_even_after_switching_back() {
        let mut state = ProblemsState::default();
        state.set_target(target("first", 1));
        let generation = state.generation;
        state.apply_diagnostics(generation, target("first", 1), vec![diagnostic(1)]);
        state.set_target(target("second", 1));
        assert!(state.diagnostics.is_empty());
        state.set_target(target("first", 1));
        assert!(!state.apply_diagnostics(generation, target("first", 1), vec![diagnostic(1)]));
        assert!(state.diagnostics.is_empty());
        state.set_target(None);
        assert!(state.diagnostics.is_empty());
    }

    #[test]
    fn unchanged_targets_do_not_cancel_requests_or_clear_diagnostics() {
        let mut state = ProblemsState::default();
        state.set_target(target("document", 1));
        state.apply_diagnostics(state.generation, target("document", 1), vec![diagnostic(1)]);
        let request = state.request_gate.issue();
        assert!(!state.set_target(target("document", 1)));
        assert!(state.request_gate.accept(request));
        assert_eq!(state.diagnostics, vec![diagnostic(1)]);
    }
}
