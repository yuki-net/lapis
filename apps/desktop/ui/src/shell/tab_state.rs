use crate::extension_ui::{PanelPosition, ViewId};
use lapis_editor_core::DocumentId;

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(crate) enum PanelTab {
    Document(DocumentId),
    Tool(ViewId),
}

#[derive(Clone, Debug)]
pub(crate) struct DraggedPanelTab {
    pub group: Option<gpui::EntityId>,
    pub source_host: super::window_group::HostId,
    pub source_panel: PanelPosition,
    pub tab: PanelTab,
}

impl DraggedPanelTab {
    pub(crate) fn belongs_to(&self, group: Option<gpui::EntityId>) -> bool {
        self.group.is_some() && self.group == group
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_rejects_another_group_even_when_host_and_tab_ids_match() {
        let drag = DraggedPanelTab {
            group: Some(1.into()),
            source_host: 1,
            source_panel: PanelPosition::Main,
            tab: PanelTab::tool("terminal"),
        };
        assert!(drag.belongs_to(Some(1.into())));
        assert!(!drag.belongs_to(Some(2.into())));
        assert!(!drag.belongs_to(None));
    }
}
