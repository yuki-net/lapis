use crate::extension_ui::{PanelPosition, ViewId};
use lapis_editor_core::DocumentId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PanelTab {
    Document(DocumentId),
    Tool(ViewId),
}

#[derive(Clone, Debug)]
pub(crate) struct DraggedPanelTab {
    pub source_panel: PanelPosition,
    pub tab: PanelTab,
}

impl PanelTab {
    pub(crate) fn tool(view: impl Into<ViewId>) -> Self {
        Self::Tool(view.into())
    }

    pub(crate) fn view_id(&self) -> Option<&ViewId> {
        match self {
            Self::Tool(view) => Some(view),
            Self::Document(_) => None,
        }
    }
}
