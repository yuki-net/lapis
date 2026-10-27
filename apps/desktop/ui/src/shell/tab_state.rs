use crate::extension_ui::ViewId;
use lapis_editor_core::DocumentId;

pub(crate) use crate::extension_ui::ViewInstanceId;

/// Panel内のTabの所属を識別する。ZoneやResourceの識別子とは独立する。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct TabGroupId(pub(super) u64);

impl TabGroupId {
    pub(crate) fn new(value: u64) -> Self {
        Self(value)
    }

    pub(crate) fn as_str(&self) -> String {
        format!("group-{}", self.0)
    }

    pub(super) fn parse(value: &str) -> Option<Self> {
        value.strip_prefix("group-")?.parse().ok().map(Self)
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(crate) enum PanelTab {
    Document(DocumentId),
    Tool {
        view: ViewId,
        instance: ViewInstanceId,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct DraggedPanelTab {
    pub group: Option<gpui::EntityId>,
    pub source_host: super::window_group::HostId,
    pub source_group: super::TabGroupId,
    pub tab: PanelTab,
}

impl DraggedPanelTab {
    /// Returns true only when both group IDs are present and equal.
    pub(crate) fn belongs_to(&self, group: Option<gpui::EntityId>) -> bool {
        self.group.is_some() && self.group == group
    }
}

/// ドラッグ開始だけでは配置を変えない。取消後の MouseUp も移動に使わない。
#[derive(Default)]
pub(super) struct TabDragState(Option<DraggedPanelTab>);

impl TabDragState {
    /// Records a drag, replacing any pending one without changing tab placement.
    pub fn begin(&mut self, drag: DraggedPanelTab) {
        self.0 = Some(drag);
    }

    /// Clears the pending drag and reports whether one existed.
    pub fn cancel(&mut self) -> bool {
        self.0.take().is_some()
    }

    pub(crate) fn current(&self) -> Option<&DraggedPanelTab> {
        self.0.as_ref()
    }

    /// Consumes the pending drag, returning it only if GPUI still considers the release active.
    pub fn release(&mut self, active: bool) -> Option<DraggedPanelTab> {
        self.0.take().filter(|_| active)
    }
}

impl PanelTab {
    pub(crate) fn tool(view: impl Into<ViewId>) -> Self {
        let view = view.into();
        Self::Tool {
            instance: ViewInstanceId::shared_tool(&view),
            view,
        }
    }

    pub(crate) fn tool_instance(
        view: impl Into<ViewId>,
        instance: impl Into<ViewInstanceId>,
    ) -> Self {
        Self::Tool {
            view: view.into(),
            instance: instance.into(),
        }
    }

    pub(crate) fn view_id(&self) -> Option<&ViewId> {
        match self {
            Self::Tool { view, .. } => Some(view),
            Self::Document(_) => None,
        }
    }

    pub(crate) fn instance_id(&self) -> ViewInstanceId {
        match self {
            Self::Tool { instance, .. } => instance.clone(),
            Self::Document(id) => ViewInstanceId::new(format!("document:{}", id.as_str())),
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
            source_group: crate::shell::TabGroupId::new(1),
            tab: PanelTab::tool("terminal"),
        };
        assert!(drag.belongs_to(Some(1.into())));
        assert!(!drag.belongs_to(Some(2.into())));
        assert!(!drag.belongs_to(None));
    }

    #[test]
    fn cancelled_or_expired_drag_cannot_move_on_late_mouse_up() {
        let mut state = TabDragState::default();
        let drag = DraggedPanelTab {
            group: Some(1.into()),
            source_host: 0,
            source_group: crate::shell::TabGroupId::new(1),
            tab: PanelTab::tool("terminal"),
        };
        state.begin(drag.clone());
        state.cancel();
        assert!(state.release(true).is_none());
        state.begin(drag.clone());
        assert!(state.release(false).is_none());
        assert!(state.release(true).is_none());
        state.begin(drag);
        assert!(state.release(true).is_some());
        assert!(state.release(true).is_none());
    }
}
