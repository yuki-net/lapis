use editor_core::DocumentId;
use extension_ui_contract::ViewId;
use std::path::PathBuf;

use crate::workbench::{ViewInstanceKey, ViewScope, WorkbenchMode};

#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = lapis, no_json)]
pub struct InvokeCommand(pub String);

pub const EXTRA_COMMANDS: &[&str] = &[
    crate::ids::COMMAND_OPEN_WORKSPACE,
    crate::ids::COMMAND_TOGGLE_PANEL_CONTROLS,
    crate::ids::COMMAND_WORKBENCH_IDE,
    crate::ids::COMMAND_WORKBENCH_AGENT,
    crate::ids::COMMAND_BIND_VIEW_SHARED,
    crate::ids::COMMAND_BIND_VIEW_TASK,
    #[cfg(debug_assertions)]
    crate::ids::COMMAND_TOGGLE_DEVTOOL,
];

#[derive(Clone)]
pub enum UiCommand {
    NewDocument,
    OpenFile,
    OpenWorkspace,
    OpenRecent(PathBuf),
    OpenPath(PathBuf),
    Save,
    SaveAs,
    CloseWorkspace,
    NewFolder,
    CloneRepository,
    NewWindow,
    ConnectHost,
    SwitchWorkspace(String),
    CloseWindow,
    #[cfg_attr(target_family = "wasm", allow(dead_code))]
    MoveTab(crate::DraggedPanelTab, crate::PanelId),
    ReleaseTab(crate::DraggedPanelTab, crate::ScreenPoint),
    Undo,
    Redo,
    Find,
    FindNext,
    FindWorkspace,
    Complete,
    GoToDefinition,
    Quit,
    Cut,
    Copy,
    Paste,
    ActivateDocument(DocumentId),
    /// 外部 Resource を参照するタブを操作対象にする。Resource の意味は View が決める。
    ActivateViewResource {
        view: ViewId,
        resource: String,
    },
    CloseDocument(DocumentId),
    CloseTabs(Vec<crate::PanelTab>),
    SelectTask(editor_core::ConversationId),
    SetWorkbenchMode(WorkbenchMode),
    BindViewToSelectedTask(crate::workbench::ViewInstanceKey),
    SetViewBinding {
        view: ViewInstanceKey,
        scope: ViewScope,
    },
    AcceptRemoteWorkbench,
    RetryLocalWorkbench,
    QuickSearch,
}
