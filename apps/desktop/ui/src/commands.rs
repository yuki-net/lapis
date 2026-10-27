use crate::extension_ui::ThemeId;
use lapis_editor_core::DocumentId;
use std::path::PathBuf;

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
    MoveTab(crate::shell::DraggedPanelTab, crate::shell::PanelId),
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
    Theme(ThemeId),
    QuickSearch,
}
