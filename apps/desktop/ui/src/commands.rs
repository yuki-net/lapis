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
    CloseWorkspace,
    NewFolder,
    CloneRepository,
    NewWindow,
    CloseWindow,
    DetachTab(crate::shell::PanelTab),
    DockTab(crate::shell::PanelTab),
    MoveTab(
        crate::shell::DraggedPanelTab,
        crate::extension_ui::PanelPosition,
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
