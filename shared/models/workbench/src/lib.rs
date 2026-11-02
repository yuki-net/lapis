//! ClientとHostが共有する作業画面の配置値。

use lapis_editor_core::ConversationId;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConversationViewState {
    #[serde(default)]
    pub layout: Option<ZoneLayoutViewState>,
    #[serde(default = "default_show_panel_controls")]
    pub show_panel_controls: bool,
    #[serde(default)]
    pub workbench: WorkbenchViewState,
}

fn default_show_panel_controls() -> bool {
    true
}

impl Default for ConversationViewState {
    fn default() -> Self {
        Self {
            layout: None,
            show_panel_controls: true,
            workbench: WorkbenchViewState::default(),
        }
    }
}

/// Viewの配置とTaskへの参照。Host上の実行資源は含めない。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkbenchViewState {
    pub mode: WorkbenchModeState,
    pub selected_task: Option<ConversationId>,
    pub bindings: Vec<ViewBindingState>,
    pub ide_layout: Option<WorkbenchLayoutState>,
    pub ide_context: Option<ConversationId>,
    pub task_layouts: std::collections::BTreeMap<ConversationId, WorkbenchLayoutState>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkbenchLayoutState {
    pub layout: Option<ZoneLayoutViewState>,
    pub show_panel_controls: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkbenchModeState {
    #[default]
    Ide,
    Agent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewBindingState {
    pub view: String,
    pub scope: ViewScopeState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ViewScopeState {
    SharedWorkbench,
    Task(ConversationId),
    FollowSelectedTask,
    Resource(String),
}

pub const ZONE_LAYOUT_VERSION: u16 = 3;

/// Viewの配置だけを保存し、DocumentやTerminalの所有権は含めない。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ZoneLayoutViewState {
    pub version: u16,
    pub zones: Vec<ZoneViewState>,
    pub bottom_span_left: bool,
    pub bottom_span_right: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ZoneViewState {
    pub position: String,
    pub open: bool,
    pub extent_ratio: f32,
    pub layout: Option<PanelLayoutViewState>,
    pub panels: Vec<ZonePanelViewState>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PanelLayoutViewState {
    Panel {
        id: String,
    },
    Arrangement {
        direction: String,
        children: Vec<PanelLayoutViewState>,
        weights: Vec<f32>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ZonePanelViewState {
    pub id: String,
    pub tabs: Vec<String>,
    pub active_tab: Option<String>,
    pub open: bool,
}
