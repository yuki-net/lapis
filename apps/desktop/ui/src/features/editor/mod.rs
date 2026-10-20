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
mod interactions;
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
            id::COMMAND_TOGGLE_BOTTOM,
            "command.toggle-bottom-panel",
            "bottom-panel",
            50,
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
    Status(String),
    WorkspaceSearch(String),
    Completion(bool),
}

pub(crate) struct Editor {
    session: Document,
    document_id: Option<DocumentId>,
    worker: ClientWorker<DocumentClient>,
    generation: u64,
    recovery: bool,
    recovering: bool,
    enabled: bool,
    edit_epoch: Option<crate::client_adapter::EditEpoch>,
    in_flight: usize,
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

impl Editor {
    pub(crate) fn new(worker: ClientWorker<DocumentClient>, cx: &mut Context<Self>) -> Self {
        let mut editor = Self {
            session: Document::new(),
            document_id: None,
            worker,
            generation: 0,
            recovery: false,
            recovering: false,
            enabled: true,
            edit_epoch: None,
            in_flight: 0,
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
        editor.submit(false, |_| Ok(()), cx);
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

    fn submit_with_recovery(
        &mut self,
        preserve_input: bool,
        recover_on_error: bool,
        operation: impl FnOnce(&mut DocumentClient) -> Result<(), String> + Send + 'static,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.enabled {
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
        let admitted = self.worker.submit(
            cx,
            move |client| {
                let epoch = epoch.unwrap_or_else(|| client.edit_chain.epoch());
                let permitted = if recovering {
                    Ok(())
                } else {
                    client.edit_chain.apply(epoch, || Ok(()))
                };
                let result = permitted.and_then(|_| operation(client));
                let current_epoch = client.edit_chain.epoch();
                let broken = client.edit_chain.apply(current_epoch, || Ok(())).is_err();
                Ok((result, broken, client.snapshot()))
            },
            move |editor, result, cx| {
                editor.in_flight = editor.in_flight.saturating_sub(1);
                if editor.generation != generation {
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
                        return;
                    }
                };
                editor.recovery = false;
                editor.edit_epoch = Some(snapshot.edit_epoch);
                for view in &snapshot.workspace_view.open_documents {
                    if let Some(id) = &view.document_id {
                        editor
                            .scroll_positions
                            .entry(id.clone())
                            .or_insert_with(|| gpui::point(px(view.scroll_x), px(view.scroll_y)));
                        editor.views.entry(id.clone()).or_insert((
                            view.selection_start..view.selection_end,
                            view.cursor_char == view.selection_start,
                        ));
                    }
                }
                if let Some(id) = &editor.document_id {
                    editor
                        .scroll_positions
                        .insert(id.clone(), editor.editor_scroll.handle().offset());
                    editor.views.insert(
                        id.clone(),
                        (editor.selected_range.clone(), editor.selection_reversed),
                    );
                }
                let changed_document = editor.document_id != snapshot.active;
                editor.document_id = snapshot.active.clone();
                if changed_document {
                    let offset = editor
                        .document_id
                        .as_ref()
                        .and_then(|id| editor.scroll_positions.get(id))
                        .copied()
                        .unwrap_or_else(|| gpui::point(px(0.0), px(0.0)));
                    editor.editor_scroll.handle().set_offset(offset);
                }
                editor.session = snapshot
                    .documents
                    .iter()
                    .find(|cache| Some(&cache.id) == snapshot.active.as_ref())
                    .map(|cache| cache.document.clone())
                    .unwrap_or_default();
                if changed_document || !preserve_input {
                    let (range, reversed) = editor
                        .document_id
                        .as_ref()
                        .and_then(|id| editor.views.get(id))
                        .cloned()
                        .unwrap_or((0..0, false));
                    editor.selected_range = range.start.min(editor.session.len_chars())
                        ..range.end.min(editor.session.len_chars());
                    editor.selection_reversed = reversed;
                    editor.marked_range = None;
                    editor.last_line_layouts.clear();
                } else {
                    // Undo・Remote 短縮更新後に範囲外カーソルを残さない。
                    let len = editor.session.len_chars();
                    editor.selected_range =
                        editor.selected_range.start.min(len)..editor.selected_range.end.min(len);
                    editor.marked_range =
                        editor.marked_range.take().filter(|range| range.end <= len);
                    editor.last_line_layouts.clear();
                }
                cx.emit(EditorEvent::Snapshot(Box::new(snapshot)));
                cx.notify();
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

    fn edit(
        &mut self,
        range: Range<usize>,
        text: &str,
        cx: &mut Context<Self>,
    ) -> Result<bool, DocumentError> {
        let Some(id) = self.document_id.clone() else {
            return Ok(false);
        };
        if self.recovery {
            return Err(DocumentError::conflict("入力の復旧が必要です"));
        }
        let previous = self.session.clone();
        let revision = self.session.revision();
        let changed = self.session.replace_char_range(range.clone(), text)?;
        if !changed {
            return Ok(false);
        }
        let text = text.to_owned();
        let epoch = self
            .edit_epoch
            .ok_or_else(|| DocumentError::conflict("文書を読み込み中です"))?;
        let admitted = self.submit(
            true,
            move |client| {
                let editor = &mut client.editor;
                client.edit_chain.apply(epoch, || {
                    if !editor.activate_document(&id) {
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
        Ok(changed)
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
    pub(crate) fn has_pending(&self) -> bool {
        self.in_flight > 0 || self.recovery
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
            .w(px((0..self.session.len_lines())
                .filter_map(|line| self.session.line(line))
                .map(|line| line.chars().count())
                .max()
                .unwrap_or(0) as f32
                * 18.0
                + 80.0))
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
