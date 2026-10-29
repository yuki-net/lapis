use std::{collections::HashMap, ops::Range, time::Duration};

use gpui::{
    App, Bounds, Context, CursorStyle, Element, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, Focusable, GlobalElementId, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, Render, ShapedLine,
    SharedString, Style, TextRun, Timer, UTF16Selection, Window, div, fill, point, prelude::*, px,
    size,
};

use crate::{
    app::*,
    client_adapter::{ClientSnapshot, ClientWorker, DocumentClient},
    components::{ScrollAxis, ScrollState, scroll_viewport},
    features::id,
    theme,
};
use lapis_document::{Document, DocumentError};
use lapis_editor_core::DocumentId;
mod actions;
mod canvas;
mod input;
mod interactions;
mod syntax;
mod widths;
use canvas::{EditorElement, EditorLineLayout};

pub(super) fn descriptor() -> crate::extension_ui::FeatureDescriptor {
    use crate::extension_ui::UiContribution;

    crate::extension_ui::FeatureDescriptor::core(id::FEATURE_EDITOR)
        .contributes(UiContribution::command(
            id::COMMAND_NEW_DOCUMENT,
            "command.new-document",
            "document-new",
            10,
        ))
        .contributes(UiContribution::command(
            id::COMMAND_OPEN_DOCUMENT,
            "command.open-document",
            "folder-open",
            19,
        ))
        .contributes(UiContribution::command(
            id::COMMAND_OPEN_WORKSPACE,
            "command.open-workspace",
            "folder-open",
            20,
        ))
        .contributes(UiContribution::command(
            id::COMMAND_SAVE_DOCUMENT,
            "command.save-document",
            "document-save",
            30,
        ))
        .contributes(UiContribution::command(
            id::COMMAND_TOGGLE_PREVIEW,
            "command.toggle-preview",
            "preview",
            40,
        ))
        .contributes(UiContribution::command(
            id::COMMAND_TOGGLE_ASSISTANT,
            "command.toggle-assistant",
            "assistant",
            60,
        ))
}

pub(super) fn rust_descriptor() -> crate::extension_ui::FeatureDescriptor {
    crate::extension_ui::FeatureDescriptor::bundled(
        id::FEATURE_RUST,
        [crate::extension_ui::ActivationCondition::OnLanguage(
            "rust".into(),
        )],
    )
    .requires("workspace.process")
}

pub(crate) enum EditorEvent {
    Snapshot(Box<ClientSnapshot>),
    Idle,
    Status(String),
    WorkspaceSearch(String),
    Completion(bool),
}

pub(crate) struct Editor {
    syntax_worker: ClientWorker<syntax::SyntaxCache>,
    syntax_generation: std::sync::Arc<std::sync::atomic::AtomicU64>,
    syntax_busy: bool,
    syntax_lines: Option<syntax::Lines>,
    line_widths: widths::LineWidths,
    session: Document,
    document_id: Option<DocumentId>,
    worker: ClientWorker<DocumentClient>,
    generation: u64,
    snapshot_sequence: u64,
    pub(crate) host: u64,
    pub(crate) excluded_documents: Vec<DocumentId>,
    pub(crate) only_documents: Option<Vec<DocumentId>>,
    recovery: bool,
    recovering: bool,
    enabled: bool,
    context_transition: bool,
    edit_epoch: Option<crate::client_adapter::EditEpoch>,
    in_flight: usize,
    group_sync_pending: bool,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    status: String,
    focus_handle: FocusHandle,
    is_selecting: bool,
    last_editor_bounds: Option<Bounds<Pixels>>,
    last_line_layouts: Vec<EditorLineLayout>,
    editor_scroll: ScrollState,
    search_matches: Vec<Range<usize>>,
    views: HashMap<DocumentId, (Range<usize>, bool)>,
    scroll_positions: HashMap<DocumentId, gpui::Point<gpui::Pixels>>,
}

impl gpui::EventEmitter<EditorEvent> for Editor {}

