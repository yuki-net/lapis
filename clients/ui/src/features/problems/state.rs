use crate::client_adapter::{ClientWorker, DocumentClient, ResponseGate};
use document::Revision;
use editor_core::DocumentId;
use gpui::Context;
use lsp::{DefinitionTarget, Diagnostic, LspPosition};

pub(crate) enum ProblemsEvent {
    Status(String),
    Definition(DefinitionTarget),
}
pub(crate) struct ProblemsFeature {
    worker: ClientWorker<DocumentClient>,
    pub(super) state: ProblemsState,
    pub(super) message: String,
    busy: bool,
    pending: bool,
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

/// 対象パスの診断が要求 revision のものだけかを返す。
///
/// 他ファイルの診断は revision が違ってもよい。
fn diagnostics_match_revision(
    diagnostics: &[Diagnostic],
    path: &std::path::Path,
    revision: Revision,
) -> bool {
    diagnostics
        .iter()
        .all(|diagnostic| diagnostic.path != path || diagnostic.revision == revision)
}

impl gpui::EventEmitter<ProblemsEvent> for ProblemsFeature {}
impl ProblemsFeature {
    pub(crate) fn new(worker: ClientWorker<DocumentClient>, cx: &mut Context<Self>) -> Self {
        let feature = Self {
            worker,
            state: ProblemsState::default(),
            message: String::new(),
            busy: false,
            pending: false,
        };
        feature.subscribe_diagnostics(cx);
        feature
    }
    /// backend の診断更新通知で再取得する。ポーリングは行わない。
    fn subscribe_diagnostics(&self, cx: &mut Context<Self>) {
        let worker = self.worker.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let opened = executor
                .spawn(async move {
                    worker
                        .request(|client| Ok(client.lsp.diagnostics_changes()))
                        .recv()
                })
                .await;
            let Ok(Ok(Some(mut changes))) = opened else {
                return;
            };
            loop {
                let (next, outcome) = executor
                    .spawn(async move {
                        use futures_util::StreamExt as _;
                        let outcome = changes.next().await;
                        (changes, outcome)
                    })
                    .await;
                changes = next;
                if outcome.is_none() {
                    break;
                }
                if this
                    .update(cx, |problems, cx| problems.on_diagnostics_changed(cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }
    pub(crate) fn set_target(
        &mut self,
        target: Option<(DocumentId, Revision)>,
        cx: &mut Context<Self>,
    ) {
        if self.state.set_target(target) {
            // 対象文書の revision が変わるたびに診断を取り直す。
            self.poll(cx);
            cx.notify();
        }
    }
    /// 診断が要求 revision に追いついていない場合に、少し待って取り直す。
    fn schedule_poll(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            crate::timer::Timer::after(std::time::Duration::from_millis(200)).await;
            let _ = this.update(cx, |problems, cx| problems.poll(cx));
        })
        .detach();
    }
    fn on_diagnostics_changed(&mut self, cx: &mut Context<Self>) {
        // 取得中は完了後に取り直す。通知を pop して捨てない。
        self.poll(cx);
    }
    fn finish_poll(&mut self, cx: &mut Context<Self>) {
        self.busy = false;
        if self.pending {
            self.pending = false;
            self.poll(cx);
        }
    }
    /// 診断要求を worker で開始し、完了待ちだけを background で行う。
    fn poll(&mut self, cx: &mut Context<Self>) {
        if self.state.target.is_none() {
            self.pending = false;
            return;
        }
        if self.busy {
            self.pending = true;
            return;
        }
        self.busy = true;
        self.pending = false;
        let generation = self.state.generation;
        let admitted = self.worker.submit(
            cx,
            |client| {
                let request = client
                    .lsp
                    .request_diagnostics(&client.editor)
                    .map_err(|error| error.to_string())?;
                let target = client
                    .editor
                    .active_document_id()
                    .map(|id| (id.clone(), client.editor.active_revision()));
                Ok((request, target))
            },
            move |problems, result, cx| {
                let (request, target) = match result {
                    Ok(result) => result,
                    Err(error) => {
                        problems.message = error;
                        problems.finish_poll(cx);
                        cx.notify();
                        return;
                    }
                };
                if generation != problems.state.generation || problems.state.target != target {
                    // 取得中に対象が変わった。現在の対象で取り直す。
                    problems.pending = true;
                    problems.finish_poll(cx);
                    return;
                }
                let (Some(request), Some(target)) = (request, target) else {
                    problems.finish_poll(cx);
                    return;
                };
                let executor = cx.background_executor().clone();
                cx.spawn(async move |this, cx| {
                    let (request_target, result) =
                        executor.spawn(async move { request.wait() }).await;
                    let _ = this.update(cx, |problems, cx| {
                        if generation != problems.state.generation
                            || problems.state.target.as_ref() != Some(&target)
                        {
                            problems.pending = true;
                            problems.finish_poll(cx);
                            return;
                        }
                        if request_target
                            .as_ref()
                            .is_none_or(|(_, revision)| revision != &target.1)
                        {
                            problems.pending = true;
                            problems.finish_poll(cx);
                            return;
                        }
                        match result {
                            Ok(diagnostics) => {
                                // 要求時の revision より古い診断は適用しない。
                                let stale =
                                    request_target.as_ref().is_some_and(|(path, revision)| {
                                        !diagnostics_match_revision(&diagnostics, path, *revision)
                                    });
                                if stale {
                                    // backend が診断を作り直すまで、少し待って取り直す。
                                    problems.schedule_poll(cx);
                                    problems.finish_poll(cx);
                                    return;
                                }
                                if problems.state.apply_diagnostics(
                                    generation,
                                    Some(target),
                                    diagnostics,
                                ) {
                                    problems.message.clear();
                                    cx.notify();
                                }
                            }
                            Err(error) => {
                                problems.message = error.to_string();
                                cx.notify();
                            }
                        }
                        problems.finish_poll(cx);
                    });
                })
                .detach();
            },
        );
        if let Err(error) = admitted {
            self.busy = false;
            self.pending = false;
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
    use lsp::{DiagnosticSeverity, LspRange};

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
    fn diagnostics_from_an_older_revision_are_not_accepted() {
        let path = std::path::Path::new("file.rs");
        assert!(diagnostics_match_revision(
            &[diagnostic(3)],
            path,
            Revision::new(3)
        ));
        assert!(!diagnostics_match_revision(
            &[diagnostic(2)],
            path,
            Revision::new(3)
        ));
        // 他ファイルの診断は revision が違ってもよい。
        assert!(diagnostics_match_revision(
            &[diagnostic(2)],
            std::path::Path::new("other.rs"),
            Revision::new(3)
        ));
    }

    #[test]
    fn diagnostics_updated_without_a_revision_change_replace_the_previous_result() {
        let mut state = ProblemsState::default();
        state.set_target(target("document", 1));
        let generation = state.generation;
        assert!(state.apply_diagnostics(generation, target("document", 1), vec![diagnostic(1)]));
        let mut updated = diagnostic(1);
        updated.message = "updated by push".into();
        assert!(state.apply_diagnostics(generation, target("document", 1), vec![updated]));
        assert_eq!(state.diagnostics[0].message, "updated by push");
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
