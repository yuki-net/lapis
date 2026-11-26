use std::time::Duration;

use desktop_client::ProcessSession;
use editor_core::WorkspaceId;
use gpui::Context;
use process::{ProcessOrigin, ProcessRootId, ProcessSnapshot};

use crate::client_adapter::ClientWorker;

use super::{ProcessPanelState, ResourceManagerFeature};

/// Resource Manager 専用 worker。Document の編集・保存と同じ worker を占有しない。
pub(crate) struct ProcessWorker {
    session: ProcessSession,
    workspace: Option<WorkspaceId>,
}

impl ProcessWorker {
    pub(crate) fn new(session: ProcessSession, workspace: Option<WorkspaceId>) -> Self {
        Self { session, workspace }
    }

    fn set_workspace(&mut self, workspace: Option<WorkspaceId>) {
        self.workspace = workspace;
    }

    fn refresh(&mut self) -> Result<(ProcessSnapshot, Option<String>), String> {
        self.session
            .refresh(self.workspace.as_ref())
            .map_err(|error| error.to_string())?;
        Ok((
            self.session.snapshot().clone(),
            self.session.host_error().map(ToOwned::to_owned),
        ))
    }

    fn terminate(&mut self, origin: ProcessOrigin, root_id: &ProcessRootId) -> Result<(), String> {
        self.session
            .terminate(self.workspace.as_ref(), origin, root_id)
            .map_err(|error| error.to_string())
    }
}

impl ResourceManagerFeature {
    pub(crate) fn new(worker: ClientWorker<ProcessWorker>, workspace: Option<WorkspaceId>) -> Self {
        Self {
            worker,
            workspace,
            state: ProcessPanelState::default(),
            busy: false,
            scheduled: false,
            refresh_generation: 0,
            visible: false,
        }
        // 初回取得はパネルが表示されたときに行う。非表示のまま要求を出さない。
    }

