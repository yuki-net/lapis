use std::{
    ops::Range,
    time::{Duration, Instant},
};

use gpui::{
    App, Bounds, ClickEvent, Context, CursorStyle, Element, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, Focusable, GlobalElementId, KeyDownEvent, LayoutId,
    ModifiersChangedEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad,
    Pixels, Point, PromptButton, PromptLevel, Render, ScrollHandle, ShapedLine, SharedString,
    Style, TextRun, Timer, UTF16Selection, Window, WindowControlArea, anchored, div, fill, point,
    prelude::*, px, relative, size,
};
use lapis_app_services::{
    ConversationViewState, DocumentAction, DocumentCloseDisposition, EditorSession,
};
use lapis_editor_core::{ExecutionId, TaskId};
use lapis_git::ChangeKind;
use lapis_lsp::LspPosition;
use lapis_task_runner::{ExecutionStatus, TaskControl, TaskMode};
use lapis_workspace::FileEntryKind;

use crate::{
    app::*,
    components::{Icon, IconName, SurfaceVariant, panel_empty_state, surface, tool_empty_state},
    extension_ui::{ActivationEvent, FeatureRegistry, ThemeId, UiSlot, ViewId},
    features::{
        self,
        command_search::{
            CommandSearchProvider, DoubleShiftDetector, QuickSearch, QuickSearchEvent,
        },
        conversation::ConversationFeature,
        git::GitFeature,
        id,
        problems::ProblemsFeature,
        search::SearchFeature,
        tasks::TasksFeature,
        terminal::TerminalFeature,
    },
    keymap::KeymapRegistry,
    localization::Localizer,
    shell::{PanelTab, ShellState},
    theme,
};

mod actions;
mod canvas;
mod conversation_actions;
mod document_actions;
mod interactions;
mod runtime;
mod view_state;
use canvas::{EditorElement, EditorLineLayout};

#[path = "../files/view.rs"]
mod files_view;
#[path = "../git/actions.rs"]
mod git_actions;
#[path = "../git/view.rs"]
mod git_view;
#[path = "../search/view.rs"]
mod search_view;
#[path = "../../shell/main/mod.rs"]
mod shell_main;
#[path = "../tasks/controls.rs"]
mod task_controls;
#[path = "../tasks/actions.rs"]
mod tasks_actions;
#[path = "../tasks/view.rs"]
mod tasks_view;
#[path = "../terminal/actions.rs"]
mod terminal_actions;

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
    settings: lapis_app_services::SettingsSession,
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
        let settings = services.settings.clone();
        let global_settings = settings.settings();
        let configured_theme = ThemeId::new(global_settings.theme.clone());
        let theme_loaded = theme::set_active(&configured_theme);
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
            settings,
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
            status: "New document".to_owned(),
            shell: ShellState::default(),
            feature_registry: features::bundled_registry(),
            locale: {
                let mut localizer = Localizer::bundled();
                let _ = localizer.set_active(&global_settings.locale);
                localizer
            },
            keymap: KeymapRegistry::bundled(),
            is_selecting: false,
            last_editor_bounds: None,
            last_line_layouts: Vec::new(),
            editor_scroll: ScrollHandle::new(),
        };
        if !theme_loaded {
            editor.status = format!("未登録のテーマのためDarkを使用: {}", global_settings.theme);
        }
        editor.apply_conversation_view(restored_view);
        editor.shell.synchronize_documents(&editor.session.tabs());
        if initial_view.show_tasks {
            editor.shell.activate_view(
                crate::extension_ui::PanelPosition::Right,
                ViewId::new(id::VIEW_ASSISTANT),
            );
        }
        if initial_view.show_terminal || initial_view.show_problems {
            editor.shell.activate_view(
                crate::extension_ui::PanelPosition::Bottom,
                ViewId::new(if initial_view.show_problems {
                    id::VIEW_PROBLEMS
                } else {
                    id::VIEW_TERMINAL
                }),
            );
        }
        editor.refresh_feature_activation();
        editor.start_background_tasks(cx);
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
            theme::task_primary_border()
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
            theme::task_primary_text()
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
        ExecutionStatus::Succeeded => theme::status_success(),
        ExecutionStatus::Failed | ExecutionStatus::Cancelled => theme::status_error(),
        ExecutionStatus::WaitingForInput | ExecutionStatus::WaitingForApproval => {
            theme::status_warning()
        }
        ExecutionStatus::Queued | ExecutionStatus::Running => theme::status_info(),
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
        ChangeKind::Added => theme::diff_added(),
        ChangeKind::Deleted | ChangeKind::Conflicted => theme::diff_removed(),
        ChangeKind::Modified | ChangeKind::Renamed => theme::diff_changed(),
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
        .hover(|style| {
            style
                .bg(theme::surface_hover())
                .border_color(theme::command_input_border())
        })
        .child(label)
        .child(div().flex_1())
        .child(
            div()
                .text_size(px(10.0))
                .text_color(theme::subtle())
                .child(shortcut),
        )
}