impl Drop for Editor {
    fn drop(&mut self) {
        self.syntax_generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Editor {
    /// Creates a local editor view and starts snapshot polling on the shared worker.
    pub(crate) fn new(worker: ClientWorker<DocumentClient>, cx: &mut Context<Self>) -> Self {
        Self::new_with_polling(worker, true, cx)
    }

    /// Creates a local editor view over an existing worker.
    ///
    /// With `poll`, requests an initial snapshot and polls every two seconds when
    /// idle and outside recovery. Otherwise, snapshots must be supplied by the host.
    pub(crate) fn new_with_polling(
        worker: ClientWorker<DocumentClient>,
        poll: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut editor = Self {
            syntax_worker: ClientWorker::new(syntax::SyntaxCache::default()),
            syntax_generation: Default::default(),
            syntax_busy: false,
            syntax_lines: None,
            line_widths: widths::LineWidths::new(&Document::new()),
            session: Document::new(),
            document_id: None,
            worker,
            generation: 0,
            snapshot_sequence: 0,
            host: 0,
            excluded_documents: Vec::new(),
            only_documents: None,
            recovery: false,
            recovering: false,
            enabled: true,
            context_transition: false,
            edit_epoch: None,
            in_flight: 0,
            group_sync_pending: false,
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            status: String::new(),
            focus_handle: cx.focus_handle(),
            is_selecting: false,
            last_editor_bounds: None,
            last_line_layouts: Vec::new(),
            editor_scroll: ScrollState::new(),
            search_matches: Vec::new(),
            views: HashMap::new(),
            scroll_positions: HashMap::new(),
        };
        if poll {
            editor.submit(false, |_| Ok(()), cx);
        }
        if poll {
            cx.spawn(async move |this, cx| {
                loop {
                    Timer::after(Duration::from_secs(2)).await;
                    if this
                        .update(cx, |editor, cx| {
                            if editor.in_flight == 0 && !editor.recovery {
                                editor.submit_with_recovery(true, false, |_| Ok(()), cx);
                            }
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();
        }
        editor
    }

    pub(crate) fn submit(
        &mut self,
        preserve_input: bool,
        operation: impl FnOnce(&mut DocumentClient) -> Result<(), String> + Send + 'static,
        cx: &mut Context<Self>,
    ) -> bool {
        self.submit_with_recovery(preserve_input, preserve_input, operation, cx)
    }

    /// Queues an operation for this view's active document and records this host as creator of new tabs.
    ///
    /// Returns whether the worker accepted the request, not whether it succeeded.
    /// Disabled or recovery-blocked views reject it. Failures emit status events;
    /// `recover_on_error` retains the draft for recovery after an asynchronous failure.
    /// `preserve_input` preserves selection and composition when the document stays active.
    /// Only the latest submitted generation applies its response to this view.
    fn submit_with_recovery(
        &mut self,
        preserve_input: bool,
        recover_on_error: bool,
        operation: impl FnOnce(&mut DocumentClient) -> Result<(), String> + Send + 'static,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.enabled || self.context_transition {
            return false;
        }
        if self.recovery && !self.recovering {
            cx.emit(EditorEvent::Status(
                "入力の復旧が必要です。文書全体をコピーしてから再読み込みしてください".into(),
            ));
            return false;
        }
        let previous_generation = self.generation;
        self.generation += 1;
        let generation = self.generation;
        self.in_flight += 1;
        let recovering = self.recovering;
        let epoch = self.edit_epoch;
        let target = self.document_id.clone();
        let host = self.host;
        let admitted = self.worker.submit(
            cx,
            move |client| {
                if let Some(target) = target
                    && !client.editor.select_document_for_operation(&target)
                {
                    return Err("対象文書は既に閉じられています".into());
                }
                let epoch = epoch.unwrap_or_else(|| client.edit_chain.epoch());
                let permitted = if recovering {
                    Ok(())
                } else {
                    client.edit_chain.apply(epoch, || Ok(()))
                };
                let before = client
                    .editor
                    .tabs()
                    .into_iter()
                    .map(|tab| tab.id)
                    .collect::<Vec<_>>();
                let result = permitted.and_then(|_| operation(client));
                for tab in client.editor.tabs() {
                    if !before.contains(&tab.id) {
                        client.document_owners.insert(tab.id, host);
                    }
                }
                let current_epoch = client.edit_chain.epoch();
                let broken = client.edit_chain.apply(current_epoch, || Ok(())).is_err();
                Ok((result, broken, client.snapshot()))
            },
            move |editor, result, cx| {
                editor.in_flight = editor.in_flight.saturating_sub(1);
                if editor.generation != generation {
                    editor.notify_idle(cx);
                    return;
                }
                editor.recovering = false;
                let result = result.and_then(|(result, broken, snapshot)| {
                    if broken {
                        editor.recovery = true;
                    }
                    result?;
                    snapshot
                });
                let snapshot = match result {
                    Ok(snapshot) => snapshot,
                    Err(error) => {
                        editor.status = error.clone();
                        editor.recovery |= recover_on_error || recovering;
                        cx.emit(EditorEvent::Status(error));
                        cx.notify();
                        editor.notify_idle(cx);
                        return;
                    }
                };
                editor.recovery = false;
                editor.edit_epoch = Some(snapshot.edit_epoch);
                if recovering
                    && let Some(allowed) = &mut editor.only_documents
                    && let Some(id) = &snapshot.active
                    && !allowed.contains(id)
                {
                    allowed.push(id.clone());
                }
                let local = editor.local_snapshot(&snapshot);
                editor.accept_snapshot(&local, preserve_input, cx);
                cx.emit(EditorEvent::Snapshot(Box::new(snapshot)));
                cx.notify();
                editor.notify_idle(cx);
            },
        );
        if let Err(error) = admitted {
            self.generation = previous_generation;
            self.in_flight = self.in_flight.saturating_sub(1);
            self.status = error.clone();
            cx.emit(EditorEvent::Status(error));
            cx.notify();
            false
        } else {
            true
        }
    }

    /// Filters tabs by this host's inclusion and exclusion lists, retaining all document caches.
    ///
    /// Selects the snapshot's active tab if visible, then this view's active tab,
    /// then the first visible tab, or none when no tabs remain.
    pub(crate) fn local_snapshot(&self, snapshot: &ClientSnapshot) -> ClientSnapshot {
        let mut snapshot = snapshot.clone();
        snapshot.tabs.retain(|tab| {
            !self.excluded_documents.contains(&tab.id)
                && self
                    .only_documents
                    .as_ref()
                    .is_none_or(|ids| ids.contains(&tab.id))
        });
        snapshot.active = snapshot
            .active
            .clone()
            .filter(|id| snapshot.tabs.iter().any(|tab| &tab.id == id))
            .or_else(|| {
                self.document_id
                    .clone()
                    .filter(|id| snapshot.tabs.iter().any(|tab| &tab.id == id))
            })
            .or_else(|| snapshot.tabs.first().map(|tab| tab.id.clone()));
        for tab in &mut snapshot.tabs {
            tab.active = Some(&tab.id) == snapshot.active.as_ref();
        }
        snapshot
    }

    /// Refreshes an idle view from the group snapshot while preserving its local active tab if visible.
    /// Pending operations or draft recovery defer the refresh.
    pub(crate) fn receive_snapshot(&mut self, snapshot: &ClientSnapshot, cx: &mut Context<Self>) {
        self.group_sync_pending = self.has_pending();
        if !self.has_pending() {
            self.edit_epoch = Some(snapshot.edit_epoch);
            let mut snapshot = snapshot.clone();
            snapshot.active = self.document_id.clone();
            let local = self.local_snapshot(&snapshot);
            self.accept_snapshot(&local, true, cx);
            cx.notify();
        }
    }

    /// Workspace切替を通常の入力・Closeと同じ待ち処理の境界で保護する。
    pub(crate) fn begin_context_transition(&mut self) -> bool {
        if !self.enabled || self.has_pending() || self.recovery {
            return false;
        }
        self.generation += 1;
        self.context_transition = true;
        self.in_flight += 1;
        true
    }

    pub(crate) fn context_transition_pending(&self) -> bool {
        self.context_transition
    }

    pub(crate) fn finish_context_transition(
        &mut self,
        snapshot: Option<&ClientSnapshot>,
        cx: &mut Context<Self>,
    ) {
        if !self.context_transition {
            return;
        }
        self.context_transition = false;
        self.in_flight = self.in_flight.saturating_sub(1);
        if let Some(snapshot) = snapshot {
            self.edit_epoch = Some(snapshot.edit_epoch);
            self.accept_snapshot(snapshot, false, cx);
        }
        self.notify_idle(cx);
        cx.notify();
    }

    /// 古い世代の応答も含め、待ち処理の解消をHostへ通知する。
    fn notify_idle(&mut self, cx: &mut Context<Self>) {
        if self.in_flight == 0 && self.group_sync_pending {
            self.group_sync_pending = false;
            cx.emit(EditorEvent::Idle);
        }
    }

    /// Applies a snapshot unless its sequence is older than the last accepted one.
    ///
    /// Restores saved selection and scroll on document switches. `preserve_input`
    /// keeps selection and valid composition only when the active document is unchanged;
    /// selection endpoints are clamped to the refreshed document's character length.
    fn accept_snapshot(
        &mut self,
        snapshot: &ClientSnapshot,
        preserve_input: bool,
        cx: &mut Context<Self>,
    ) {
        if snapshot.sequence < self.snapshot_sequence {
            return;
        }
        self.snapshot_sequence = snapshot.sequence;
        for view in &snapshot.workspace_view.open_documents {
            if let Some(id) = &view.document_id {
                self.scroll_positions
                    .entry(id.clone())
                    .or_insert_with(|| gpui::point(px(view.scroll_x), px(view.scroll_y)));
                self.views.entry(id.clone()).or_insert((
                    view.selection_start..view.selection_end,
                    view.cursor_char == view.selection_start,
                ));
            }
        }
        if let Some(id) = &self.document_id {
            self.scroll_positions
                .insert(id.clone(), self.editor_scroll.handle().offset());
            self.views.insert(
                id.clone(),
                (self.selected_range.clone(), self.selection_reversed),
            );
        }
        let changed_document = self.document_id != snapshot.active;
        self.document_id = snapshot.active.clone();
        if changed_document {
            let offset = self
                .document_id
                .as_ref()
                .and_then(|id| self.scroll_positions.get(id))
                .copied()
                .unwrap_or_else(|| gpui::point(px(0.0), px(0.0)));
            self.editor_scroll.handle().set_offset(offset);
        }
        let previous_revision = self.session.revision();
        let previous_path = self.session.path().map(ToOwned::to_owned);
        self.session = snapshot
            .documents
            .iter()
            .find(|cache| Some(&cache.id) == snapshot.active.as_ref())
            .map(|cache| cache.document.clone())
            .unwrap_or_default();
        if changed_document
            || previous_revision != self.session.revision()
            || previous_path.as_deref() != self.session.path()
        {
            self.line_widths.refresh(&self.session);
            self.refresh_syntax(cx);
        }
        if changed_document || !preserve_input {
            let (range, reversed) = self
                .document_id
                .as_ref()
                .and_then(|id| self.views.get(id))
                .cloned()
                .unwrap_or((0..0, false));
            self.selected_range =
                range.start.min(self.session.len_chars())..range.end.min(self.session.len_chars());
            self.selection_reversed = reversed;
            self.marked_range = None;
            self.last_line_layouts.clear();
        } else {
            // Undo・Remote 短縮更新後に範囲外カーソルを残さない。
            let len = self.session.len_chars();
            self.selected_range =
                self.selected_range.start.min(len)..self.selected_range.end.min(len);
            self.marked_range = self.marked_range.take().filter(|range| range.end <= len);
            self.last_line_layouts.clear();
        }
        self.views
            .retain(|id, _| snapshot.documents.iter().any(|cache| &cache.id == id));
        self.scroll_positions
            .retain(|id, _| snapshot.documents.iter().any(|cache| &cache.id == id));
    }

    /// Optimistically replaces a range of character offsets and queues the shared-document edit.
    ///
    /// Returns false when disabled, without an active document, or for unchanged text.
    /// Invalid ranges propagate document errors; recovery or a missing edit epoch
    /// returns a conflict. Queue rejection restores the prior document and returns
    /// an I/O error. Later worker failures are reported through status and recovery.
    fn edit(
        &mut self,
        range: Range<usize>,
        text: &str,
        cx: &mut Context<Self>,
    ) -> Result<bool, DocumentError> {
        if !self.enabled || self.context_transition {
            return Ok(false);
        }
        let Some(id) = self.document_id.clone() else {
            return Ok(false);
        };
        if self.recovery {
            return Err(DocumentError::conflict("入力の復旧が必要です"));
        }
        let epoch = self
            .edit_epoch
            .ok_or_else(|| DocumentError::conflict("文書を読み込み中です"))?;
        let previous = self.session.clone();
        let syntax_edit = range.clone();
        let first_line = previous.char_to_position(range.start)?.line as usize;
        let last_line = previous.char_to_position(range.end)?.line as usize;
        let revision = self.session.revision();
        let changed = self.session.replace_char_range(range.clone(), text)?;
        if !changed {
            return Ok(false);
        }
        let text = text.to_owned();
        let admitted = self.submit(
            true,
            move |client| {
                let editor = &mut client.editor;
                client.edit_chain.apply(epoch, || {
                    if !editor.select_document_for_operation(&id) {
                        return Err("編集対象の文書は閉じられています".into());
                    }
                    if editor.active_revision() != revision {
                        return Err(
                            "文書が変更されました。入力を新しい文書へ退避してください".into()
                        );
                    }
                    editor
                        .replace_range(range, &text)
                        .map(|_| ())
                        .map_err(|error| error.to_string())
                })
            },
            cx,
        );
        if !admitted {
            self.session = previous;
            return Err(DocumentError::io("Clientの処理待ちが上限に達しました"));
        }
        {
            let new_end = (last_line + 1)
                .saturating_add(self.session.len_lines())
                .saturating_sub(previous.len_lines());
            self.line_widths.replace(
                first_line..last_line + 1,
                &self.session,
                first_line..new_end,
            );
        }
        if self.syntax_lines.as_mut().is_some_and(|lines| {
            syntax::rebase(lines, &previous, &self.session, syntax_edit).is_none()
        }) {
            self.syntax_lines = None;
        }
        self.schedule_syntax(cx);
        Ok(changed)
    }

    fn refresh_syntax(&mut self, cx: &mut Context<Self>) {
        self.syntax_lines = None;
        self.schedule_syntax(cx);
    }

    fn schedule_syntax(&mut self, cx: &mut Context<Self>) {
        self.syntax_generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.start_syntax(cx);
    }

    fn start_syntax(&mut self, cx: &mut Context<Self>) {
        use std::sync::atomic::Ordering;
        if self.syntax_busy {
            return;
        }
        let generation = self.syntax_generation.load(Ordering::Relaxed);
        let latest = self.syntax_generation.clone();
        let document = self.session.clone();
        let id = self.document_id.clone();
        let revision = document.revision();
        self.syntax_busy = true;
        let result = self.syntax_worker.submit(
            cx,
            move |cache| {
                let lines = cache.update(&document, generation, &latest);
                Ok(lines)
            },
            move |editor, result, cx| {
                editor.syntax_busy = false;
                if editor.syntax_generation.load(Ordering::Relaxed) != generation {
                    editor.start_syntax(cx);
                    return;
                }
                if editor.document_id == id && editor.session.revision() == revision {
                    if let Ok(lines) = result {
                        editor.syntax_lines = lines;
                    }
                    cx.notify();
                }
            },
        );
        if result.is_err() {
            self.syntax_busy = false;
        }
    }

    fn content_width(&self) -> f32 {
        (self.line_widths.max() + 80.0).max(600.0)
    }

    pub(crate) fn activate(&mut self, id: DocumentId, cx: &mut Context<Self>) {
        self.submit(
            false,
            move |client| {
                client.editor.activate_document(&id);
                Ok(())
            },
            cx,
        );
    }

    pub(crate) fn selected_text(&self) -> Option<String> {
        (!self.selected_range.is_empty())
            .then(|| self.session.slice_chars(self.selected_range.clone()).ok())
            .flatten()
    }

    fn line_start_char(&self, line: usize) -> Result<usize, DocumentError> {
        self.session.position_to_char(lapis_document::Position {
            line: line as u32,
            utf16_column: 0,
        })
    }

    pub(crate) fn cursor_line_column(&self) -> (usize, usize) {
        self.session
            .char_to_position(self.cursor_offset())
            .map(|position| {
                (
                    position.line as usize + 1,
                    position.utf16_column as usize + 1,
                )
            })
            .unwrap_or((1, 1))
    }

    pub(crate) fn recover_draft(&mut self, cx: &mut Context<Self>) {
        let text = self.session.content_for_save();
        self.recovering = true;
        if !self.submit(
            false,
            move |client| {
                client
                    .editor
                    .recover_draft(&text)
                    .map_err(|error| error.to_string())?;
                client.edit_chain.restart();
                Ok(())
            },
            cx,
        ) {
            self.recovering = false;
            self.recovery = true;
        }
    }
    pub(crate) fn document(&self) -> &Document {
        &self.session
    }

    /// Returns a tab's ID, character selection, cursor offset, and logical-pixel scroll offsets.
    /// Uses live state for the active tab, cached state otherwise, or none if no selection is cached.
    pub(crate) fn tab_view(
        &self,
        id: &DocumentId,
    ) -> Option<(DocumentId, Range<usize>, usize, f32, f32)> {
        if self.document_id.as_ref() == Some(id) {
            return self.view_state();
        }
        let (range, reversed) = self.views.get(id)?;
        let offset = self.scroll_positions.get(id).copied().unwrap_or_default();
        Some((
            id.clone(),
            range.clone(),
            if *reversed { range.start } else { range.end },
            offset.x.into(),
            offset.y.into(),
        ))
    }

    /// Caches a transferred tab's character selection, cursor direction, and logical-pixel scroll offsets.
    /// The cached view takes effect when that tab is selected.
    pub(crate) fn seed_view(&mut self, view: (DocumentId, Range<usize>, usize, f32, f32)) {
        let (id, selection, cursor, x, y) = view;
        self.views
            .insert(id.clone(), (selection.clone(), cursor == selection.start));
        self.scroll_positions.insert(id, gpui::point(px(x), px(y)));
    }

    /// Selects an incoming document from the group snapshot, restoring its cached view state.
    pub(crate) fn select_transferred(
        &mut self,
        snapshot: &ClientSnapshot,
        id: DocumentId,
        cx: &mut Context<Self>,
    ) {
        let mut snapshot = self.local_snapshot(snapshot);
        snapshot.active = Some(id);
        self.accept_snapshot(&snapshot, false, cx);
        cx.notify();
    }
    pub(crate) fn has_pending(&self) -> bool {
        self.in_flight > 0 || self.recovery
    }
    /// Reports outstanding worker requests, excluding draft recovery without a request.
    pub(crate) fn has_in_flight(&self) -> bool {
        self.in_flight > 0
    }
    pub(crate) fn needs_recovery(&self) -> bool {
        self.recovery
    }
    pub(crate) fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.enabled = enabled;
        cx.notify();
    }
    pub(crate) fn view_state(&self) -> Option<(DocumentId, Range<usize>, usize, f32, f32)> {
        let offset = self.editor_scroll.handle().offset();
        self.document_id.clone().map(|id| {
            (
                id,
                self.selected_range.clone(),
                self.cursor_offset(),
                f32::from(offset.x),
                f32::from(offset.y),
            )
        })
    }
    pub(crate) fn document_id(&self) -> Option<DocumentId> {
        self.document_id.clone()
    }
}

impl Focusable for Editor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Editor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = div()
            .relative()
            .min_w_full()
            .min_h_full()
            .w(px(self.content_width()))
            .px(px(18.0))
            .py(px(10.0))
            .cursor(CursorStyle::IBeam)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::editor_mouse_down))
            .on_mouse_move(cx.listener(Self::editor_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::editor_mouse_up))
            .child(EditorElement {
                editor: cx.entity(),
            });
        div()
            .size_full()
            .flex()
            .flex_col()
            .when(self.recovery, |view| {
                view.child(
                    div()
                        .flex()
                        .gap_1()
                        .child(self.status.clone())
                        .child(
                            crate::components::action_button("Copy draft", false).on_click(
                                cx.listener(|this, _, _, cx| {
                                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                        this.session.content_for_save(),
                                    ))
                                }),
                            ),
                        )
                        .child(
                            crate::components::action_button("Recover as new document", true)
                                .on_click(cx.listener(|this, _, _, cx| this.recover_draft(cx))),
                        ),
                )
            })
            .track_focus(&self.focus_handle)
            .key_context("Editor")
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::enter))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::find))
            .on_action(cx.listener(Self::find_next))
            .on_action(cx.listener(Self::find_workspace))
            .on_action(cx.listener(Self::complete))
            .on_action(cx.listener(Self::go_to_definition))
            .child(
                scroll_viewport(
                    "source-scroll",
                    ScrollAxis::Both,
                    &self.editor_scroll,
                    content,
                )
                .size_full(),
            )
    }
}
