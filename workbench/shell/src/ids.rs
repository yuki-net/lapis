//! Built-in Feature / Command / View identifiers. Re-exported by desktop-ui.

pub const FEATURE_SHELL: &str = "core.shell";
pub const FEATURE_EDITOR: &str = "core.editor";
pub const FEATURE_COMMAND_SEARCH: &str = "core.command-search";
#[cfg(debug_assertions)]
pub const FEATURE_DEVTOOLS: &str = "bundled.devtools";
pub const FEATURE_CONVERSATION: &str = "core.conversation";
pub const FEATURE_FILES: &str = "bundled.files";
pub const FEATURE_SEARCH: &str = "bundled.search";
pub const FEATURE_GIT: &str = "bundled.git";
pub const FEATURE_PREVIEW: &str = "bundled.preview";
pub const FEATURE_ASSISTANT: &str = "bundled.assistant";
pub const FEATURE_TERMINAL: &str = "bundled.terminal";
pub const FEATURE_RESOURCES: &str = "bundled.resource-manager";
pub const FEATURE_RUST: &str = "bundled.language.rust";
pub const FEATURE_MARKDOWN: &str = "bundled.language.markdown";

pub const COMMAND_FIND_WORKSPACE: &str = "lapis.command.find-workspace";
pub const COMMAND_NEW_DOCUMENT: &str = "lapis.command.new-document";
pub const COMMAND_OPEN_WORKSPACE: &str = "lapis.command.open-workspace";
pub const COMMAND_OPEN_DOCUMENT: &str = "lapis.command.open-document";
pub const COMMAND_SAVE_AS: &str = "lapis.command.save-as";
pub const COMMAND_UNDO: &str = "lapis.command.undo";
pub const COMMAND_REDO: &str = "lapis.command.redo";
pub const COMMAND_FIND: &str = "lapis.command.find";
pub const COMMAND_FIND_NEXT: &str = "lapis.command.find-next";
pub const COMMAND_SHOW_COMMANDS: &str = "lapis.command.show-commands";
pub const COMMAND_QUIT: &str = "lapis.command.quit";
pub const COMMAND_SAVE_DOCUMENT: &str = "lapis.command.save-document";
pub const COMMAND_TOGGLE_PREVIEW: &str = "lapis.command.toggle-preview";
pub const COMMAND_TOGGLE_BOTTOM_ZONE: &str = "lapis.command.toggle-bottom-zone";
pub const COMMAND_TOGGLE_LEFT_ZONE: &str = "lapis.command.toggle-left-zone";
pub const COMMAND_TOGGLE_RIGHT_ZONE: &str = "lapis.command.toggle-right-zone";
pub const COMMAND_TOGGLE_ASSISTANT: &str = "lapis.command.toggle-assistant";
pub const COMMAND_TOGGLE_PANEL_CONTROLS: &str = "lapis.command.toggle-panel-controls";
pub const COMMAND_WORKBENCH_IDE: &str = "lapis.command.workbench-ide";
pub const COMMAND_WORKBENCH_AGENT: &str = "lapis.command.workbench-agent";
pub const COMMAND_BIND_VIEW_SHARED: &str = "lapis.command.bind-view-shared";
pub const COMMAND_BIND_VIEW_TASK: &str = "lapis.command.bind-view-task";
pub const COMMAND_ACCEPT_REMOTE_WORKBENCH: &str = "lapis.command.accept-remote-workbench";
pub const COMMAND_RETRY_LOCAL_WORKBENCH: &str = "lapis.command.retry-local-workbench";
pub const COMMAND_START_CODEX: &str = "lapis.command.start-codex";
pub const COMMAND_START_TERMINAL: &str = "lapis.command.start-terminal";
#[cfg(debug_assertions)]
pub const COMMAND_TOGGLE_DEVTOOL: &str = "lapis.command.dev.toggle-devtool";

pub const VIEW_FILES: &str = "lapis.view.files";
pub const VIEW_SEARCH: &str = "lapis.view.search";
pub const VIEW_GIT: &str = "lapis.view.git";
pub const VIEW_CHANGES: &str = "lapis.view.changes";
pub const VIEW_PROJECT_TASKS: &str = "lapis.view.project-tasks";
pub const VIEW_PREVIEW: &str = "lapis.view.preview";
pub const VIEW_ASSISTANT: &str = "lapis.view.assistant";
pub const VIEW_TERMINAL: &str = "lapis.view.terminal";
pub const VIEW_RESOURCES: &str = "lapis.view.resources";
pub const VIEW_COMMAND_SEARCH: &str = "lapis.view.command-search";
pub const VIEW_SETTINGS: &str = "lapis.view.settings";
pub const VIEW_NOTIFICATIONS: &str = "lapis.view.notifications";