    /// パネルの表示状態を Shell から受け取る。表示中だけ取得を続ける。
    pub(crate) fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if visible {
            self.refresh(cx);
        }
    }

    /// Workspace の切替を専用 worker へ同期し、一覧を取り直す。
    pub(crate) fn set_workspace(&mut self, workspace: Option<WorkspaceId>, cx: &mut Context<Self>) {
        if self.workspace == workspace {
            return;
        }
        self.workspace = workspace.clone();
        self.refresh_generation = self.refresh_generation.wrapping_add(1);
        self.state = ProcessPanelState::default();
        let _ = self.worker.request(move |worker| {
            worker.set_workspace(workspace);
            Ok(())
        });
        if self.visible {
            self.refresh(cx);
        }
    }

    fn schedule(&mut self, cx: &mut Context<Self>) {
        if self.scheduled || !self.visible {
            return;
        }
        self.scheduled = true;
        cx.spawn(async move |this, cx| {
            crate::timer::Timer::after(Duration::from_secs(2)).await;
            let _ = this.update(cx, |feature, cx| {
                feature.scheduled = false;
                if feature.visible {
                    feature.refresh(cx);
                }
            });
        })
        .detach();
    }

    /// Host と Desktop の現在値を取得する。1 回に 1 要求だけを直列化する。
    ///
    /// 非表示の間は要求を出さない。表示されたときに改めて取得する。
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        if !self.visible {
            return;
        }
        if self.busy {
            self.schedule(cx);
            return;
        }
        self.busy = true;
        self.refresh_generation = self.refresh_generation.wrapping_add(1);
        let generation = self.refresh_generation;
        let admitted = self.worker.submit(
            cx,
            |worker| worker.refresh(),
            move |feature, result, cx| {
                if generation != feature.refresh_generation {
                    // Workspace 切替などで古くなった結果は捨て、次の周期で取り直す。
                    feature.busy = false;
                    feature.schedule(cx);
                    return;
                }
                feature.busy = false;
                feature.state.apply_refresh(result);
                cx.notify();
                feature.schedule(cx);
            },
        );
        if let Err(error) = admitted {
            self.busy = false;
            self.state.messages.refresh_failed(error);
            cx.notify();
            self.schedule(cx);
        }
    }

    pub(crate) fn request_terminate(&mut self, root_id: ProcessRootId, cx: &mut Context<Self>) {
        self.state.begin_termination(root_id);
        cx.notify();
    }

    pub(crate) fn cancel_terminate(&mut self, cx: &mut Context<Self>) {
        self.state.cancel_termination();
        cx.notify();
    }

    pub(crate) fn confirm_terminate(&mut self, cx: &mut Context<Self>) {
        let Some(root_id) = self.state.pending_terminate.take() else {
            return;
        };
        let origin = self
            .state
            .snapshot
            .roots
            .iter()
            .find(|root| root.root_id == root_id)
            .map(|root| root.origin)
            .unwrap_or(ProcessOrigin::Host);
        let admitted = self.worker.submit(
            cx,
            move |worker| worker.terminate(origin, &root_id),
            |feature, result, cx| {
                feature.state.termination_finished(result);
                feature.refresh(cx);
            },
        );
        if let Err(error) = admitted {
            self.state.termination_submitted(Err(error));
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::AppContext;
    use process::{
        ProcessConnection, ProcessError, ProcessErrorKind, ProcessKind, ProcessOwner, ProcessRoot,
        ProcessSnapshot, ProcessState,
    };
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    struct Host {
        workspaces: Arc<Mutex<Vec<Option<WorkspaceId>>>>,
    }

    impl ProcessConnection for Host {
        fn snapshot(&self, workspace: &WorkspaceId) -> Result<ProcessSnapshot, ProcessError> {
            self.workspaces
                .lock()
                .unwrap()
                .push(Some(workspace.clone()));
            Ok(ProcessSnapshot {
                sampled_at_ms: 1,
                roots: vec![ProcessRoot {
                    root_id: ProcessRootId::try_new("process-1").unwrap(),
                    origin: ProcessOrigin::Host,
                    kind: ProcessKind::Terminal,
                    owner: ProcessOwner::terminal("terminal-1"),
                    pid: 1,
                    label: "Terminal".into(),
                    started_at_ms: 1,
                    state: ProcessState::Running,
                    cpu_centipercent: 0,
                    memory_bytes: 0,
                    samples: Vec::new(),
                    nodes: Vec::new(),
                    terminable: true,
                }],
            })
        }

        fn terminate(&self, _: &WorkspaceId, _: &ProcessRootId) -> Result<(), ProcessError> {
            Err(ProcessError::new(ProcessErrorKind::NotOwned, "unused"))
        }
    }

    fn host() -> (ProcessSession, Arc<Mutex<Vec<Option<WorkspaceId>>>>) {
        let workspaces = Arc::new(Mutex::new(Vec::new()));
        let session = ProcessSession::new(
            Arc::new(Host {
                workspaces: workspaces.clone(),
            }),
            None,
        );
        (session, workspaces)
    }

    fn advance(cx: &mut gpui::TestAppContext, duration: Duration) {
        let deadline = std::time::Instant::now() + duration + Duration::from_secs(3);
        let mut elapsed = Duration::ZERO;
        let step = Duration::from_millis(20);
        while elapsed < duration {
            cx.executor().advance_clock(step);
            cx.run_until_parked();
            assert!(std::time::Instant::now() < deadline, "test timed out");
            elapsed += step;
            std::thread::yield_now();
        }
    }

    /// worker thread の応答を実時間で待ちながら、疑似時計を進める。
    fn wait_until(
        cx: &mut gpui::TestAppContext,
        mut done: impl FnMut(&mut gpui::TestAppContext) -> bool,
    ) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            cx.executor().advance_clock(Duration::from_millis(20));
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            assert!(std::time::Instant::now() < deadline, "test timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn root_count(feature: &ResourceManagerFeature) -> usize {
        feature.state.snapshot.roots.len()
    }

    #[test]
    fn refresh_and_terminate_use_the_workspace_held_by_the_worker() {
        let (session, workspaces) = host();
        let first = WorkspaceId::new("workspace-1");
        let second = WorkspaceId::new("workspace-2");
        let mut worker = ProcessWorker::new(session, Some(first.clone()));
        worker.refresh().unwrap();
        worker.set_workspace(Some(second.clone()));
        worker.refresh().unwrap();
        assert_eq!(*workspaces.lock().unwrap(), vec![Some(first), Some(second)]);
    }

    #[test]
    fn a_worker_without_a_workspace_does_not_request_host_values() {
        let (session, workspaces) = host();
        let mut worker = ProcessWorker::new(session, None);
        worker.refresh().unwrap();
        assert!(workspaces.lock().unwrap().is_empty());
    }

    #[gpui::test]
    fn a_workspace_switch_drops_the_previous_list_and_fetches_the_new_one(
        cx: &mut gpui::TestAppContext,
    ) {
        let (session, workspaces) = host();
        let first = WorkspaceId::new("workspace-1");
        let second = WorkspaceId::new("workspace-2");
        let feature = cx.update(|cx| {
            cx.new(|_| {
                ResourceManagerFeature::new(
                    ClientWorker::new(ProcessWorker::new(session, Some(first.clone()))),
                    Some(first.clone()),
                )
            })
        });
        feature.update(cx, |feature, cx| feature.set_visible(true, cx));
        wait_until(cx, |cx| cx.update(|cx| root_count(feature.read(cx))) == 1);

        // 切替直後に前の Workspace の一覧を残さない。
        feature.update(cx, |feature, cx| {
            feature.set_workspace(Some(second.clone()), cx);
            assert_eq!(feature.state.snapshot.roots.len(), 0);
        });
        wait_until(cx, |cx| cx.update(|cx| root_count(feature.read(cx))) == 1);
        let calls = workspaces.lock().unwrap().clone();
        // 最初の取得は切替前、以降は切替後の Workspace だけを使う。
        assert_eq!(calls.first(), Some(&Some(first)));
        assert!(calls[1..].iter().all(|call| call == &Some(second.clone())));
    }

    #[gpui::test]
    fn polling_runs_only_while_the_panel_is_visible(cx: &mut gpui::TestAppContext) {
        let (session, workspaces) = host();
        let workspace = WorkspaceId::new("workspace-1");
        let feature = cx.update(|cx| {
            cx.new(|_| {
                ResourceManagerFeature::new(
                    ClientWorker::new(ProcessWorker::new(session, Some(workspace.clone()))),
                    Some(workspace.clone()),
                )
            })
        });
        let calls = || workspaces.lock().unwrap().len();

        // 非表示のまま時間を進めても取得しない。
        advance(cx, Duration::from_secs(5));
        assert_eq!(calls(), 0);
        assert_eq!(cx.update(|cx| root_count(feature.read(cx))), 0);

        feature.update(cx, |feature, cx| feature.set_visible(true, cx));
        wait_until(cx, |cx| cx.update(|cx| root_count(feature.read(cx))) == 1);
        assert_eq!(calls(), 1);

        // 2 秒間隔の再取得が続く。
        advance(cx, Duration::from_secs(3));
        wait_until(cx, |_| calls() >= 2);

        // 非表示にした後は、次の周期以降に取得しない。
        feature.update(cx, |feature, cx| feature.set_visible(false, cx));
        advance(cx, Duration::from_secs(8));
        assert_eq!(calls(), 2);
    }
}
