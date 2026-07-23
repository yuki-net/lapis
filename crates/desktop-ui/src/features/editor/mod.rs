use std::{
    ops::Range,
    time::{Duration, Instant},
};

use gpui::{
    App, Bounds, Context, CursorStyle, Element, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, Focusable, GlobalElementId, KeyDownEvent, LayoutId,
    ModifiersChangedEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad,
    Pixels, Point, Render, ScrollHandle, ShapedLine, SharedString, Style, TextRun, Timer,
    UTF16Selection, Window, WindowControlArea, div, fill, point, prelude::*, px, relative, rgb,
    rgba, size,
};
use lapis_app_services::{ConversationViewState, DocumentAction, EditorSession};
use lapis_editor_core::{ExecutionId, TaskId};
use lapis_git::ChangeKind;
use lapis_lsp::LspPosition;
use lapis_task_runner::{ExecutionStatus, TaskControl, TaskMode};
use lapis_workspace::FileEntryKind;

use crate::{
    app::*,
    components::{panel_empty_state, panel_tab, tool_empty_state, tool_tab},
    extension_ui::{ActivationEvent, FeatureRegistry, UiSlot, ViewId},
    features::{
        self, conversation::ConversationFeature, git::GitFeature, id, problems::ProblemsFeature,
        search::SearchFeature, tasks::TasksFeature, terminal::TerminalFeature,
    },
    keymap::KeymapRegistry,
    localization::Localizer,
    shell::{
        ResizeTarget, ShellState,
        search_page::{DoubleShiftDetector, QuickSearch, QuickSearchEvent},
        search_provider::CommandSearchProvider,
    },
    theme,
};

mod actions;
mod canvas;
mod interactions;
mod view_state;
use canvas::{EditorElement, EditorLineLayout};

#[path = "../../shell/command_palette.rs"]
mod command_palette;
#[path = "../conversation/view_state.rs"]
mod conversation_view_state;
#[path = "../files/view.rs"]
mod files_view;
#[path = "../git/actions.rs"]
mod git_actions;
#[path = "../git/view.rs"]
mod git_view;
#[path = "../preview/view.rs"]
mod preview_view;
#[path = "../problems/view.rs"]
mod problems_view;
#[path = "../search/view.rs"]
mod search_view;
#[path = "../../shell/actions.rs"]
mod shell_actions;
#[path = "../../shell/footer/mod.rs"]
mod shell_footer;
#[path = "../../shell/header/mod.rs"]
mod shell_header;
#[path = "../../shell/main/mod.rs"]
mod shell_main;
#[path = "../../shell/render.rs"]
mod shell_render;
#[path = "../../shell/resize.rs"]
mod shell_resize;
#[path = "../tasks/controls.rs"]
mod task_controls;
#[path = "../tasks/actions.rs"]
mod tasks_actions;
#[path = "../tasks/view.rs"]
mod tasks_view;
#[path = "../terminal/actions.rs"]
mod terminal_actions;
#[path = "../terminal/view.rs"]
mod terminal_view;

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

pub struct Editor {
    session: EditorSession,
    tasks: TasksFeature,
    git: GitFeature,
    problems: ProblemsFeature,
    terminal: TerminalFeature,
    search: SearchFeature,
    quick_search: Entity<QuickSearch>,
    double_shift: DoubleShiftDetector,
    conversation: ConversationFeature,
    focus_handle: FocusHandle,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    status: String,
    shell: ShellState,
    feature_registry: FeatureRegistry,
    locale: Localizer,
    keymap: KeymapRegistry,
    is_selecting: bool,
    last_editor_bounds: Option<Bounds<Pixels>>,
    last_line_layouts: Vec<EditorLineLayout>,
    editor_scroll: ScrollHandle,
}

