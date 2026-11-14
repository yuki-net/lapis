use editor_core::DocumentId;
use extension_ui_contract::ViewId;

pub(crate) use extension_ui_contract::ViewInstanceId;

/// Panel内のTabの所属を識別する。ZoneやResourceの識別子とは独立する。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct PanelId(pub(super) u64);

impl PanelId {
    pub(crate) fn new(value: u64) -> Self {
        Self(value)
    }

    pub(crate) fn as_str(&self) -> String {
        format!("panel-{}", self.0)
    }

    pub(super) fn parse(value: &str) -> Option<Self> {
        value.strip_prefix("panel-")?.parse().ok().map(Self)
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(crate) enum PanelTab {
    Document(DocumentId),
    Tool {
        view: ViewId,
        /// タブの個体。Resource ID とは独立し、同じ種類のタブを複数持てる。
        instance: ViewInstanceId,
        /// 機能が所有する外部 Resource を参照するタブの Resource ID。
        ///
        /// `UiContribution::references_external_resource` を宣言した View だけが持つ。
        /// Resource の生成・解決・終了は宣言した機能が行い、Layout は ID を保持するだけ。
        resource: Option<String>,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct DraggedPanelTab {
    pub group: Option<gpui::EntityId>,
    pub source_host: super::window_group::HostId,
    pub source_panel: super::PanelId,
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
            instance: crate::extension_ui::shared_tool_instance(&view),
            view,
            resource: None,
        }
    }

    pub(crate) fn tool_instance(
        view: impl Into<ViewId>,
        instance: impl Into<ViewInstanceId>,
    ) -> Self {
        Self::Tool {
            view: view.into(),
            instance: instance.into(),
            resource: None,
        }
    }

    /// 機能が所有する外部 Resource を参照するタブ。
    ///
    /// 個体はタブごとに独立し、同じ Resource へは `resource` で戻す。
    /// Resource ID の意味は宣言した機能が決める。
    pub(crate) fn external(
        view: impl Into<ViewId>,
        instance: impl Into<ViewInstanceId>,
        resource: Option<String>,
    ) -> Self {
        Self::Tool {
            view: view.into(),
            instance: instance.into(),
            resource,
        }
    }

    /// タブの参照先 Resource を差し替える。個体 ID は変えない。
    pub(crate) fn with_resource(&self, resource: Option<String>) -> Self {
        match self {
            Self::Tool { view, instance, .. } => Self::Tool {
                view: view.clone(),
                instance: instance.clone(),
                resource,
            },
            Self::Document(_) => self.clone(),
        }
    }

    /// 外部 Resource を参照するタブの Resource ID。未確定なら `None`。
    pub(crate) fn resource(&self) -> Option<&str> {
        match self {
            Self::Tool { resource, .. } => resource.as_deref(),
            Self::Document(_) => None,
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

    pub(crate) fn stable_view_key(&self) -> String {
        self.instance_id().as_str().to_owned()
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
            source_panel: crate::shell::PanelId::new(1),
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
            source_panel: crate::shell::PanelId::new(1),
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

    #[test]
    fn external_tabs_rebind_their_resource_without_changing_the_instance() {
        let tab = PanelTab::external("lapis.view.terminal", "terminal:one", None);
        let bound = tab.with_resource(Some("terminal-a".into()));
        assert_eq!(tab.resource(), None);
        assert_eq!(bound.resource(), Some("terminal-a"));
        assert_eq!(tab.instance_id(), bound.instance_id());
        assert_eq!(
            bound.view_id().map(ViewId::as_str),
            Some("lapis.view.terminal")
        );
    }
}
