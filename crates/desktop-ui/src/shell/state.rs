use crate::{
    extension_ui::{FeatureRegistry, UiSlot, ViewId},
    features::id,
    theme,
};

#[derive(Clone, Copy)]
pub(crate) enum ResizeTarget {
    ToolIsland,
    SidePanel,
    BottomPanel,
}

pub struct ShellState {
    pub active_tool: ViewId,
    pub side_panel: Option<ViewId>,
    pub bottom_panel_open: bool,
    pub bottom_panel: ViewId,
    pub command_palette_open: bool,
    pub tool_island_width: f32,
    pub side_panel_width: f32,
    pub bottom_panel_height: f32,
    pub resizing: Option<ResizeTarget>,
}

impl Default for ShellState {
    fn default() -> Self {
        Self {
            active_tool: ViewId::new(id::VIEW_FILES),
            side_panel: None,
            bottom_panel_open: false,
            bottom_panel: ViewId::new(id::VIEW_TERMINAL),
            command_palette_open: false,
            tool_island_width: theme::TOOL_ISLAND_WIDTH,
            side_panel_width: theme::SIDE_PANEL_WIDTH,
            bottom_panel_height: theme::BOTTOM_PANEL_HEIGHT,
            resizing: None,
        }
    }
}

impl ShellState {
    pub fn synchronize_activation(&self, registry: &mut FeatureRegistry) {
        registry.set_active_view(UiSlot::ToolDock, Some(self.active_tool.clone()));
        registry.set_active_view(UiSlot::SideDock, self.side_panel.clone());
        registry.set_active_view(
            UiSlot::BottomDock,
            self.bottom_panel_open.then(|| self.bottom_panel.clone()),
        );
    }
}
