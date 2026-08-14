use super::*;
use crate::extension_ui::PanelPosition;

impl Editor {
    pub(super) fn request_document_close(
        &mut self,
        document_id: lapis_editor_core::DocumentId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(document) = self
            .session
            .tabs()
            .into_iter()
            .find(|document| document.id == document_id)
        else {
            return;
        };
        let affected_panels = self.panels_with_document(&document_id);

        if document.dirty {
            if self.session.active_document_id() != Some(&document_id) {
                self.persist_active_view();
                self.session.activate_document(&document_id);
                self.shell.synchronize_documents(&self.session.tabs());
                self.restore_active_view();
                cx.notify();
            }

            let message = format!("Close {}?", document.display_name);
            let detail = "This document has unsaved changes.";
            let receiver = window.prompt(
                PromptLevel::Warning,
                &message,
                Some(detail),
                &[
                    PromptButton::new("Save"),
                    PromptButton::new("Discard"),
                    PromptButton::cancel("Cancel"),
                ],
                cx,
            );
            cx.spawn(async move |this, cx| {
                let Ok(answer) = receiver.await else {
                    return;
                };
                let _ = this.update(cx, |editor, cx| {
                    editor.finish_document_close_prompt(document_id, affected_panels, answer, cx);
                });
            })
            .detach();
        } else {
            self.finish_document_close(
                document_id,
                affected_panels,
                DocumentCloseDisposition::PreserveChanges,
                cx,
            );
        }
    }

    fn panels_with_document(
        &self,
        document_id: &lapis_editor_core::DocumentId,
    ) -> Vec<PanelPosition> {
        let tab = PanelTab::Document(document_id.clone());
        self.shell
            .panels()
            .into_iter()
            .filter(|panel| panel.contains(&tab))
            .map(|panel| panel.position)
            .collect()
    }

    fn finish_document_close_prompt(
        &mut self,
        document_id: lapis_editor_core::DocumentId,
        affected_panels: Vec<PanelPosition>,
        answer: usize,
        cx: &mut Context<Self>,
    ) {
        match answer {
            0 => match self.session.save_document() {
                Ok(DocumentAction::Completed) => {
                    self.finish_document_close(
                        document_id,
                        affected_panels,
                        DocumentCloseDisposition::PreserveChanges,
                        cx,
                    );
                }
                Ok(DocumentAction::Cancelled) => {}
                Err(error) => {
                    self.status = format!("保存失敗: {error}");
                    cx.notify();
                }
            },
            1 => self.finish_document_close(
                document_id,
                affected_panels,
                DocumentCloseDisposition::DiscardChanges,
                cx,
            ),
            _ => {}
        }
    }

    fn finish_document_close(
        &mut self,
        document_id: lapis_editor_core::DocumentId,
        affected_panels: Vec<PanelPosition>,
        disposition: DocumentCloseDisposition,
        cx: &mut Context<Self>,
    ) {
        match self.session.close_document(&document_id, disposition) {
            Ok(true) => {
                self.shell.synchronize_documents(&self.session.tabs());
                self.restore_active_view();
                for position in affected_panels {
                    self.close_empty_panel(position, cx);
                }
                self.refresh_feature_activation();
                cx.notify();
            }
            Ok(false) => {}
            Err(error) => {
                self.status = format!("閉じることができませんでした: {error}");
                cx.notify();
            }
        }
    }
}
