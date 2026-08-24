use super::{CommandId, IconId, MessageId, ViewId};

/// ツールを収容するパネルの位置。位置はツールの種類ではなくレイアウト状態です。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PanelPosition {
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
    pub default_panel: Option<PanelPosition>,
    pub allowed_panels: Vec<PanelPosition>,
    pub title: MessageId,
    pub icon: IconId,
    pub command: Option<CommandId>,
    pub order: i32,
    pub instance_policy: ToolInstancePolicy,
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
            default_panel: Some(PanelPosition::Main),
            allowed_panels: vec![PanelPosition::Main],
            title: title.into(),
            icon: icon.into(),
            command: None,
            order: 0,
            instance_policy: ToolInstancePolicy::Shared,
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
            default_panel: default_panel_for_slot(slot),
            allowed_panels: all_panels_for_slot(slot),
            slot,
            title: title.into(),
            icon: icon.into(),
            command: None,
            order,
            instance_policy: ToolInstancePolicy::Shared,
            scroll_policy: PanelScrollPolicy::Panel(ScrollAxis::Both),
        }
    }

    /// ツールの既定位置と、利用者が移動可能な位置を明示して登録する。
    pub fn panel_view(
        view: impl Into<ViewId>,
        default_panel: PanelPosition,
        allowed_panels: impl IntoIterator<Item = PanelPosition>,
        title: impl Into<MessageId>,
        icon: impl Into<IconId>,
        order: i32,
    ) -> Self {
        Self {
            view: Some(view.into()),
            slot: UiSlot::ToolDock,
            default_panel: Some(default_panel),
            allowed_panels: allowed_panels.into_iter().collect(),
            title: title.into(),
            icon: icon.into(),
            command: None,
            order,
            instance_policy: ToolInstancePolicy::Shared,
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
            default_panel: None,
            allowed_panels: Vec::new(),
            title: title.into(),
            icon: icon.into(),
            command: Some(command.into()),
            order,
            instance_policy: ToolInstancePolicy::Shared,
            scroll_policy: PanelScrollPolicy::Disabled,
        }
    }

    pub fn multiple_instances(mut self) -> Self {
        self.instance_policy = ToolInstancePolicy::Multiple;
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

const fn default_panel_for_slot(slot: UiSlot) -> Option<PanelPosition> {
    match slot {
        UiSlot::ToolDock => Some(PanelPosition::Left),
        UiSlot::SideDock => Some(PanelPosition::Right),
        UiSlot::BottomDock => Some(PanelPosition::Bottom),
        UiSlot::CommandPalette
        | UiSlot::StatusBar
        | UiSlot::EditorDecoration
        | UiSlot::SettingsPage => None,
    }
}

fn all_panels_for_slot(slot: UiSlot) -> Vec<PanelPosition> {
    match slot {
        UiSlot::ToolDock | UiSlot::SideDock | UiSlot::BottomDock => vec![
            PanelPosition::Main,
            PanelPosition::Left,
            PanelPosition::Bottom,
            PanelPosition::Right,
        ],
        _ => default_panel_for_slot(slot).into_iter().collect(),
    }
}
