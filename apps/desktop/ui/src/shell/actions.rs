use super::*;
use crate::extension_ui::PanelPosition;

impl Editor {
    pub(super) fn toggle_preview(
        &mut self,
        _: &TogglePreview,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_panel(
            PanelPosition::Right,
            Some(ViewId::new(id::VIEW_PREVIEW)),
            cx,
        );
        self.shell.command_palette_open = false;
        self.refresh_feature_activation();
        cx.notify();
    }

    pub(super) fn toggle_bottom_panel(
        &mut self,
        _: &ToggleBottomPanel,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_panel(PanelPosition::Bottom, cx);
        self.shell.command_palette_open = false;
        self.refresh_feature_activation();
        cx.notify();
    }

    pub(super) fn toggle_assistant(
        &mut self,
        _: &ToggleAssistant,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_panel(
            PanelPosition::Right,
            Some(ViewId::new(id::VIEW_ASSISTANT)),
            cx,
        );
        self.shell.command_palette_open = false;
        self.refresh_feature_activation();
        cx.notify();
    }

    pub(super) fn select_tool(&mut self, view: ViewId, cx: &mut Context<Self>) {
        self.select_view(PanelPosition::Left, view, cx);
    }

    pub(super) fn select_view(
        &mut self,
        position: PanelPosition,
        view: ViewId,
        cx: &mut Context<Self>,
    ) {
        self.activate_panel_view(position, view, cx);
        self.refresh_feature_activation();
        cx.notify();
    }

    pub(super) fn open_panel(
        &mut self,
        position: PanelPosition,
        view: Option<ViewId>,
        cx: &mut Context<Self>,
    ) {
        self.request_panel_open(position, true, cx);
        if let Some(view) = view {
            self.shell
                .panel_mut(position)
                .activate_tool_without_open(view);
        }
    }

    pub(super) fn toggle_panel(&mut self, position: PanelPosition, cx: &mut Context<Self>) {
        if position == PanelPosition::Main {
            return;
        }
        let open = !self.shell.panel(position).open;
        self.request_panel_open(position, open, cx);
    }

    pub(super) fn toggle_header_panel(&mut self, position: PanelPosition, cx: &mut Context<Self>) {
        self.toggle_panel(position, cx);
        self.shell.command_palette_open = false;
        self.refresh_feature_activation();
        cx.notify();
    }

    pub(super) fn activate_panel_view(
        &mut self,
        position: PanelPosition,
        view: ViewId,
        cx: &mut Context<Self>,
    ) {
        self.request_panel_open(position, true, cx);
        self.shell
            .panel_mut(position)
            .activate_tool_without_open(view);
    }

    pub(super) fn close_panel(&mut self, position: PanelPosition, cx: &mut Context<Self>) {
        if position == PanelPosition::Main {
            return;
        }
        self.shell.panel_mut(position).active = None;
        self.request_panel_open(position, false, cx);
        self.refresh_feature_activation();
        cx.notify();
    }

    fn request_panel_open(&mut self, position: PanelPosition, open: bool, cx: &mut Context<Self>) {
        let transition = self
            .shell
            .panel_mut(position)
            .request_open(open, std::time::Instant::now());
        if let Some((generation, duration)) = transition {
            self.schedule_panel_transition(position, generation, duration, cx);
        }
    }

