//! ClientとHostが共有する作業画面の配置値。

use editor_core::{ConversationId, DocumentId, WorkspaceId};
use serde::{Deserialize, Deserializer, Serialize};
use terminal::TerminalId;
use workspace::WorkspaceRelativePath;

mod replica;
pub use replica::{WorkbenchReplica, WorkbenchWritePlan};

/// 表示状態の正本と、競合時に保持された現在値。
pub enum WorkbenchUpdate {
    Updated(VersionedViewState),
    Conflict(VersionedViewState),
}

/// Client側からHostの作業状態を扱う境界。呼び出しはUIスレッド外で行う。
pub trait WorkbenchConnection: Send + Sync {
    fn get(&self, workspace: &editor_core::WorkspaceId) -> Result<VersionedViewState, String>;
    fn update(
        &self,
        workspace: &editor_core::WorkspaceId,
        expected: u64,
        view: ConversationViewState,
    ) -> Result<WorkbenchUpdate, String>;
    fn subscribe(
        &self,
        workspace: &editor_core::WorkspaceId,
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
    /// Workspaceに属する文書と表示位置。端末側の絶対pathは含めない。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<SharedWorkspaceView>,
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
            workspace: None,
        }
    }
}

impl ConversationViewState {
    /// 共有Workbenchへ通知できる状態に整える。
    /// 配置とタブ一覧は共有し、Workspaceの表示位置とアクティブタブはClient固有に残す。
    pub fn for_shared_workbench(mut self) -> Self {
        self.workspace = None;
        if let Some(layout) = &mut self.layout {
            layout.clear_active_tabs();
        }
        if let Some(layout) = &mut self.workbench.ide_layout {
            if let Some(layout) = &mut layout.layout {
                layout.clear_active_tabs();
            }
        }
        for layout in self.workbench.task_layouts.values_mut() {
            if let Some(layout) = &mut layout.layout {
                layout.clear_active_tabs();
            }
        }
        self
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SharedWorkspaceView {
    pub workspace_id: WorkspaceId,
    pub open_documents: Vec<SharedDocumentView>,
    pub active_path: Option<WorkspaceRelativePath>,
    pub active_document_id: Option<DocumentId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SharedDocumentView {
    pub document_id: Option<DocumentId>,
    pub path: Option<WorkspaceRelativePath>,
    pub cursor_char: u64,
    pub selection_start: u64,
    pub selection_end: u64,
    pub scroll_x: f32,
    pub scroll_y: f32,
    pub draft_content: Option<String>,
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

impl ZoneLayoutViewState {
    /// アクティブタブはClientごとに独立するため、共有状態から外す。
    fn clear_active_tabs(&mut self) {
        for zone in &mut self.zones {
            for panel in &mut zone.panels {
                panel.active_tab = None;
            }
        }
    }

    /// 共有状態に含まれないアクティブタブを、現在のClientの選択で補う。
    /// 同じPanelに残っているタブだけを対象にし、別Clientの構成変更を打ち消さない。
    pub fn retain_active_tabs_from(&mut self, local: &Self) {
        for zone in &mut self.zones {
            let Some(local_zone) = local
                .zones
                .iter()
                .find(|candidate| candidate.position == zone.position)
            else {
                continue;
            };
            for panel in &mut zone.panels {
                if panel.active_tab.is_some() {
                    continue;
                }
                let Some(local_panel) = local_zone
                    .panels
                    .iter()
                    .find(|candidate| candidate.id == panel.id)
                else {
                    continue;
                };
                let Some(active) = &local_panel.active_tab else {
                    continue;
                };
                if panel.tabs.contains(active) {
                    panel.active_tab = Some(active.clone());
                }
            }
        }
    }
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
    #[serde(default)]
    pub tabs: Vec<TabRef>,
    /// ピン留めされたタブ。古い保存データには存在しないため default で補う。
    #[serde(default)]
    pub pinned_tabs: Vec<TabRef>,
    #[serde(default)]
    pub active_tab: Option<TabRef>,
    pub open: bool,
}

/// Panelに置かれたタブが参照するResource。Viewの所有権は持たない。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum TabRef {
    Document {
        id: DocumentId,
    },
    Terminal {
        id: TerminalId,
    },
    /// ViewInstanceId。同じViewを複数開いても個体を区別できる。
    View {
        id: String,
    },
}

impl TabRef {
    /// 旧形式の文字列を読み、現在の参照へ変換する。
    pub fn from_legacy(value: &str) -> Option<Self> {
        if let Some(id) = value.strip_prefix("document:") {
            return (!id.is_empty()).then(|| Self::Document {
                id: DocumentId::new(id),
            });
        }
        let (_, instance) = value.strip_prefix("tool:")?.split_once(':')?;
        (!instance.is_empty()).then(|| Self::View {
            id: instance.to_owned(),
        })
    }
}

impl<'de> Deserialize<'de> for TabRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Encoded {
            Legacy(String),
            Current { kind: String, id: String },
        }

        match Encoded::deserialize(deserializer)? {
            Encoded::Legacy(value) => {
                Self::from_legacy(&value).ok_or_else(|| serde::de::Error::custom("不正なタブ参照"))
            }
            Encoded::Current { kind, id } => match kind.as_str() {
                "document" if !id.is_empty() => Ok(Self::Document {
                    id: DocumentId::new(id),
                }),
                "terminal" if !id.is_empty() => Ok(Self::Terminal {
                    id: TerminalId::new(id),
                }),
                "view" if !id.is_empty() => Ok(Self::View { id }),
                _ => Err(serde::de::Error::custom("不正なタブ参照")),
            },
        }
    }
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
    fn old_layout_without_workspace_round_trips_without_adding_a_new_field() {
        let old = serde_json::json!({"show_panel_controls": false});
        let view: ConversationViewState = serde_json::from_value(old).unwrap();
        assert!(view.workspace.is_none());
        let encoded = serde_json::to_value(view).unwrap();
        assert!(encoded.get("workspace").is_none());
    }

    #[test]
    fn legacy_string_tabs_are_readable_and_new_tabs_are_resource_refs() {
        let old = serde_json::json!({
            "id": "panel-1",
            "tabs": [
                "document:doc-a",
                "tool:lapis.view.terminal:lapis.view.terminal:token"
            ],
            "pinned_tabs": ["document:doc-a"],
            "active_tab": "tool:lapis.view.terminal:lapis.view.terminal:token",
            "open": true
        });
        let panel: ZonePanelViewState = serde_json::from_value(old).unwrap();
        assert_eq!(
            panel.tabs,
            vec![
                TabRef::Document {
                    id: DocumentId::new("doc-a")
                },
                TabRef::View {
                    id: "lapis.view.terminal:token".into()
                }
            ]
        );
        assert_eq!(panel.pinned_tabs, vec![panel.tabs[0].clone()]);
        assert_eq!(panel.active_tab, Some(panel.tabs[1].clone()));

        let encoded = serde_json::to_value(ZonePanelViewState {
            id: "panel-2".into(),
            tabs: vec![TabRef::Terminal {
                id: TerminalId::new("host-terminal"),
            }],
            pinned_tabs: Vec::new(),
            active_tab: None,
            open: true,
        })
        .unwrap();
        assert_eq!(
            encoded["tabs"][0],
            serde_json::json!({"kind": "terminal", "id": "host-terminal"})
        );
    }

    fn tabs_layout(active: Option<TabRef>) -> ZoneLayoutViewState {
        let tabs = vec![
            TabRef::Document {
                id: DocumentId::new("doc-a"),
            },
            TabRef::Document {
                id: DocumentId::new("doc-b"),
            },
        ];
        ZoneLayoutViewState {
            version: ZONE_LAYOUT_VERSION,
            zones: vec![ZoneViewState {
                position: "main".into(),
                open: true,
                extent_ratio: 1.0,
                layout: None,
                panels: vec![ZonePanelViewState {
                    id: "panel-1".into(),
                    tabs,
                    pinned_tabs: Vec::new(),
                    active_tab: active,
                    open: true,
                }],
            }],
            bottom_span_left: false,
            bottom_span_right: false,
        }
    }

    #[test]
    fn shared_workbench_omits_client_active_tabs_and_keeps_local_selection() {
        let active = TabRef::Document {
            id: DocumentId::new("doc-a"),
        };
        let layout = tabs_layout(Some(active.clone()));
        let local = layout.clone();
        let view = ConversationViewState {
            layout: Some(layout.clone()),
            workbench: WorkbenchViewState {
                ide_layout: Some(WorkbenchLayoutState {
                    layout: Some(layout.clone()),
                    show_panel_controls: true,
                }),
                task_layouts: [(
                    ConversationId::new("task-a"),
                    WorkbenchLayoutState {
                        layout: Some(layout.clone()),
                        show_panel_controls: true,
                    },
                )]
                .into(),
                ..Default::default()
            },
            workspace: Some(SharedWorkspaceView {
                workspace_id: WorkspaceId::new("workspace-1"),
                open_documents: Vec::new(),
                active_path: None,
                active_document_id: None,
            }),
            ..Default::default()
        };

        let shared = view.for_shared_workbench();
        assert!(shared.workspace.is_none());
        assert_eq!(
            shared.layout.as_ref().unwrap().zones[0].panels[0].active_tab,
            None
        );
        assert_eq!(
            shared.layout.as_ref().unwrap().zones[0].panels[0].tabs,
            local.zones[0].panels[0].tabs
        );
        assert_eq!(
            shared
                .workbench
                .ide_layout
                .as_ref()
                .unwrap()
                .layout
                .as_ref()
                .unwrap()
                .zones[0]
                .panels[0]
                .active_tab,
            None
        );
        assert_eq!(
            shared.workbench.task_layouts[&ConversationId::new("task-a")]
                .layout
                .as_ref()
                .unwrap()
                .zones[0]
                .panels[0]
                .active_tab,
            None
        );

        let mut restored = shared.layout.unwrap();
        restored.retain_active_tabs_from(&local);
        assert_eq!(restored, local);
    }

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
