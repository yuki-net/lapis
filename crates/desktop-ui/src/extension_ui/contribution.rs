use super::{CommandId, IconId, MessageId, ViewId};

/// ツールを収容するパネルの位置。位置はツールの種類ではなくレイアウト状態です。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PanelPosition {
    Left,
    Center,
    Bottom,
    Right,
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
}

impl UiContribution {
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
            allowed_panels: default_panel_for_slot(slot).into_iter().collect(),
            slot,
            title: title.into(),
            icon: icon.into(),
            command: None,
            order,
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
        }
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
