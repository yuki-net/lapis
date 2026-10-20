use gpui::{
    App, AppContext, Application, Bounds, KeyBinding, TitlebarOptions, WindowBounds, WindowOptions,
    actions, px, size,
};
use lapis_desktop_client::{
    ConversationSession, EditorSession, GitSession, LspSession, SettingsSession, TaskSession,
    TerminalSession, WorkspaceSearchSession,
};

use crate::features::command_search::{
    SearchBackspace, SearchConfirm, SearchDelete, SearchDismiss, SearchEnd, SearchHome, SearchLeft,
    SearchNext, SearchPrevious, SearchRight, SearchSelectAll,
};
use crate::root::DesktopRoot;

actions!(
    editor,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        Enter,
        Paste,
        Cut,
        Copy,
        Open,
        Save,
        New,
        Undo,
        Redo,
        Find,
        FindWorkspace,
        FindNext,
        Complete,
        GoToDefinition,
        ShowCommands,
        Dismiss,
        TogglePreview,
        ToggleBottomPanel,
        ToggleAssistant,
        Quit,
    ]
);

pub struct DesktopServices {
    pub(crate) task: TaskSession,
    pub(crate) git: GitSession,
    pub(crate) lsp: LspSession,
    pub(crate) terminal: TerminalSession,
    pub(crate) search: WorkspaceSearchSession,
    pub(crate) conversation: ConversationSession,
    pub(crate) settings: SettingsSession,
}

impl DesktopServices {
    pub fn new(
        task: TaskSession,
        git: GitSession,
        lsp: LspSession,
        terminal: TerminalSession,
        search: WorkspaceSearchSession,
        conversation: ConversationSession,
        settings: SettingsSession,
    ) -> Self {
        Self {
            task,
            git,
            lsp,
            terminal,
            search,
            conversation,
            settings,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct InitialView {
    pub empty_window: bool,
    pub show_tasks: bool,
    pub show_terminal: bool,
    pub show_problems: bool,
}

pub fn run(session: EditorSession, services: DesktopServices, initial_view: InitialView) {
    Application::new()
        .with_assets(crate::components::IconAssets)
        .run(move |cx: &mut App| {
            bind_keys(cx);
            #[cfg(debug_assertions)]
            crate::devtools::init(cx);
            let bounds = Bounds::centered(None, size(px(1440.0), px(900.0)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Lapis".into()),
                        appears_transparent: true,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                move |window, cx| {
                    cx.new(|cx| DesktopRoot::new(session, services, initial_view, window, cx))
                },
            )
            .expect("Lapis window should open");
            cx.on_action(|_: &Quit, cx| cx.quit());
        });
}

#[derive(Clone)]
pub(crate) struct WindowFactory {
    repository: std::sync::Arc<dyn lapis_workspace::WorkspaceRepository>,
    dialog: std::sync::Arc<dyn lapis_desktop_client::WorkspaceDialog>,
    state: std::sync::Arc<dyn lapis_desktop_client::WorkspaceStateRepository>,
    documents: std::sync::Arc<dyn lapis_document::DocumentConnection>,
    task: std::sync::Arc<dyn lapis_task::TaskConnection>,
    git: std::sync::Arc<dyn lapis_git::GitConnection>,
    lsp: std::sync::Arc<dyn lapis_lsp::LanguageServerBackend>,
    terminal: std::sync::Arc<dyn lapis_terminal::TerminalBackend>,
    search: std::sync::Arc<dyn lapis_workspace::WorkspaceSearchBackend>,
    conversation: std::sync::Arc<dyn lapis_desktop_client::ConversationRepository>,
    settings: SettingsSession,
}
impl WindowFactory {
    pub(crate) fn new(editor: &EditorSession, services: &DesktopServices) -> Self {
        Self {
            repository: editor.repository(),
            dialog: editor.file_dialog(),
            state: editor.state_repository(),
            documents: editor.document_connection(),
            task: services.task.backend(),
            git: services.git.backend(),
            lsp: services.lsp.backend(),
            terminal: services.terminal.backend(),
            search: services.search.backend(),
            conversation: services.conversation.repository(),
            settings: services.settings.clone(),
        }
    }
    pub(crate) fn build(self) -> (EditorSession, DesktopServices) {
        let editor =
            EditorSession::new_empty(self.repository, self.dialog, self.state, self.documents);
        let conversation = ConversationSession::new(self.conversation, editor.snapshot());
        let services = DesktopServices::new(
            TaskSession::new(self.task),
            GitSession::new(self.git),
            LspSession::new(self.lsp),
            TerminalSession::new(self.terminal),
            WorkspaceSearchSession::new(self.search),
            conversation,
            self.settings,
        );
        (editor, services)
    }
}
pub(crate) fn open_desktop_window(
    editor: EditorSession,
    services: DesktopServices,
    initial: InitialView,
    cx: &mut App,
) {
    let bounds = Bounds::centered(None, size(px(1440.0), px(900.0)), cx);
    let result = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("Lapis".into()),
                appears_transparent: true,
                ..Default::default()
            }),
            ..Default::default()
        },
        move |window, cx| cx.new(|cx| DesktopRoot::new(editor, services, initial, window, cx)),
    );
    if let Err(error) = result {
        eprintln!("Windowを開けませんでした: {error}");
    }
}

fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some("Editor")),
        KeyBinding::new("delete", Delete, Some("Editor")),
        KeyBinding::new("left", Left, Some("Editor")),
        KeyBinding::new("right", Right, Some("Editor")),
        KeyBinding::new("shift-left", SelectLeft, Some("Editor")),
        KeyBinding::new("shift-right", SelectRight, Some("Editor")),
        KeyBinding::new("cmd-a", SelectAll, Some("Editor")),
        KeyBinding::new("ctrl-a", SelectAll, Some("Editor")),
        KeyBinding::new("cmd-v", Paste, Some("Editor")),
        KeyBinding::new("ctrl-v", Paste, Some("Editor")),
        KeyBinding::new("cmd-c", Copy, Some("Editor")),
        KeyBinding::new("ctrl-c", Copy, Some("Editor")),
        KeyBinding::new("cmd-x", Cut, Some("Editor")),
        KeyBinding::new("ctrl-x", Cut, Some("Editor")),
        KeyBinding::new("home", Home, Some("Editor")),
        KeyBinding::new("end", End, Some("Editor")),
        KeyBinding::new("enter", Enter, Some("Editor")),
        KeyBinding::new("cmd-o", Open, None),
        KeyBinding::new("ctrl-o", Open, None),
        KeyBinding::new("cmd-s", Save, None),
        KeyBinding::new("ctrl-s", Save, None),
        KeyBinding::new("cmd-n", New, None),
        KeyBinding::new("ctrl-n", New, None),
        KeyBinding::new("cmd-z", Undo, Some("Editor")),
        KeyBinding::new("ctrl-z", Undo, Some("Editor")),
        KeyBinding::new("cmd-shift-z", Redo, Some("Editor")),
        KeyBinding::new("ctrl-y", Redo, Some("Editor")),
        KeyBinding::new("cmd-f", Find, Some("Editor")),
        KeyBinding::new("ctrl-f", Find, Some("Editor")),
        KeyBinding::new("ctrl-shift-f", FindWorkspace, None),
        KeyBinding::new("cmd-g", FindNext, Some("Editor")),
        KeyBinding::new("f3", FindNext, Some("Editor")),
        KeyBinding::new("ctrl-space", Complete, Some("Editor")),
        KeyBinding::new("f12", GoToDefinition, Some("Editor")),
        KeyBinding::new("cmd-shift-k", ShowCommands, None),
        KeyBinding::new("ctrl-shift-k", ShowCommands, None),
        KeyBinding::new("backspace", SearchBackspace, Some("QuickSearch")),
        KeyBinding::new("delete", SearchDelete, Some("QuickSearch")),
        KeyBinding::new("left", SearchLeft, Some("QuickSearch")),
        KeyBinding::new("right", SearchRight, Some("QuickSearch")),
        KeyBinding::new("cmd-a", SearchSelectAll, Some("QuickSearch")),
        KeyBinding::new("ctrl-a", SearchSelectAll, Some("QuickSearch")),
        KeyBinding::new("home", SearchHome, Some("QuickSearch")),
        KeyBinding::new("end", SearchEnd, Some("QuickSearch")),
        KeyBinding::new("up", SearchPrevious, Some("QuickSearch")),
        KeyBinding::new("down", SearchNext, Some("QuickSearch")),
        KeyBinding::new("enter", SearchConfirm, Some("QuickSearch")),
        KeyBinding::new("escape", SearchDismiss, Some("QuickSearch")),
        KeyBinding::new("escape", Dismiss, None),
        KeyBinding::new("ctrl-alt-p", TogglePreview, None),
        KeyBinding::new("ctrl-j", ToggleBottomPanel, None),
        KeyBinding::new("ctrl-shift-a", ToggleAssistant, None),
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("ctrl-q", Quit, None),
    ]);
}
