use gpui::{
    App, AppContext, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions, actions,
    size,
};
use lapis_desktop_client::{
    ConversationSession, EditorSession, GitSession, LspSession, SettingsSession, TaskSession,
    TerminalSession, WorkspaceSearchSession,
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
        SaveAs,
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
        ToggleBottomZone,
        ToggleLeftZone,
        ToggleRightZone,
        ToggleAssistant,
        Quit,
    ]
);

#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = lapis, no_json)]
pub(crate) struct InvokeCommand(pub String);

pub(crate) const EXTRA_COMMANDS: &[&str] = &[
    crate::features::id::COMMAND_OPEN_WORKSPACE,
    crate::features::id::COMMAND_TOGGLE_PANEL_CONTROLS,
    crate::features::id::COMMAND_WORKBENCH_IDE,
    crate::features::id::COMMAND_WORKBENCH_AGENT,
    crate::features::id::COMMAND_BIND_VIEW_SHARED,
    crate::features::id::COMMAND_BIND_VIEW_TASK,
    #[cfg(debug_assertions)]
    crate::features::id::COMMAND_TOGGLE_INSPECTOR,
];

pub struct DesktopServices {
    pub(crate) connect_host: Option<std::sync::Arc<dyn Fn() -> Result<(), String> + Send + Sync>>,
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
            connect_host: None,
            task,
            git,
            lsp,
            terminal,
            search,
            conversation,
            settings,
        }
    }
    pub fn with_host_connector(
        mut self,
        connector: impl Fn() -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        self.connect_host = Some(std::sync::Arc::new(connector));
        self
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct InitialView {
    pub empty_window: bool,
    #[doc(hidden)]
    pub native_window_smoke: bool,
    pub show_tasks: bool,
    pub show_terminal: bool,
    pub show_problems: bool,
}

/// Runs the desktop event loop with an initial root window and quits after the last window closes.
pub fn run(session: EditorSession, services: DesktopServices, initial_view: InitialView) {
    Application::new()
        .with_assets(crate::components::IconAssets)
        .run(move |cx: &mut App| {
            crate::keymap::gpui::bind_keys(cx);
            #[cfg(debug_assertions)]
            crate::devtools::init(cx);
            open_desktop_window(session, services, initial_view, cx);
            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
        });
}

#[derive(Clone)]
pub(crate) struct WindowFactory {
    connect_host: Option<std::sync::Arc<dyn Fn() -> Result<(), String> + Send + Sync>>,
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
    pub(crate) fn path_scope(&self) -> Option<std::path::PathBuf> {
        self.dialog.path_scope()
    }
    pub(crate) fn new(editor: &EditorSession, services: &DesktopServices) -> Self {
        Self {
            connect_host: services.connect_host.clone(),
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
        let mut services = DesktopServices::new(
            TaskSession::new(self.task),
            GitSession::new(self.git),
            LspSession::new(self.lsp),
            TerminalSession::new(self.terminal),
            WorkspaceSearchSession::new(self.search),
            conversation,
            self.settings,
        );
        services.connect_host = self.connect_host;
        (editor, services)
    }
    pub(crate) fn open_host(self) -> Result<(), String> {
        self.connect_host
            .ok_or_else(|| "この起動構成ではHost接続を開けません".to_owned())?()
    }
}
/// Opens an independent root window; an opening failure leaves no new window and is not returned.
pub(crate) fn open_desktop_window(
    editor: EditorSession,
    services: DesktopServices,
    initial: InitialView,
    cx: &mut App,
) {
    let result = cx.open_window(window_options(cx), move |window, cx| {
        cx.new(|cx| DesktopRoot::new(editor, services, initial, window, cx))
    });
    if let Err(error) = result {
        eprintln!("Windowを開けませんでした: {error}");
    }
}

/// Returns centered 1440 by 900 logical-pixel bounds with a transparent Lapis title bar.
pub(crate) fn window_options(cx: &App) -> WindowOptions {
    let bounds = Bounds::centered(
        None,
        size(
            crate::tokens::size::DEFAULT_WINDOW_WIDTH,
            crate::tokens::size::DEFAULT_WINDOW_HEIGHT,
        ),
        cx,
    );
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some("Lapis".into()),
            appears_transparent: true,
            ..Default::default()
        }),
        ..Default::default()
    }
}
