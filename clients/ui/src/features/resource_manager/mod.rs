use crate::{client_adapter::ClientWorker, features::id};
use extension_ui_contract::{
    ActivationCondition, FeatureDescriptor, UiContribution, UiSlot, ViewId,
};
use process::{ProcessKind, ProcessOrigin, ProcessRoot, ProcessRootId, ProcessSnapshot};

mod state;
mod view;

pub(crate) use state::ProcessWorker;

pub(crate) struct ResourceManagerFeature {
    worker: ClientWorker<ProcessWorker>,
    workspace: Option<editor_core::WorkspaceId>,
    pub(super) state: ProcessPanelState,
    busy: bool,
    scheduled: bool,
    refresh_generation: u64,
    /// パネルが画面に出ているか。非表示の間は取得を止める。
    visible: bool,
}

/// Resource Manager パネルの表示状態。worker や GPUI に依存せず更新できる。
#[derive(Default)]
pub(super) struct ProcessPanelState {
    pub(super) snapshot: ProcessSnapshot,
    pub(super) messages: ResourceMessages,
    pub(super) pending_terminate: Option<ProcessRootId>,
}

impl ProcessPanelState {
    /// パネルに表示する（再取得メッセージ, 終了エラー）。
    pub(super) fn status_lines(&self) -> (Option<&str>, Option<&str>) {
        let refresh = (!self.messages.refresh.is_empty()).then_some(self.messages.refresh.as_str());
        (refresh, self.messages.terminate.error())
    }

    /// 再取得結果を適用する。終了エラーには触れない。
    pub(super) fn apply_refresh(
        &mut self,
        result: Result<(ProcessSnapshot, Option<String>), String>,
    ) {
        match result {
            Ok((snapshot, host_error)) => {
                self.snapshot = snapshot;
                self.messages.refresh_succeeded(host_error);
            }
            Err(error) => self.messages.refresh_failed(error),
        }
    }

    pub(super) fn begin_termination(&mut self, root_id: ProcessRootId) {
        self.pending_terminate = Some(root_id);
        self.messages.terminate.begin();
    }

    pub(super) fn cancel_termination(&mut self) {
        self.pending_terminate = None;
        self.messages.terminate.begin();
    }

    pub(super) fn termination_submitted(&mut self, result: Result<(), String>) {
        self.messages.terminate.submitted(result);
    }

    pub(super) fn termination_finished(&mut self, result: Result<(), String>) {
        self.messages.terminate.finished(result);
    }
}

/// 定期再取得のメッセージと終了操作の失敗を別々に保持する。
#[derive(Default)]
pub(super) struct ResourceMessages {
    pub(super) refresh: String,
    pub(super) terminate: TerminationFeedback,
}

impl ResourceMessages {
    pub(super) fn refresh_succeeded(&mut self, host_error: Option<String>) {
        self.refresh = host_error.unwrap_or_default();
    }

    pub(super) fn refresh_failed(&mut self, error: String) {
        self.refresh = error;
    }
}

/// 終了操作の失敗は、定期再取得のメッセージとは別に保持する。
#[derive(Default)]
pub(super) struct TerminationFeedback {
    error: Option<String>,
}

impl TerminationFeedback {
    pub(super) fn begin(&mut self) {
        self.error = None;
    }

    pub(super) fn submitted(&mut self, result: Result<(), String>) {
        if let Err(error) = result {
            self.error = Some(error);
        }
    }

    pub(super) fn finished(&mut self, result: Result<(), String>) {
        self.error = result.err();
    }