impl Editor {
    pub(crate) fn new(
        session: EditorSession,
        mut services: DesktopServices,
        initial_view: InitialView,
        cx: &mut Context<Self>,
    ) -> Self {
        let editor_entity = cx.entity();
        let quick_search = cx.new(|cx| {
            QuickSearch::new(cx, move |event, window, cx| {
                editor_entity.update(cx, |editor, cx| {
                    editor.handle_quick_search_event(event, window, cx);
                });
            })
        });
        let restored_view = services.conversation.active_view();
        let restored_terminals = services
            .conversation
            .active_record()
            .map(|record| record.terminals.clone())
            .unwrap_or_default();
        if services.terminal.terminals().is_empty() {
            services.terminal.restore_summaries(&restored_terminals);
        }
        let selected_execution = services
            .conversation
            .active_record()
            .and_then(|record| record.selected_execution.clone())
            .or_else(|| {
                services
                    .task
                    .records()
                    .first()
                    .map(|record| record.execution.id.clone())
            });
        let mut editor = Self {
            session,
            tasks: TasksFeature::new(services.task, selected_execution),
            git: GitFeature::new(services.git),
            problems: ProblemsFeature::new(services.lsp),
            terminal: TerminalFeature::new(services.terminal),
            search: SearchFeature::new(services.search),
            quick_search,
            double_shift: DoubleShiftDetector::default(),
            conversation: ConversationFeature::new(services.conversation),
            focus_handle: cx.focus_handle(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            status: "新しいドキュメント".to_owned(),
            shell: ShellState {
                side_panel: initial_view
                    .show_tasks
                    .then(|| ViewId::new(id::VIEW_ASSISTANT)),
                bottom_panel_open: initial_view.show_terminal || initial_view.show_problems,
                bottom_panel: ViewId::new(if initial_view.show_problems {
                    id::VIEW_PROBLEMS
                } else {
                    id::VIEW_TERMINAL
                }),
                ..ShellState::default()
            },
            feature_registry: features::bundled_registry(),
            locale: {
                let mut localizer = Localizer::bundled();
                let _ = localizer.set_active(&services.settings.settings().locale);
                localizer
            },
            keymap: KeymapRegistry::bundled(),
            is_selecting: false,
            last_editor_bounds: None,
            last_line_layouts: Vec::new(),
            editor_scroll: ScrollHandle::new(),
        };
        editor.apply_conversation_view(restored_view);
        if initial_view.show_tasks {
            editor.shell.side_panel = Some(ViewId::new(id::VIEW_ASSISTANT));
        }
        if initial_view.show_terminal || initial_view.show_problems {
            editor.shell.bottom_panel_open = true;
            editor.shell.bottom_panel = ViewId::new(if initial_view.show_problems {
                id::VIEW_PROBLEMS
            } else {
                id::VIEW_TERMINAL
            });
        }
        editor.refresh_feature_activation();
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_secs(2)).await;
                if this
                    .update(cx, |editor, cx| {
                        if !editor.session.poll_external_changes().is_empty() {
                            editor.status = "外部ファイル変更を検出しました".to_owned();
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_millis(250)).await;
                if this
                    .update(cx, |editor, cx| match editor.tasks.session.refresh() {
                        Ok(true) => {
                            if editor.tasks.selected_execution.is_none() {
                                editor.tasks.selected_execution = editor
                                    .tasks
                                    .session
                                    .records()
                                    .first()
                                    .map(|record| record.execution.id.clone());
                            }
                            cx.notify();
                        }
                        Ok(false) => {}
                        Err(error) => {
                            editor.status = format!("Task 更新失敗: {error}");
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_secs(1)).await;
                if this
                    .update(cx, |editor, cx| {
                        if !editor.feature_registry.is_running(id::FEATURE_GIT) {
                            return;
                        }
                        let Some(root) = editor.session.workspace_root().map(ToOwned::to_owned)
                        else {
                            return;
                        };
                        if editor.git.session.refresh(&root).unwrap_or(false) {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_millis(250)).await;
                if this
                    .update(cx, |editor, cx| match editor.terminal.session.refresh() {
                        Ok(true) => cx.notify(),
                        Ok(false) => {}
                        Err(error) => {
                            editor.status = format!("Terminal refresh failed: {error}");
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_millis(500)).await;
                if this
                    .update(cx, |editor, cx| {
                        editor.refresh_feature_activation();
                        if !editor.feature_registry.is_running(id::FEATURE_RUST) {
                            if editor.problems.lsp.is_started()
                                && let Err(error) = editor.problems.lsp.shutdown()
                            {
                                editor.status = format!("LSP shutdown: {error}");
                                cx.notify();
                            }
                            return;
                        }
                        let synced = editor.problems.lsp.sync_active(&editor.session);
                        let refreshed = editor.problems.lsp.refresh();
                        match (synced, refreshed) {
                            (Ok(sync_changed), Ok(diagnostics_changed)) => {
                                if sync_changed || diagnostics_changed {
                                    cx.notify();
                                }
                            }
                            (Err(error), _) | (_, Err(error)) => {
                                editor.status = format!("LSP: {error}");
                                cx.notify();
                            }
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_millis(100)).await;
                if this
                    .update(cx, |editor, cx| {
                        if editor.search.workspace.refresh() {
                            editor.feature_registry.deactivate_command(
                                &crate::extension_ui::CommandId::new(id::COMMAND_FIND_WORKSPACE),
                            );
                            editor.status = if let Some(error) = editor.search.workspace.error() {
                                format!("Workspace search failed: {error}")
                            } else {
                                format!(
                                    "Workspace search: {} hits",
                                    editor.search.workspace.hits().len()
                                )
                            };
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_millis(50)).await;
                if this
                    .update(cx, |editor, cx| {
                        if let Some(result) = editor
                            .problems
                            .completion_receiver
                            .as_ref()
                            .and_then(|receiver| receiver.try_recv().ok())
                        {
                            editor.problems.completion_receiver = None;
                            editor.status = match result {
                                Ok(items) if items.is_empty() => "補完候補はありません".to_owned(),
                                Ok(items) => format!(
                                    "補完: {}",
                                    items
                                        .iter()
                                        .take(6)
                                        .map(|item| item.label.as_str())
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                ),
                                Err(error) => format!("補完失敗: {error}"),
                            };
                            cx.notify();
                        }
                        if let Some(result) = editor
                            .problems
                            .definition_receiver
                            .as_ref()
                            .and_then(|receiver| receiver.try_recv().ok())
                        {
                            editor.problems.definition_receiver = None;
                            match result {
                                Ok(Some(target)) => match editor.session.open_definition(&target) {
                                    Ok(offset) => {
                                        editor.restore_active_view();
                                        editor.move_to(offset, cx);
                                        editor.status = format!("定義: {}", target.path.display());
                                    }
                                    Err(error) => editor.status = format!("定義移動失敗: {error}"),
                                },
                                Ok(None) => editor.status = "定義が見つかりません".to_owned(),
                                Err(error) => editor.status = format!("定義移動失敗: {error}"),
                            }
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_secs(1)).await;
                if this
                    .update(cx, |editor, _| {
                        let _ = editor.capture_conversation();
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

    pub(crate) fn editor_focus_handle(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    fn refresh_feature_activation(&mut self) {
        if self.session.workspace_root().is_some() {
            self.feature_registry
                .activate(ActivationEvent::WorkspaceOpened);
        }
        let language = self
            .session
            .active_path()
            .and_then(|path| self.problems.languages.detect_path(path));
        self.feature_registry
            .activate(ActivationEvent::LanguageChanged(language));
        self.feature_registry.set_command_active(
            crate::extension_ui::CommandId::new(id::COMMAND_START_TERMINAL),
            self.terminal.has_running_process(),
        );
        self.feature_registry.set_command_active(
            crate::extension_ui::CommandId::new(id::COMMAND_START_CODEX),
            self.tasks.has_active_execution(),
        );
        self.shell
            .synchronize_activation(&mut self.feature_registry);
    }

    fn cursor_line_column(&self) -> (usize, usize) {
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
}

impl Focusable for Editor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn task_action_button(label: &'static str, primary: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(label)
        .h(px(25.0))
        .px_2()
        .rounded(px(5.0))
        .border_1()
        .border_color(if primary {
            rgb(0x6366f1)
        } else {
            theme::border()
        })
        .bg(if primary {
            theme::accent_soft()
        } else {
            theme::surface()
        })
        .text_size(px(10.0))
        .text_color(if primary {
            rgb(0xd8d8ff)
        } else {
            theme::muted()
        })
        .flex()
        .items_center()
        .hover(|style| style.bg(theme::surface_hover()).text_color(theme::text()))
        .child(label)
}

fn task_status_color(status: ExecutionStatus) -> gpui::Rgba {
    match status {
        ExecutionStatus::Succeeded => rgb(0x7dd3a7),
        ExecutionStatus::Failed | ExecutionStatus::Cancelled => rgb(0xf29a9a),
        ExecutionStatus::WaitingForInput | ExecutionStatus::WaitingForApproval => rgb(0xf4c67a),
        ExecutionStatus::Queued | ExecutionStatus::Running => rgb(0x9ba8ff),
    }
}

fn change_label(kind: ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Added => "A",
        ChangeKind::Modified => "M",
        ChangeKind::Deleted => "D",
        ChangeKind::Renamed => "R",
        ChangeKind::Untracked => "?",
        ChangeKind::Conflicted => "!",
    }
}

fn change_color(kind: ChangeKind) -> gpui::Rgba {
    match kind {
        ChangeKind::Added => rgb(0x7dd3a7),
        ChangeKind::Deleted | ChangeKind::Conflicted => rgb(0xf29a9a),
        ChangeKind::Modified | ChangeKind::Renamed => rgb(0xf4c67a),
        ChangeKind::Untracked => theme::muted(),
    }
}

fn truncate_chars(text: &str, limit: usize) -> String {
    let mut value = text.chars().take(limit).collect::<String>();
    if text.chars().count() > limit {
        value.push('…');
    }
    value
}

fn command_item(index: usize, label: String, shortcut: String) -> gpui::Stateful<gpui::Div> {
    div()
        .id(("command-palette-item", index))
        .h(px(31.0))
        .px_2()
        .rounded(px(6.0))
        .flex()
        .items_center()
        .text_size(px(12.0))
        .text_color(theme::text())
        .hover(|style| style.bg(theme::surface_active()))
        .child(label)
        .child(div().flex_1())
        .child(
            div()
                .text_size(px(10.0))
                .text_color(theme::subtle())
                .child(shortcut),
        )
}

fn quick_action(label: &'static str, shortcut: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(label)
        .h(px(34.0))
        .px_3()
        .rounded(px(7.0))
        .border_1()
        .border_color(theme::border())
        .bg(theme::surface())
        .flex()
        .items_center()
        .text_size(px(12.0))
        .text_color(theme::text())
        .hover(|style| style.bg(theme::surface_hover()).border_color(rgb(0x444657)))
        .child(label)
        .child(div().flex_1())
        .child(
            div()
                .text_size(px(10.0))
                .text_color(theme::subtle())
                .child(shortcut),
        )
}

fn file_badge(label: &'static str, color: gpui::Rgba) -> gpui::Div {
    div()
        .size(px(14.0))
        .rounded(px(3.0))
        .flex()
        .items_center()
        .justify_center()
        .bg(color)
        .text_color(theme::canvas())
        .text_size(px(8.0))
        .child(label)
}
