use super::{CommandId, IconId, MessageId, ViewId};

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
            slot,
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
            title: title.into(),
            icon: icon.into(),
            command: Some(command.into()),
            order,
        }
    }
}
