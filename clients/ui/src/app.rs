use desktop_client::{
    ConversationSession, EditorSession, GitSession, LspSession, ProcessSession, SettingsSession,
    TaskSession, TerminalSession, WorkspaceSearchSession,
};
use gpui::{App, AppContext, actions};

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
    crate::features::id::COMMAND_TOGGLE_DEVTOOL,
];

/// 接続先の Workspace 切替候補。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceOption {
    pub id: String,
    pub name: String,
}

/// 接続中の Host を UI に示すための表示情報。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConnectionIdentity {
    pub host_name: Option<String>,
    pub host_id: Option<String>,
    /// Web のように Host 側の Workspace 名を UI が持つ場合だけ入れる。
    pub workspace_name: Option<String>,
    pub workspace_id: Option<String>,
    /// この接続で Workspace を切り替えられるか。
    pub can_switch: bool,
    /// 切り替え候補。`workspace.list` の応答をそのまま使う。
    pub workspaces: Vec<WorkspaceOption>,
}

/// ローカル Web の接続情報を表示のたびに集める供給元。
pub type WebAccessSource =
    std::sync::Arc<dyn Fn() -> Option<desktop_client::WebAccess> + Send + Sync>;

pub struct DesktopServices {
    pub(crate) connect_host: Option<std::sync::Arc<dyn Fn() -> Result<(), String> + Send + Sync>>,
    pub(crate) connection: Option<ConnectionIdentity>,
    pub(crate) initial_task: Option<editor_core::ConversationId>,
    pub(crate) task: TaskSession,
    pub(crate) git: GitSession,
    pub(crate) lsp: LspSession,
    pub(crate) terminal: TerminalSession,
    pub(crate) search: WorkspaceSearchSession,
    pub(crate) conversation: ConversationSession,
    pub(crate) settings: SettingsSession,
    pub(crate) process: ProcessSession,
    pub(crate) workbench_watch: Option<desktop_client::WorkbenchWatch>,
    /// アプリ側の更新処理。install layout を検出できない実行では持たない。
    pub(crate) updater: Option<UpdaterHandle>,
    /// ローカル Web の接続情報を都度集める供給元。Web を持たない構成では `None`。
    pub(crate) web_access: Option<WebAccessSource>,
    /// Workspace の view state 共有。Web 側の統合は別 PR のため Desktop だけで使う。
    #[cfg(not(target_family = "wasm"))]
    pub(crate) session_view: Option<std::sync::Arc<desktop_client::SessionViewSync>>,
}

/// 更新処理の共有 handle。UI は Controller だけを呼び、状態は Watch を購読する。
#[derive(Clone)]
pub struct UpdaterHandle {
    pub(crate) controller: std::sync::Arc<dyn desktop_client::UpdateController>,
    pub(crate) watch: desktop_client::UpdateWatch,
    pub(crate) interval: std::time::Duration,
    /// 設定変更の通知。方針が変わったときだけ再確認する。
    pub(crate) settings_changed: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
}

impl UpdaterHandle {
    pub fn new(
        controller: std::sync::Arc<dyn desktop_client::UpdateController>,
        watch: desktop_client::UpdateWatch,
        interval: std::time::Duration,
    ) -> Self {
        Self {
            controller,
            watch,
            interval,
            settings_changed: None,
        }
    }

    /// 設定変更を更新処理へ知らせる入口を設定する。
    pub fn with_settings_changed(mut self, changed: impl Fn() + Send + Sync + 'static) -> Self {
        self.settings_changed = Some(std::sync::Arc::new(changed));
        self
    }

