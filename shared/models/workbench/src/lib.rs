//! ClientとHostが共有する作業画面の配置値。

use lapis_editor_core::ConversationId;
use serde::{Deserialize, Serialize};

mod replica;
pub use replica::{WorkbenchReplica, WorkbenchWritePlan};

/// 表示状態の正本と、競合時に保持された現在値。
pub enum WorkbenchUpdate {
    Updated(VersionedViewState),
    Conflict(VersionedViewState),
}

/// Client側からHostの作業状態を扱う境界。呼び出しはUIスレッド外で行う。
pub trait WorkbenchConnection: Send + Sync {
    fn get(&self, workspace: &lapis_editor_core::WorkspaceId)
    -> Result<VersionedViewState, String>;
    fn update(
        &self,
        workspace: &lapis_editor_core::WorkspaceId,
        expected: u64,
        view: ConversationViewState,
    ) -> Result<WorkbenchUpdate, String>;
    fn subscribe(
        &self,
        workspace: &lapis_editor_core::WorkspaceId,
    ) -> Result<Box<dyn WorkbenchSubscription>, String>;
}

/// 通知が欠落または切断した場合はエラーとし、Clientが正本を再取得する。
pub trait WorkbenchSubscription: Send {
    fn recv_timeout(&mut self, timeout: std::time::Duration) -> Result<Option<u64>, String>;
}

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

/// Hostが保持する表示状態。revisionは同時更新の検出に使う。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct VersionedViewState {
    pub revision: u64,
    pub view: Option<ConversationViewState>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewUpdateError {
    Conflict { expected: u64, actual: u64 },
    RevisionOverflow,
}

impl VersionedViewState {
    /// 未登録状態だけがrevision 0で、更新済み状態には必ず表示値がある。
    pub fn is_consistent(&self) -> bool {
        (self.revision == 0) == self.view.is_none()
    }

    /// 呼出元が参照したrevisionから変化していない場合だけ更新する。
    pub fn update(
        &mut self,
        expected: u64,
        view: ConversationViewState,
    ) -> Result<u64, ViewUpdateError> {
        if expected != self.revision {
            return Err(ViewUpdateError::Conflict {
                expected,
                actual: self.revision,
            });
        }
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(ViewUpdateError::RevisionOverflow)?;
        self.view = Some(view);
        self.revision = revision;
        Ok(revision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_update_does_not_replace_the_accepted_view() {
        let mut state = VersionedViewState::default();
        let first = ConversationViewState {
            show_panel_controls: false,
            ..Default::default()
        };
        assert_eq!(state.update(0, first.clone()), Ok(1));
        assert_eq!(
            state.update(0, ConversationViewState::default()),
            Err(ViewUpdateError::Conflict {
                expected: 0,
                actual: 1
            })
        );
        assert_eq!(state.view, Some(first));
        assert_eq!(state.revision, 1);
    }

    #[test]
    fn overflow_does_not_replace_the_view() {
        let mut state = VersionedViewState {
            revision: u64::MAX,
            ..Default::default()
        };
        let previous = state.clone();
        let next = ConversationViewState {
            show_panel_controls: false,
            ..Default::default()
        };
        assert_eq!(
            state.update(u64::MAX, next),
            Err(ViewUpdateError::RevisionOverflow)
        );
        assert_eq!(state, previous);
    }
}
