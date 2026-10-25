use crate::extension_ui::ThemeId;
use lapis_editor_core::DocumentId;
use std::path::PathBuf;

use crate::workbench::{ViewInstanceKey, ViewScope, WorkbenchMode};

#[derive(Clone)]
pub(crate) enum UiCommand {
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
    CloseWindow,
    DetachTab(crate::shell::PanelTab),
    DockTab(crate::shell::PanelTab),
    MoveTab(crate::shell::DraggedPanelTab, crate::shell::TabGroupId),
    ReleaseTab(
        crate::shell::DraggedPanelTab,
        crate::window_host::drag::ScreenPoint,
    ),
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    ActivateDocument(DocumentId),
    CloseDocument(DocumentId),
    SelectTask(lapis_editor_core::ConversationId),
    SetWorkbenchMode(WorkbenchMode),
    BindViewToSelectedTask(crate::workbench::ViewInstanceKey),
    SetViewBinding {
        view: ViewInstanceKey,
        scope: ViewScope,
    },
    Theme(ThemeId),
    QuickSearch,
}