    pub fn notify_settings_changed(&self) {
        if let Some(changed) = &self.settings_changed {
            changed();
        }
    }
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
            connection: None,
            initial_task: None,
            task,
            git,
            lsp,
            terminal,
            search,
            conversation,
            settings,
            process: ProcessSession::unavailable(),
            workbench_watch: None,
            updater: None,
            web_access: None,
            #[cfg(not(target_family = "wasm"))]
            session_view: None,
        }
    }
    pub fn with_connection_identity(mut self, connection: ConnectionIdentity) -> Self {
        self.connection = Some(connection);
        self
    }
    /// ローカル Web の接続情報を、表示のたびに集め直す供給元を設定する。
    /// 起動構成によっては表示できないため、供給元自体が `None` を返してよい。
    pub fn with_web_access(
        mut self,
        access: impl Fn() -> Option<desktop_client::WebAccess> + Send + Sync + 'static,
    ) -> Self {
        self.web_access = Some(std::sync::Arc::new(access));
        self
    }
    /// URL の fragment から復元した Task を、最初の snapshot で選択する。
    pub fn with_initial_task(mut self, task: editor_core::ConversationId) -> Self {
        self.initial_task = Some(task);
        self
    }
    pub fn with_host_connector(
        mut self,
        connector: impl Fn() -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        self.connect_host = Some(std::sync::Arc::new(connector));
        self
    }
    pub fn with_workbench_watch(mut self, watch: desktop_client::WorkbenchWatch) -> Self {
        self.workbench_watch = Some(watch);
        self
    }
    /// Workspace の view state 共有を有効にする。
    #[cfg(not(target_family = "wasm"))]
    pub fn with_session_view(
        mut self,
        sync: std::sync::Arc<desktop_client::SessionViewSync>,
    ) -> Self {
        self.session_view = Some(sync);
        self
    }
    #[cfg(not(target_family = "wasm"))]
    pub fn session_view(&self) -> Option<&std::sync::Arc<desktop_client::SessionViewSync>> {
        self.session_view.as_ref()
    }
    pub fn with_process_session(mut self, process: ProcessSession) -> Self {
        self.process = process;
        self
    }
    /// アプリ側の更新処理を有効にする。install layout を検出できたときだけ呼ぶ。
    pub fn with_updater(mut self, updater: UpdaterHandle) -> Self {
        self.updater = Some(updater);
        self
    }
    pub fn workbench_watch(&self) -> Option<&desktop_client::WorkbenchWatch> {
        self.workbench_watch.as_ref()
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

/// Release ビルドでも tracing のエラーを stderr に出力する。
#[cfg(all(not(debug_assertions), not(target_family = "wasm")))]
pub(crate) fn init_release_tracing() {
    use tracing_subscriber::Layer;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_filter(tracing_subscriber::filter::LevelFilter::INFO);
    let _ = tracing_subscriber::registry().with(stderr_layer).try_init();
}

#[derive(Clone)]
pub(crate) struct WindowFactory {
    connect_host: Option<std::sync::Arc<dyn Fn() -> Result<(), String> + Send + Sync>>,
    connection: Option<ConnectionIdentity>,
    repository: std::sync::Arc<dyn workspace::WorkspaceRepository>,
    dialog: std::sync::Arc<dyn desktop_client::WorkspaceDialog>,
    state: std::sync::Arc<dyn desktop_client::WorkspaceStateRepository>,
    documents: std::sync::Arc<dyn document::DocumentConnection>,
    task: std::sync::Arc<dyn task::TaskConnection>,
    git: std::sync::Arc<dyn git::GitConnection>,
    lsp: std::sync::Arc<dyn lsp::LanguageServerBackend>,
    terminal: std::sync::Arc<dyn terminal::TerminalBackend>,
    search: std::sync::Arc<dyn workspace::WorkspaceSearchBackend>,
    conversation: std::sync::Arc<dyn desktop_client::ConversationRepository>,
    settings: SettingsSession,
    process: ProcessSession,
    updater: Option<UpdaterHandle>,
    web_access: Option<WebAccessSource>,
    #[cfg(not(target_family = "wasm"))]
    session_view: Option<std::sync::Arc<desktop_client::SessionViewSync>>,
}
impl WindowFactory {
    pub(crate) fn path_scope(&self) -> Option<std::path::PathBuf> {
        self.dialog.path_scope()
    }
    pub(crate) fn new(editor: &EditorSession, services: &DesktopServices) -> Self {
        Self {
            connect_host: services.connect_host.clone(),
            connection: services.connection.clone(),
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
            process: services.process.clone(),
            updater: services.updater.clone(),
            web_access: services.web_access.clone(),
            #[cfg(not(target_family = "wasm"))]
            session_view: services.session_view.clone(),
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
        services.connection = self.connection;
        services.process = self.process;
        services.updater = self.updater;
        services.web_access = self.web_access;
        #[cfg(not(target_family = "wasm"))]
        {
            services.session_view = self.session_view;
        }
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
    let locale_id = services.settings.settings().locale();
    let workspace_name = services
        .connection
        .as_ref()
        .and_then(|connection| connection.workspace_name.clone())
        .unwrap_or_else(|| editor.workspace_name().to_owned());
    let workspace_id = services
        .connection
        .as_ref()
        .and_then(|connection| connection.workspace_id.clone())
        .or_else(|| editor.workspace_id().map(|id| id.as_str().to_owned()));
    let title = desktop_client::window_title(Some(&workspace_name), workspace_id.as_deref());
    let result = cx.open_window(
        crate::desktop::window_options_with_title(cx, title),
        move |window, cx| cx.new(|cx| DesktopRoot::new(editor, services, initial, window, cx)),
    );
    if let Err(error) = result {
        tracing::error!(target: "lapis::app", "Windowを開けませんでした: {error}");
        let mut locale = crate::localization::Localizer::bundled();
        locale.set_active(&locale_id);
        let title = locale.resolve(&localization::MessageId::new("notification.error.window"));
        crate::notification::publish_error(cx, "window.creation_failed", &title, error.to_string());
    }
}
