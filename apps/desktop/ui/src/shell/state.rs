use crate::{
    extension_ui::{FeatureRegistry, PanelPosition, ViewId},
    features::id,
    theme,
};

#[derive(Clone, Copy)]
pub(crate) enum ResizeTarget {
    Left,
    Right,
    Bottom,
}

/// 一つの位置に表示されるツール群。位置以外の振る舞いはすべてのパネルで共通にする。
#[derive(Clone, Debug)]
pub(crate) struct PanelHost {
    pub position: PanelPosition,
    pub tabs: Vec<ViewId>,
    pub active: Option<ViewId>,
    pub open: bool,
    pub size: f32,
}

impl PanelHost {
    fn new(position: PanelPosition, tabs: Vec<ViewId>, open: bool, size: f32) -> Self {
        let active = tabs.first().cloned();
        Self {
            position,
            tabs,
            active,
            open,
            size,
        }
    }

    pub fn activate(&mut self, view: ViewId) {
        if !self.tabs.contains(&view) {
            self.tabs.push(view.clone());
        }
        self.active = Some(view);
        self.open = true;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.active = None;
    }
}

pub struct ShellState {
    pub left_panel: PanelHost,
    pub bottom_panel: PanelHost,
    pub right_panel: PanelHost,
    pub command_palette_open: bool,
    pub resizing: Option<ResizeTarget>,
}

impl Default for ShellState {
    fn default() -> Self {
        Self {
            left_panel: PanelHost::new(
                PanelPosition::Left,
                vec![ViewId::new(id::VIEW_FILES)],
                true,
                theme::TOOL_ISLAND_WIDTH,
            ),
            bottom_panel: PanelHost::new(
                PanelPosition::Bottom,
                Vec::new(),
                true,
                theme::BOTTOM_PANEL_HEIGHT,
            ),
            right_panel: PanelHost::new(
                PanelPosition::Right,
                Vec::new(),
                true,
                theme::SIDE_PANEL_WIDTH,
            ),
            command_palette_open: false,
            resizing: None,
        }
    }
}

impl ShellState {
    pub fn panel_mut(&mut self, position: PanelPosition) -> Option<&mut PanelHost> {
        match position {
            PanelPosition::Left => Some(&mut self.left_panel),
            PanelPosition::Bottom => Some(&mut self.bottom_panel),
            PanelPosition::Right => Some(&mut self.right_panel),
            PanelPosition::Center => None,
        }
    }

    pub fn activate_view(&mut self, position: PanelPosition, view: ViewId) {
        if let Some(panel) = self.panel_mut(position) {
            panel.activate(view);
        }
    }

    pub fn synchronize_activation(&self, registry: &mut FeatureRegistry) {
        for panel in [&self.left_panel, &self.bottom_panel, &self.right_panel] {
            registry.set_panel_active_view(
                panel.position,
                panel.open.then(|| panel.active.clone()).flatten(),
            );
        }
    }
}