    pub(super) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

pub(super) fn descriptor() -> FeatureDescriptor {
    FeatureDescriptor::bundled(
        id::FEATURE_RESOURCES,
        [ActivationCondition::OnView(ViewId::new(id::VIEW_RESOURCES))],
    )
    .contributes(UiContribution::view(
        id::VIEW_RESOURCES,
        UiSlot::BottomDock,
        "view.resources",
        "problems",
        40,
    ))
    .requires("workspace.process.read")
}

/// 一覧に出す行。root とその子孫を深さ付きで返す。
pub(crate) enum ResourceRow<'a> {
    Origin(ProcessOrigin),
    Root(&'a ProcessRoot),
    Child {
        root: &'a ProcessRoot,
        node: &'a process::ProcessNode,
    },
}

pub(crate) fn rows(snapshot: &ProcessSnapshot) -> Vec<ResourceRow<'_>> {
    let mut rows = Vec::new();
    for origin in [ProcessOrigin::Host, ProcessOrigin::Desktop] {
        let roots = snapshot
            .roots
            .iter()
            .filter(|root| root.origin == origin)
            .collect::<Vec<_>>();
        if roots.is_empty() {
            continue;
        }
        rows.push(ResourceRow::Origin(origin));
        for root in roots {
            rows.push(ResourceRow::Root(root));
            for node in root.nodes.iter().filter(|node| node.pid != root.pid) {
                rows.push(ResourceRow::Child { root, node });
            }
        }
    }
    rows
}

pub(crate) fn kind_label(kind: ProcessKind) -> &'static str {
    match kind {
        ProcessKind::Host => "Host",
        ProcessKind::Desktop => "Desktop",
        ProcessKind::Terminal => "Terminal",
        ProcessKind::Agent => "Agent",
        ProcessKind::LanguageServer => "LSP",
        ProcessKind::TaskRun => "Task Run",
        ProcessKind::Build => "Build",
        ProcessKind::Git => "Git",
        ProcessKind::Other => "Process",
    }
}

pub(crate) fn format_memory(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GIB {
        format!("{:.1} GB", bytes / GIB)
    } else if bytes >= MIB {
        format!("{:.1} MB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.0} KB", bytes / KIB)
    } else {
        format!("{bytes:.0} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use process::{ProcessNode, ProcessOwner, ProcessSample, ProcessState};

    fn node(pid: u32, depth: u16) -> ProcessNode {
        ProcessNode {
            pid,
            parent_pid: None,
            name: "child".into(),
            depth,
            cpu_centipercent: 0,
            memory_bytes: 0,
            started_at_ms: 0,
        }
    }

    fn root(origin: ProcessOrigin, pid: u32) -> ProcessRoot {
        ProcessRoot {
            root_id: ProcessRootId::try_new(format!("process-{pid}")).unwrap(),
            origin,
            kind: ProcessKind::Terminal,
            owner: ProcessOwner::terminal("terminal-1"),
            pid,
            label: "Terminal".into(),
            started_at_ms: 0,
            state: ProcessState::Running,
            cpu_centipercent: 0,
            memory_bytes: 0,
            samples: Vec::<ProcessSample>::new(),
            nodes: vec![node(pid, 0), node(pid + 1, 1)],
            terminable: true,
        }
    }

    #[test]
    fn rows_group_origins_and_skip_the_root_node_duplicate() {
        let snapshot = ProcessSnapshot {
            sampled_at_ms: 0,
            roots: vec![
                root(ProcessOrigin::Host, 10),
                root(ProcessOrigin::Desktop, 20),
            ],
        };
        let rows = rows(&snapshot);
        assert_eq!(rows.len(), 6);
        assert!(matches!(rows[0], ResourceRow::Origin(ProcessOrigin::Host)));
        assert!(matches!(rows[1], ResourceRow::Root(root) if root.pid == 10));
        assert!(matches!(rows[2], ResourceRow::Child { node, .. } if node.pid == 11));
        assert!(matches!(
            rows[3],
            ResourceRow::Origin(ProcessOrigin::Desktop)
        ));
    }

    #[test]
    fn memory_is_formatted_with_binary_units() {
        assert_eq!(format_memory(512), "512 B");
        assert_eq!(format_memory(2048), "2 KB");
        assert_eq!(format_memory(2 * 1024 * 1024), "2.0 MB");
        assert_eq!(format_memory(3 * 1024 * 1024 * 1024), "3.0 GB");
    }
}