    fn schedule_panel_transition(
        &mut self,
        position: PanelPosition,
        generation: u64,
        duration: Duration,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            Timer::after(duration).await;
            let _ = this.update(cx, |editor, cx| {
                editor.complete_panel_transition(position, generation, cx);
            });
        })
        .detach();
    }

    fn complete_panel_transition(
        &mut self,
        position: PanelPosition,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        let next = self
            .shell
            .panel_mut(position)
            .complete_transition(generation, std::time::Instant::now());
        if let Some((next_generation, duration)) = next {
            self.schedule_panel_transition(position, next_generation, duration, cx);
        }
        self.refresh_feature_activation();
        cx.notify();
    }

    pub(super) fn open_tool_picker(&mut self, position: PanelPosition, cx: &mut Context<Self>) {
        self.shell.tool_picker = Some(position);
        self.shell.set_tool_picker_query(String::new());
        cx.notify();
    }

    pub(super) fn toggle_settings_menu(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.shell.settings_menu_open = !self.shell.settings_menu_open;
        self.shell.settings_menu_anchor = position;
        self.shell.tool_picker = None;
        if !self.shell.settings_menu_open {
            self.shell.theme_picker_open = false;
        }
        cx.notify();
    }

    pub(super) fn close_settings_menu(&mut self, cx: &mut Context<Self>) {
        if self.shell.settings_menu_open {
            self.shell.settings_menu_open = false;
            self.shell.theme_picker_open = false;
            cx.notify();
        }
    }

    pub(super) fn settings_menu_key_down(
        &mut self,
        event: &KeyDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.shell.settings_menu_open && event.keystroke.key == "escape" {
            self.close_settings_menu(cx);
        }
    }

    pub(super) fn open_settings_view(&mut self, cx: &mut Context<Self>) {
        self.open_panel(
            PanelPosition::Main,
            Some(ViewId::new(id::VIEW_SETTINGS)),
            cx,
        );
        self.shell.settings_menu_open = false;
        self.shell.theme_picker_open = false;
        self.refresh_feature_activation();
        cx.notify();
    }

    pub(super) fn toggle_theme_preference(&mut self, cx: &mut Context<Self>) {
        if self.shell.theme_save_in_flight {
            return;
        }
        self.shell.theme_picker_open = !self.shell.theme_picker_open;
        cx.notify();
    }

    pub(super) fn select_theme(&mut self, theme_id: ThemeId, cx: &mut Context<Self>) {
        if self.shell.theme_save_in_flight || theme::active_id() == theme_id {
            return;
        }
        let previous = theme::active_id();
        if !theme::set_active(&theme_id) {
            self.status = format!("未登録のテーマです: {}", theme_id.as_str());
            cx.notify();
            return;
        }

        self.shell.theme_before_save = Some(previous);
        self.shell.theme_save_in_flight = true;
        self.shell.theme_picker_open = false;
        self.shell.settings_menu_open = false;
        cx.notify();

        let settings = self.settings.clone();
        let theme_value = theme_id.as_str().to_owned();
        let save = cx.background_spawn(async move { settings.set_theme(theme_value) });
        cx.spawn(async move |this, cx| {
            let result = save.await;
            let _ = this.update(cx, |editor, cx| {
                if let Err(error) = result {
                    if let Some(previous) = editor.shell.theme_before_save.take() {
                        let _ = theme::set_active(&previous);
                    }
                    editor.status = format!("テーマ保存失敗: {error}");
                } else {
                    editor.shell.theme_before_save = None;
                }
                editor.shell.theme_save_in_flight = false;
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn select_panel_tab(
        &mut self,
        position: PanelPosition,
        tab: PanelTab,
        cx: &mut Context<Self>,
    ) {
        self.request_panel_open(position, true, cx);
        self.shell.panel_mut(position).activate_without_open(tab);
        self.refresh_feature_activation();
        cx.notify();
    }

    pub(super) fn close_panel_tab(
        &mut self,
        position: PanelPosition,
        tab: PanelTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let PanelTab::Document(document_id) = tab else {
            self.shell.panel_mut(position).remove(&tab);
            self.refresh_feature_activation();
            cx.notify();
            return;
        };

        let Some(document) = self
            .session
            .tabs()
            .into_iter()
            .find(|document| document.id == document_id)
        else {
            return;
        };

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
                    editor.finish_document_close_prompt(document_id, answer, cx);
                });
            })
            .detach();
        } else {
            self.finish_document_close(document_id, DocumentCloseDisposition::PreserveChanges, cx);
        }
    }

    fn finish_document_close_prompt(
        &mut self,
        document_id: lapis_editor_core::DocumentId,
        answer: usize,
        cx: &mut Context<Self>,
    ) {
        match answer {
            0 => match self.session.save_document() {
                Ok(DocumentAction::Completed) => {
                    self.finish_document_close(
                        document_id,
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
                DocumentCloseDisposition::DiscardChanges,
                cx,
            ),
            _ => {}
        }
    }

    fn finish_document_close(
        &mut self,
        document_id: lapis_editor_core::DocumentId,
        disposition: DocumentCloseDisposition,
        cx: &mut Context<Self>,
    ) {
        match self.session.close_document(&document_id, disposition) {
            Ok(true) => {
                self.shell.synchronize_documents(&self.session.tabs());
                self.restore_active_view();
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

    pub(super) fn close_tool_picker(&mut self, cx: &mut Context<Self>) {
        if self.shell.tool_picker.take().is_some() {
            self.shell.set_tool_picker_query(String::new());
            cx.notify();
        }
    }

    pub(super) fn select_tool_from_picker(
        &mut self,
        position: PanelPosition,
        view: ViewId,
        cx: &mut Context<Self>,
    ) {
        self.activate_panel_view(position, view, cx);
        self.shell.tool_picker = None;
        self.shell.set_tool_picker_query(String::new());
        self.refresh_feature_activation();
        cx.notify();
    }

    pub(super) fn tool_picker_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.shell.tool_picker.is_none() {
            return;
        }

        let key = event.keystroke.key.as_str();
        if key == "escape" {
            self.close_tool_picker(cx);
            return;
        }
        if key == "backspace" {
            self.shell.tool_picker_query.pop();
            cx.notify();
            return;
        }
        if key == "enter" {
            if let Some(position) = self.shell.tool_picker
                && let Some(view) = self
                    .feature_registry
                    .panel_contributions(position)
                    .into_iter()
                    .find(|contribution| self.tool_matches(contribution))
                    .and_then(|contribution| contribution.view.clone())
            {
                self.select_tool_from_picker(position, view, cx);
            }
            return;
        }
        if event.keystroke.modifiers.control
            || event.keystroke.modifiers.alt
            || event.keystroke.modifiers.platform
        {
            return;
        }
        if let Some(character) = event
            .keystroke
            .key
            .chars()
            .next()
            .filter(|_| key.len() == 1)
        {
            self.shell.tool_picker_query.push(character);
            window.prevent_default();
            cx.notify();
        }
    }

    fn tool_matches(&self, contribution: &crate::extension_ui::UiContribution) -> bool {
        let query = self.shell.tool_picker_query.trim().to_lowercase();
        if query.is_empty() {
            return true;
        }
        let title = self.locale.resolve(&contribution.title).to_lowercase();
        let view = contribution
            .view
            .as_ref()
            .map(|view| view.as_str())
            .unwrap_or_default()
            .to_lowercase();
        title.contains(&query) || view.contains(&query)
    }

    pub(super) fn move_panel_tab(
        &mut self,
        source: PanelPosition,
        target: PanelPosition,
        tab: PanelTab,
        cx: &mut Context<Self>,
    ) {
        if !self.shell.panel(target).open && target != PanelPosition::Main {
            return;
        }
        self.shell.move_tab(source, target, tab);
        self.refresh_feature_activation();
        cx.notify();
    }
}
