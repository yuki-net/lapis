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
