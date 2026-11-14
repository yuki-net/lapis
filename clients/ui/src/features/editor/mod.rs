use crate::timer::Timer;
use std::{collections::HashMap, ops::Range, time::Duration};

use gpui::{
    App, Bounds, Context, CursorStyle, Element, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, Focusable, GlobalElementId, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, Render, ShapedLine,
    SharedString, Style, TextRun, UTF16Selection, Window, div, fill, point, prelude::*, px, size,
};

use crate::{
    app::*,
    client_adapter::{ClientSnapshot, ClientWorker, DocumentClient},
    components::{ScrollAxis, ScrollState, scroll_viewport},
    features::id,
    theme,
};
use document::{Document, DocumentConnection, DocumentError};
use editor_core::DocumentId;
mod actions;
mod canvas;
mod input;
mod interactions;
mod syntax;
mod widths;
use canvas::{EditorElement, EditorLineLayout};

pub(super) fn descriptor() -> extension_ui_contract::FeatureDescriptor {
    use extension_ui_contract::UiContribution;

    extension_ui_contract::FeatureDescriptor::core(id::FEATURE_EDITOR)
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

pub(super) fn rust_descriptor() -> extension_ui_contract::FeatureDescriptor {
    extension_ui_contract::FeatureDescriptor::bundled(
        id::FEATURE_RUST,
        [extension_ui_contract::ActivationCondition::OnLanguage(
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
    /// 入力中で反映できなかった共有の active 文書。次の snapshot 受理で反映する。
    /// snapshot 全体ではなく選択だけを持つことで、より新しい snapshot に追い越されない。
    shared_active: DeferredSharedActive,
    sync_pending: bool,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    status: String,
    owner: Option<editor_core::SessionId>,
    is_owner: bool,
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

/// 入力中に保留した共有の active。
///
/// snapshot 全体を保留すると、入力完了時に届くより新しい snapshot に追い越されて
/// `accept_snapshot` の sequence 検査で拒否され、共有の active が失われる。
/// 選択だけを保留し、次に受理する snapshot の active を差し替える。
#[derive(Default)]
struct DeferredSharedActive(Option<DocumentId>);

impl DeferredSharedActive {
    /// 共有の active を保留する。入力中は本文も選択も変えない。
    fn defer(&mut self, id: Option<DocumentId>) {
        self.0 = id;
    }

    /// ユーザーが明示的に選んだので保留を捨てる。
    ///
    /// 後からの明示的なタブ選択は共有より優先する。捨てないと、その選択を
    /// 反映した snapshot を受理するときに保留中の active が上書きしてしまう。
    fn explicit_selection(&mut self) {
        self.0 = None;
    }

    /// 受理する snapshot の active に保留を反映する。共有側で閉じられた文書は選ばない。
    fn apply_to(&mut self, active: &mut Option<DocumentId>, tabs: &[desktop_client::DocumentTab]) {
        let Some(id) = self.0.take() else {
            return;
        };
        if tabs.iter().any(|tab| tab.id == id) {
            *active = Some(id);
        }
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        self.syntax_generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Editor {
    /// Host の変更通知だけを契機に同期する Editor を作る。
    /// 通知が無い接続では `connection` が `None` になり、明示操作だけで更新する。
    pub(crate) fn new_with_subscription(
        worker: ClientWorker<DocumentClient>,
        connection: Option<std::sync::Arc<dyn DocumentConnection>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let editor = Self::new_with_initial_snapshot(worker, connection.is_none(), cx);
        if let Some(connection) = connection {
            editor.start_subscription(connection, cx);
        }
        editor
    }

    /// Creates a local editor view over an existing worker.
    ///
    /// With `initial_snapshot`, requests one snapshot immediately. Otherwise,
    /// snapshots must be supplied by the host.
    pub(crate) fn new_with_initial_snapshot(
        worker: ClientWorker<DocumentClient>,
        initial_snapshot: bool,
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
            shared_active: DeferredSharedActive::default(),
            sync_pending: false,
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            status: String::new(),
            owner: None,
            is_owner: false,
            focus_handle: cx.focus_handle(),
            is_selecting: false,
            last_editor_bounds: None,
            last_line_layouts: Vec::new(),
            editor_scroll: ScrollState::new(),
            search_matches: Vec::new(),
            views: HashMap::new(),
            scroll_positions: HashMap::new(),
        };
        if initial_snapshot {
            editor.submit(false, |_| Ok(()), cx);
        }
        editor
    }

    /// Host の変更通知で同期する。初回と再接続時は1度取り直す。
    fn start_subscription(
        &self,
        connection: std::sync::Arc<dyn DocumentConnection>,
        cx: &mut Context<Self>,
    ) {
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            loop {
                let Some(mut subscription) = connection.subscribe() else {
                    // 購読できない接続でも初回取得は行う。
                    let _ = this.update(cx, |editor, cx| editor.request_sync(cx));
                    return;
                };
                // 購読開始前の変更を回収する。
                if this
                    .update(cx, |editor, cx| editor.request_sync(cx))
                    .is_err()
                {
                    return;
                }
                loop {
                    let (next, outcome) = executor
                        .spawn(async move {
                            let outcome = subscription.recv_timeout(Duration::from_secs(1));
                            (subscription, outcome)
                        })
                        .await;
                    subscription = next;
                    match outcome {
                        Ok(true) => {
                            if this
                                .update(cx, |editor, cx| editor.request_sync(cx))
                                .is_err()
                            {
                                return;
                            }
                        }
                        Ok(false) => {}
                        // 通知の欠落・切断。再同期してから購読し直す。
                        Err(_) => break,
                    }
                }
                if this
                    .update(cx, |editor, cx| editor.request_sync(cx))
                    .is_err()
                {
                    return;
                }
                Timer::after(Duration::from_secs(2)).await;
            }
        })
        .detach();
    }

    /// 未確定の操作中は完了後に1回だけ同期する。
    pub(crate) fn request_sync(&mut self, cx: &mut Context<Self>) {
        if self.recovery {
            return;
        }
        if self.in_flight > 0 {
            self.sync_pending = true;
            return;
        }
        if !self.submit_with_recovery(true, false, |_| Ok(()), cx) {
            self.sync_pending = true;
        }
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

    /// 共有された active 文書を反映する。
    ///
    /// 共有 view の適用ではタブ構成と選択中の文書が変わるため、`receive_snapshot`
    /// のように現在の文書を優先せず、snapshot の active をそのまま採用する。
    /// 未確定の入力がある間は active だけを保留し、後の snapshot 受理で反映する。
    /// snapshot 全体を保留すると、入力完了時の新しい snapshot に追い越されて
    /// `accept_snapshot` に拒否され、共有の active が失われるため。
    pub(crate) fn receive_shared_snapshot(
        &mut self,
        snapshot: &ClientSnapshot,
        cx: &mut Context<Self>,
    ) {
        if self.has_pending() {
            // Idle 後の再適用でも共有の active を失わないよう、選択だけを保持する。
            // 本文は置き換えない。
            self.shared_active.defer(snapshot.active.clone());
            self.group_sync_pending = true;
            return;
        }
        self.shared_active.explicit_selection();
        self.group_sync_pending = false;
        self.edit_epoch = Some(snapshot.edit_epoch);
        let local = self.local_snapshot(snapshot);
        self.accept_snapshot(&local, true, cx);
        cx.notify();
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
        if self.in_flight == 0 && self.sync_pending && !self.recovery {
            self.sync_pending = false;
            if !self.submit_with_recovery(true, false, |_| Ok(()), cx) {
                self.sync_pending = true;
            }
        }
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
        let mut snapshot = snapshot.clone();
        self.shared_active
            .apply_to(&mut snapshot.active, &snapshot.tabs);
        let snapshot = &snapshot;
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
        let active_cache = snapshot
            .documents
            .iter()
            .find(|cache| Some(&cache.id) == snapshot.active.as_ref());
        self.owner = active_cache.and_then(|cache| cache.owner.clone());
        self.is_owner = active_cache.is_some_and(|cache| cache.is_owner);
        self.session = active_cache
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

    /// 他の Session が編集権を持ち、入力を受け付けない状態か。
    pub(crate) fn is_read_only(&self) -> bool {
        self.owner.is_some() && !self.is_owner
    }

    /// 編集権を取得する。他の Session が所有中なら奪う。
    pub(crate) fn claim_lease(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.document_id.clone() else {
            return;
        };
        self.submit(
            false,
            move |client| {
                if !client.editor.select_document_for_operation(&id) {
                    return Err("編集対象の文書は閉じられています".into());
                }
                client
                    .editor
                    .claim_active()
                    .map_err(|error| error.to_string())
            },
            cx,
        );
    }

    /// 編集権を手放す。
    pub(crate) fn release_lease(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.document_id.clone() else {
            return;
        };
        self.submit(
            false,
            move |client| {
                if !client.editor.select_document_for_operation(&id) {
                    return Err("編集対象の文書は閉じられています".into());
                }
                client
                    .editor
                    .release_active()
                    .map_err(|error| error.to_string())
            },
            cx,
        );
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
        if self.is_read_only() {
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
        let submitted = self.submit(
            false,
            move |client| {
                client.editor.activate_document(&id);
                Ok(())
            },
            cx,
        );
        // 選択を受け付けたときだけ、共有の active の保留を捨てる。復旧待ちなどで
        // 受け付けられなかった場合は選択自体が起きないため、共有の active を
        // 残しておかないと、復旧後の snapshot でも反映されなくなる。
        if submitted {
            self.shared_active.explicit_selection();
        }
    }

    pub(crate) fn selected_text(&self) -> Option<String> {
        (!self.selected_range.is_empty())
            .then(|| self.session.slice_chars(self.selected_range.clone()).ok())
            .flatten()
    }

    fn line_start_char(&self, line: usize) -> Result<usize, DocumentError> {
        self.session.position_to_char(document::Position {
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
        // 転送されたタブの選択は明示的な選択として扱う。
        self.shared_active.explicit_selection();
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
        let read_only = self.is_read_only();
        let lease = self.owner.clone().map(|owner| {
            let is_owner = self.is_owner;
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py_1()
                .bg(theme::colors(cx).background_tertiary)
                .text_size(px(11.0))
                .text_color(theme::colors(cx).text_secondary)
                .child(if is_owner {
                    format!("編集中: {owner}（このSession）")
                } else {
                    format!("編集中: {owner}")
                })
                .when(is_owner, |bar| {
                    bar.child(
                        crate::components::action_button("編集権を解放", false, cx)
                            .on_click(cx.listener(|this, _, _, cx| this.release_lease(cx))),
                    )
                })
                .when(!is_owner, |bar| {
                    bar.child(
                        crate::components::action_button("編集権を取得", true, cx)
                            .on_click(cx.listener(|this, _, _, cx| this.claim_lease(cx))),
                    )
                })
        });
        let content = div()
            .relative()
            .min_w_full()
            .min_h_full()
            .w(px(self.content_width()))
            .py(px(10.0))
            .cursor(if read_only {
                CursorStyle::Arrow
            } else {
                CursorStyle::IBeam
            })
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
                            crate::components::action_button("Copy draft", false, cx).on_click(
                                cx.listener(|this, _, _, cx| {
                                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                        this.session.content_for_save(),
                                    ))
                                }),
                            ),
                        )
                        .child(
                            crate::components::action_button("Recover as new document", true, cx)
                                .on_click(cx.listener(|this, _, _, cx| this.recover_draft(cx))),
                        ),
                )
            })
            .when_some(lease, |view, lease| view.child(lease))
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
                    cx,
                )
                .size_full(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{DeferredSharedActive, Editor};

    /// `EditorEvent::Idle` を数えるための購読先。
    struct Observer {
        _subscription: gpui::Subscription,
    }
    use crate::client_adapter::{ClientSnapshot, ClientWorker};
    use editor_core::DocumentId;
    use gpui::prelude::*;

    fn tab(id: &str) -> desktop_client::DocumentTab {
        desktop_client::DocumentTab {
            id: DocumentId::new(id),
            display_name: id.to_owned(),
            path: None,
            dirty: false,
            external_change: document::ExternalChange::Unchanged,
            encoding: document::Encoding::Utf8,
            active: false,
        }
    }

    fn tabs(ids: &[&str]) -> Vec<desktop_client::DocumentTab> {
        ids.iter().map(|id| tab(id)).collect()
    }

    /// 入力完了時に届いた新しい snapshot が共有の active を追い越しても、
    /// 保留した選択を反映して共有の active を失わない。
    #[test]
    fn a_deferred_shared_active_survives_a_newer_snapshot() {
        let tabs = tabs(&["a", "b"]);
        // 入力中は B を保留したまま、snapshot は受理しない。
        let mut shared = DeferredSharedActive::default();
        shared.defer(Some(DocumentId::new("b")));
        // 入力完了後に届いた新しい snapshot は A を選んでいる。
        let mut active = Some(DocumentId::new("a"));
        shared.apply_to(&mut active, &tabs);
        assert_eq!(active, Some(DocumentId::new("b")));
        // 反映した保留は消費し、次の snapshot では上書きしない。
        let mut next = Some(DocumentId::new("a"));
        shared.apply_to(&mut next, &tabs);
        assert_eq!(next, Some(DocumentId::new("a")));
    }

    /// 後からの明示的なタブ選択は共有の active より優先する。
    #[test]
    fn an_explicit_selection_wins_over_a_deferred_shared_active() {
        let tabs = tabs(&["a", "b", "c"]);
        let mut shared = DeferredSharedActive::default();
        shared.defer(Some(DocumentId::new("b")));
        // 入力の応答前にユーザーが C を選ぶ。
        shared.explicit_selection();
        let mut active = Some(DocumentId::new("c"));
        shared.apply_to(&mut active, &tabs);
        assert_eq!(active, Some(DocumentId::new("c")));
    }

    /// 共有側で閉じられた文書は選ばず、保留も捨てる。
    #[test]
    fn a_deferred_shared_active_ignores_a_closed_document() {
        let tabs = tabs(&["a"]);
        let mut shared = DeferredSharedActive::default();
        shared.defer(Some(DocumentId::new("b")));
        let mut active = Some(DocumentId::new("a"));
        shared.apply_to(&mut active, &tabs);
        assert_eq!(active, Some(DocumentId::new("a")));
        // 閉じられた文書の保留は残さない。
        let mut next = Some(DocumentId::new("a"));
        shared.apply_to(&mut next, &tabs);
        assert_eq!(next, Some(DocumentId::new("a")));
    }

    /// 保留が無ければ active を変えない。
    #[test]
    fn no_deferred_shared_active_keeps_the_snapshot_active() {
        let tabs = tabs(&["a", "b"]);
        let mut shared = DeferredSharedActive::default();
        let mut active = Some(DocumentId::new("a"));
        shared.apply_to(&mut active, &tabs);
        assert_eq!(active, Some(DocumentId::new("a")));
    }

    /// 本文を書き込んだ 3 つの文書を持つ Client と Editor を用意する。
    fn fixture(cx: &mut gpui::TestAppContext) -> (gpui::Entity<Editor>, ClientSnapshot) {
        use document::DocumentConnection;

        let port = crate::client_adapter::tests::Fixture::default();
        let mut client = port.client();
        let opened = client.reconcile_snapshot();
        let ids = opened
            .tabs
            .iter()
            .map(|tab| tab.id.clone())
            .collect::<Vec<_>>();
        for (index, text) in ["fn first() {}", "fn second() {}", "fn third() {}"]
            .into_iter()
            .enumerate()
        {
            let revision = opened.documents[index].document.revision();
            port.edit(&ids[index], revision, 0..0, text)
                .expect("本文を書き込める");
        }
        let opened = client.reconcile_snapshot();
        let worker = ClientWorker::new(client);
        let editor =
            cx.update(|cx| cx.new(|cx| Editor::new_with_initial_snapshot(worker, false, cx)));
        // 直前の new_document で最後の文書が選ばれているため、先頭を表示しておく。
        let mut opened = opened;
        opened.active = opened.tabs.first().map(|tab| tab.id.clone());
        (editor, opened)
    }

    fn body(editor: &gpui::Entity<Editor>, cx: &mut gpui::TestAppContext) -> String {
        editor.read_with(cx, |editor, _| {
            let len = editor.document().len_chars();
            editor.document().slice_chars(0..len).unwrap_or_default()
        })
    }

    fn show(
        editor: &gpui::Entity<Editor>,
        snapshot: &ClientSnapshot,
        cx: &mut gpui::TestAppContext,
    ) {
        let snapshot = snapshot.clone();
        editor.update(cx, |editor, cx| {
            editor.accept_snapshot(&snapshot, false, cx)
        });
    }

    /// 入力中に共有の active を受け、入力完了時に新しい snapshot を受理しても、
    /// 共有の active の本文へ切り替わる。
    #[gpui::test]
    fn a_shared_active_is_applied_after_the_pending_input_completes(cx: &mut gpui::TestAppContext) {
        let (editor, opened) = fixture(cx);
        let first = opened.tabs[0].id.clone();
        let second = opened.tabs[1].id.clone();
        show(&editor, &opened, cx);
        assert_eq!(body(&editor, cx), "fn first() {}");

        // 入力中に共有側が 2 番目を選ぶ。本文はまだ置き換えない。
        let mut shared = opened.clone();
        shared.sequence += 1;
        shared.active = Some(second.clone());
        editor.update(cx, |editor, cx| {
            editor.in_flight = 1;
            editor.receive_shared_snapshot(&shared, cx);
        });
        assert_eq!(body(&editor, cx), "fn first() {}");

        // 入力が完了し、より新しい snapshot（1 番目を選ぶ）を受理する。
        let mut newer = opened.clone();
        newer.sequence += 2;
        newer.active = Some(first);
        editor.update(cx, |editor, cx| {
            editor.in_flight = 0;
            editor.accept_snapshot(&newer, true, cx);
        });
        // 保留した共有の active を反映し、本文もそちらへ切り替わる。
        assert_eq!(
            editor.read_with(cx, |editor, _| editor.document_id()),
            Some(second)
        );
        assert_eq!(body(&editor, cx), "fn second() {}");
    }

    /// 実際の入力完了と `EditorEvent::Idle` を通しても、共有の active が反映される。
    ///
    /// worker の応答で `in_flight` が減り、`notify_idle` が `Idle` を出す経路を
    /// そのまま通す。保留した共有の active は、その完了時の snapshot 受理で反映する。
    #[gpui::test]
    fn a_pending_input_completion_applies_the_shared_active(cx: &mut gpui::TestAppContext) {
        let (editor, opened) = fixture(cx);
        let first = opened.tabs[0].id.clone();
        let second = opened.tabs[1].id.clone();
        show(&editor, &opened, cx);
        let (idles, _observer) = subscribe_idle(&editor, cx);

        // 入力中に共有側が 2 番目を選ぶ。本文はまだ置き換えない。
        let mut shared = opened.clone();
        shared.sequence += 1;
        shared.active = Some(second.clone());
        let completed = editor.update(cx, |editor, cx| {
            let submitted = editor.submit(
                false,
                {
                    let first = first.clone();
                    move |client| {
                        // 入力の応答はユーザーが表示していた文書を選んだ状態で返る。
                        client.editor.activate_document(&first);
                        Ok(())
                    }
                },
                cx,
            );
            assert!(submitted, "入力を投入できる");
            editor.receive_shared_snapshot(&shared, cx);
            editor.has_pending()
        });
        assert!(completed, "入力中は共有の active を保留する");
        assert_eq!(body(&editor, cx), "fn first() {}");

        // 実際の応答を待つ。完了時に `Idle` が出る。
        run_until_idle(cx, &idles);
        assert_eq!(
            editor.read_with(cx, |editor, _| editor.document_id()),
            Some(second),
            "入力完了後も共有の active を反映する"
        );
        assert_eq!(body(&editor, cx), "fn second() {}");
    }

    /// 実際の入力完了と `Idle` を通しても、後からの明示的な選択が優先される。
    #[gpui::test]
    fn an_explicit_selection_survives_the_input_completion(cx: &mut gpui::TestAppContext) {
        let (editor, opened) = fixture(cx);
        let first = opened.tabs[0].id.clone();
        let second = opened.tabs[1].id.clone();
        let third = opened.tabs[2].id.clone();
        show(&editor, &opened, cx);
        let (idles, _observer) = subscribe_idle(&editor, cx);

        // 入力中に共有側が 2 番目を選ぶ。
        let mut shared = opened.clone();
        shared.sequence += 1;
        shared.active = Some(second);
        editor.update(cx, |editor, cx| {
            let submitted = editor.submit(
                false,
                {
                    let first = first.clone();
                    move |client| {
                        client.editor.activate_document(&first);
                        Ok(())
                    }
                },
                cx,
            );
            assert!(submitted, "入力を投入できる");
            editor.receive_shared_snapshot(&shared, cx);
            // 応答前にユーザーが 3 番目を明示的に選ぶ。
            editor.activate(third.clone(), cx);
        });

        run_until_idle(cx, &idles);
        assert_eq!(
            editor.read_with(cx, |editor, _| editor.document_id()),
            Some(third),
            "明示的な選択が共有の active に置き換わらない"
        );
        assert_eq!(body(&editor, cx), "fn third() {}");
    }

    /// `EditorEvent::Idle` の回数を数える購読と、購読を生かすための observer を返す。
    fn subscribe_idle(
        editor: &gpui::Entity<Editor>,
        cx: &mut gpui::TestAppContext,
    ) -> (std::rc::Rc<std::cell::Cell<usize>>, gpui::Entity<Observer>) {
        let idles = std::rc::Rc::new(std::cell::Cell::new(0));
        let counted = idles.clone();
        let editor = editor.clone();
        let observer = cx.update(|cx| {
            cx.new(|cx| {
                let subscription = cx.subscribe(&editor, move |_: &mut Observer, _, event, _| {
                    if matches!(event, super::EditorEvent::Idle) {
                        counted.set(counted.get() + 1);
                    }
                });
                Observer {
                    _subscription: subscription,
                }
            })
        });
        (idles, observer)
    }

    /// worker の応答が返り、`Idle` が出るまで進める。
    fn run_until_idle(cx: &mut gpui::TestAppContext, idles: &std::rc::Rc<std::cell::Cell<usize>>) {
        let deadline = crate::time::Instant::now() + std::time::Duration::from_secs(5);
        while idles.get() == 0 {
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(20));
            cx.run_until_parked();
            assert!(crate::time::Instant::now() < deadline, "Idle が来ない");
            std::thread::yield_now();
        }
    }

    /// 選択が受け付けられなかったときは、共有の active の保留を残す。    /// 選択が受け付けられなかったときは、共有の active の保留を残す。
    ///
    /// 復旧待ちで `submit` が拒否する状況ではタブ選択自体が起きないため、
    /// 保留を捨てると復旧後の snapshot でも共有の active が反映されない。
    #[gpui::test]
    fn a_rejected_selection_keeps_the_deferred_shared_active(cx: &mut gpui::TestAppContext) {
        let (editor, opened) = fixture(cx);
        let second = opened.tabs[1].id.clone();
        let third = opened.tabs[2].id.clone();
        show(&editor, &opened, cx);

        // 入力中に共有側が 2 番目を選ぶ。
        let mut shared = opened.clone();
        shared.sequence += 1;
        shared.active = Some(second.clone());
        editor.update(cx, |editor, cx| {
            editor.in_flight = 1;
            editor.receive_shared_snapshot(&shared, cx);
        });
        // 復旧待ちで選択を受け付けない。
        editor.update(cx, |editor, cx| {
            editor.in_flight = 0;
            editor.recovery = true;
            editor.activate(third, cx);
        });
        assert!(
            editor.read_with(cx, |editor, _| editor.has_pending()),
            "復旧待ちは選択を受け付けない"
        );

        // 復旧後の snapshot を受理すると、保留した共有の active を反映する。
        let mut newer = opened.clone();
        newer.sequence += 2;
        newer.active = Some(opened.tabs[0].id.clone());
        editor.update(cx, |editor, cx| {
            editor.recovery = false;
            editor.accept_snapshot(&newer, true, cx);
        });
        assert_eq!(
            editor.read_with(cx, |editor, _| editor.document_id()),
            Some(second)
        );
    }

    /// 入力中に共有の active を保留していても、後からの明示的な選択が優先される。
    #[gpui::test]
    fn an_explicit_selection_is_not_replaced_by_a_deferred_shared_active(
        cx: &mut gpui::TestAppContext,
    ) {
        let (editor, opened) = fixture(cx);
        let second = opened.tabs[1].id.clone();
        let third = opened.tabs[2].id.clone();
        show(&editor, &opened, cx);

        // 入力中に共有側が 2 番目を選ぶ。
        let mut shared = opened.clone();
        shared.sequence += 1;
        shared.active = Some(second);
        editor.update(cx, |editor, cx| {
            editor.in_flight = 1;
            editor.receive_shared_snapshot(&shared, cx);
        });
        // 応答前にユーザーが 3 番目を明示的に選ぶ。
        let selected = third.clone();
        editor.update(cx, |editor, cx| editor.activate(selected, cx));

        // その明示的な選択を反映した応答 snapshot を受理する。
        let mut newer = opened.clone();
        newer.sequence += 2;
        newer.active = Some(third.clone());
        editor.update(cx, |editor, cx| {
            editor.in_flight = 0;
            editor.accept_snapshot(&newer, true, cx);
        });

        // 明示的な選択が共有の active に置き換わらない。
        assert_eq!(
            editor.read_with(cx, |editor, _| editor.document_id()),
            Some(third)
        );
        assert_eq!(body(&editor, cx), "fn third() {}");
    }
}
