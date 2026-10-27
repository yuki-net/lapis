use std::collections::HashSet;

use super::{CommandId, IconId, MessageId, ViewId, WorkspaceCapabilityId};

/// DesktopのZone位置。Viewの種類ではなくレイアウト上の所属です。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ZonePosition {
    Left,
    Main,
    Bottom,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolInstancePolicy {
    Shared,
    Multiple,
}

/// Viewを配置できる範囲を登録する。PanelやWindow側で機能別の例外を作らないための宣言。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ViewPlacementCapabilities {
    pub detach: bool,
    pub independent_window: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ViewVisibility {
    #[default]
    Always,
    WorkspaceOpen,
    WorkspaceCapability(WorkspaceCapabilityId),
}

impl ViewVisibility {
    pub(crate) fn is_visible(
        &self,
        workspace_open: bool,
        capabilities: &HashSet<WorkspaceCapabilityId>,
    ) -> bool {
        match self {
            Self::Always => true,
            Self::WorkspaceOpen => workspace_open,
            Self::WorkspaceCapability(capability) => capabilities.contains(capability),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollAxis {
    Vertical,
    Horizontal,
    Both,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelScrollPolicy {
    Panel(ScrollAxis),
    FeatureOwned,
    Disabled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UiSlot {
    CommandPalette,
    ToolDock,
    SideDock,
    BottomDock,
    StatusBar,
    EditorDecoration,
    SettingsPage,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiContribution {
    pub view: Option<ViewId>,
    pub slot: UiSlot,
    pub default_zone: Option<ZonePosition>,
    pub allowed_zones: Vec<ZonePosition>,
    pub category: Option<MessageId>,
    pub title: MessageId,
    pub icon: IconId,
    pub command: Option<CommandId>,
    pub order: i32,
    pub instance_policy: ToolInstancePolicy,
    pub placement: ViewPlacementCapabilities,
    pub visibility: ViewVisibility,
    pub scroll_policy: PanelScrollPolicy,
}

impl UiContribution {
    pub fn settings_view(
        view: impl Into<ViewId>,
        title: impl Into<MessageId>,
        icon: impl Into<IconId>,
    ) -> Self {
        Self {
            view: Some(view.into()),
            slot: UiSlot::SettingsPage,
            default_zone: Some(ZonePosition::Main),
            allowed_zones: vec![ZonePosition::Main],
            category: None,
            title: title.into(),
            icon: icon.into(),
            command: None,
            order: 0,
            instance_policy: ToolInstancePolicy::Shared,
            placement: ViewPlacementCapabilities::default(),
            visibility: ViewVisibility::Always,
            scroll_policy: PanelScrollPolicy::Panel(ScrollAxis::Both),
        }
    }

    pub fn view(
        view: impl Into<ViewId>,
        slot: UiSlot,
        title: impl Into<MessageId>,
        icon: impl Into<IconId>,
        order: i32,
    ) -> Self {
        Self {
            view: Some(view.into()),
            default_zone: default_zone_for_slot(slot),
            allowed_zones: all_zones_for_slot(slot),
            slot,
            category: None,
            title: title.into(),
            icon: icon.into(),
            command: None,
            order,
            instance_policy: ToolInstancePolicy::Shared,
            placement: ViewPlacementCapabilities::default(),
            visibility: ViewVisibility::Always,
            scroll_policy: PanelScrollPolicy::Panel(ScrollAxis::Both),
        }
    }

    /// ツールの既定位置と、利用者が移動可能な位置を明示して登録する。
    pub fn panel_view(
        view: impl Into<ViewId>,
        default_zone: ZonePosition,
        allowed_zones: impl IntoIterator<Item = ZonePosition>,
        title: impl Into<MessageId>,
        icon: impl Into<IconId>,
        order: i32,
    ) -> Self {
        Self {
            view: Some(view.into()),
            slot: UiSlot::ToolDock,
            default_zone: Some(default_zone),
            allowed_zones: allowed_zones.into_iter().collect(),
            category: None,
            title: title.into(),
            icon: icon.into(),
            command: None,
            order,
            instance_policy: ToolInstancePolicy::Shared,
            placement: ViewPlacementCapabilities::default(),
            visibility: ViewVisibility::Always,
            scroll_policy: PanelScrollPolicy::Panel(ScrollAxis::Both),
        }
    }

    pub fn command(
        command: impl Into<CommandId>,
        title: impl Into<MessageId>,
        icon: impl Into<IconId>,
        order: i32,
    ) -> Self {
        Self {
            view: None,
            slot: UiSlot::CommandPalette,
            default_zone: None,
            allowed_zones: Vec::new(),
            category: None,
            title: title.into(),
            icon: icon.into(),
            command: Some(command.into()),
            order,
            instance_policy: ToolInstancePolicy::Shared,
            placement: ViewPlacementCapabilities::default(),
            visibility: ViewVisibility::Always,
            scroll_policy: PanelScrollPolicy::Disabled,
        }
    }

    pub fn category(mut self, category: impl Into<MessageId>) -> Self {
        self.category = Some(category.into());
        self
    }

    pub fn multiple_instances(mut self) -> Self {
        self.instance_policy = ToolInstancePolicy::Multiple;
        self
    }

    pub fn supports_detach(mut self) -> Self {
        self.placement.detach = true;
        self
    }

    pub fn supports_independent_window(mut self) -> Self {
        self.placement.independent_window = true;
        self
    }

    pub fn visible_when_workspace_open(mut self) -> Self {
        self.visibility = ViewVisibility::WorkspaceOpen;
        self
    }

    pub fn visible_when_capability(mut self, capability: impl Into<WorkspaceCapabilityId>) -> Self {
        self.visibility = ViewVisibility::WorkspaceCapability(capability.into());
        self
    }

    pub fn feature_owned_scroll(mut self) -> Self {
        self.scroll_policy = PanelScrollPolicy::FeatureOwned;
        self
    }

    pub fn scroll_disabled(mut self) -> Self {
        self.scroll_policy = PanelScrollPolicy::Disabled;
        self
    }
}

const fn default_zone_for_slot(slot: UiSlot) -> Option<ZonePosition> {
    match slot {
        UiSlot::ToolDock => Some(ZonePosition::Left),
        UiSlot::SideDock => Some(ZonePosition::Right),
        UiSlot::BottomDock => Some(ZonePosition::Bottom),
        UiSlot::CommandPalette
        | UiSlot::StatusBar
        | UiSlot::EditorDecoration
        | UiSlot::SettingsPage => None,
    }
}

fn all_zones_for_slot(slot: UiSlot) -> Vec<ZonePosition> {
    match slot {
        UiSlot::ToolDock | UiSlot::SideDock | UiSlot::BottomDock => vec![
            ZonePosition::Main,
            ZonePosition::Left,
            ZonePosition::Bottom,
            ZonePosition::Right,
        ],
        _ => default_zone_for_slot(slot).into_iter().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_registration_keeps_instance_and_placement_capabilities_together() {
        let contribution = UiContribution::view(
            "view.terminal",
            UiSlot::ToolDock,
            "view.terminal.title",
            "terminal",
            10,
        )
        .category("category.runtime")
        .multiple_instances()
        .supports_detach()
        .supports_independent_window();

        assert_eq!(
            contribution.category,
            Some(MessageId::new("category.runtime"))
        );
        assert_eq!(contribution.instance_policy, ToolInstancePolicy::Multiple);
        assert!(contribution.placement.detach);
        assert!(contribution.placement.independent_window);
    }
}
