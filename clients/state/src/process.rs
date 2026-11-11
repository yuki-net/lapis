//! Client 側で Host と Desktop のプロセス計測結果をまとめる。

use std::sync::Arc;

use editor_core::WorkspaceId;
use process::{
    ProcessConnection, ProcessError, ProcessErrorKind, ProcessOrigin, ProcessRegistry,
    ProcessRootId, ProcessSnapshot,
};

/// Host のレジストリと Desktop のローカル計測を同じ一覧として扱う。
///
/// Remote 接続でも Host 側の値は Host のレジストリから取得し、Desktop 自身の値は
/// このプロセス内で計測したものだけを使う。両者を混ぜて別の Host へ送らない。
pub struct ProcessSession {
    host: Arc<dyn ProcessConnection>,
    local: Option<Arc<dyn ProcessRegistry>>,
    snapshot: ProcessSnapshot,
    /// Host の取得に失敗した理由。直前の Host 一覧を表示している間だけ保持する。
    host_error: Option<String>,
    /// `snapshot` の Host root を取得できた Workspace。別 Workspace の stale を防ぐ。
    host_workspace: Option<WorkspaceId>,
}

impl Clone for ProcessSession {
    fn clone(&self) -> Self {
        Self {
            host: self.host.clone(),
            local: self.local.clone(),
            snapshot: self.snapshot.clone(),
            host_error: self.host_error.clone(),
            host_workspace: self.host_workspace.clone(),
        }
    }
}

impl ProcessSession {
    pub fn new(host: Arc<dyn ProcessConnection>, local: Option<Arc<dyn ProcessRegistry>>) -> Self {
        Self {
            host,
            local,
            snapshot: ProcessSnapshot::default(),
            host_error: None,
            host_workspace: None,
        }
    }

    /// Host へ接続しない target 用。Desktop のローカル計測も持たない。
    pub fn unavailable() -> Self {
        Self::new(Arc::new(UnavailableConnection), None)
    }

    pub fn snapshot(&self) -> &ProcessSnapshot {
        &self.snapshot
    }

    /// Host の一覧が取得できず、直前の値を表示している場合の理由。
    pub fn host_error(&self) -> Option<&str> {
        self.host_error.as_deref()
    }

    /// Host と Desktop の現在値を取得する。失敗時は以前の一覧を保持する。
    pub fn refresh(&mut self, workspace: Option<&WorkspaceId>) -> Result<bool, ProcessError> {
        let mut host_error = None;
        let host = match workspace {
            Some(workspace) => match self.host.snapshot(workspace) {
                Ok(snapshot) => {
                    self.host_workspace = Some(workspace.clone());
                    Some(snapshot)
                }
                // 接続断をプロセス終了と誤認しない。同じ Workspace の直前値だけを stale として保持する。
                Err(error) if self.local.is_some() => {
                    host_error = Some(error.to_string());
                    let same_workspace = self.host_workspace.as_ref() == Some(workspace);
                    Some(ProcessSnapshot {
                        sampled_at_ms: self.snapshot.sampled_at_ms,
                        roots: if same_workspace {
                            self.snapshot
                                .roots_for(ProcessOrigin::Host)
                                .cloned()
                                .collect()
                        } else {
                            Vec::new()
                        },
                    })
                }
                Err(error) => return Err(error),
            },
            None => None,
        };
        let local = self.local.as_ref().map(|local| local.snapshot(None));
        let mut roots = Vec::new();
        let mut sampled_at_ms = 0;
        if let Some(host) = host {
            sampled_at_ms = sampled_at_ms.max(host.sampled_at_ms);
            roots.extend(host.roots);
        }
        if let Some(local) = local {
            sampled_at_ms = sampled_at_ms.max(local.sampled_at_ms);
            roots.extend(local.roots);
        }
        roots.sort_by(|left, right| {
            origin_order(left.origin)
                .cmp(&origin_order(right.origin))
                .then_with(|| kind_order(left.kind).cmp(&kind_order(right.kind)))
                .then_with(|| left.pid.cmp(&right.pid))
        });
        let snapshot = ProcessSnapshot {
            sampled_at_ms,
            roots,
        };
        let changed = snapshot != self.snapshot;
        self.snapshot = snapshot;
        self.host_error = host_error;
        Ok(changed)
    }

    /// 所有元のレジストリにだけ終了を依頼する。
    pub fn terminate(
        &mut self,
        workspace: Option<&WorkspaceId>,
        origin: ProcessOrigin,
        root_id: &ProcessRootId,
    ) -> Result<(), ProcessError> {
        match origin {
            ProcessOrigin::Host => {
                let workspace = workspace.ok_or_else(|| {
                    ProcessError::new(
                        ProcessErrorKind::NotOwned,
                        "Host process is not available without a workspace",
                    )
                })?;
                self.host.terminate(workspace, root_id)
            }
            ProcessOrigin::Desktop => self
                .local
                .as_ref()
                .ok_or_else(|| {
                    ProcessError::new(
                        ProcessErrorKind::NotOwned,
                        "Desktop process registry is unavailable",
                    )
                })?
                .terminate(None, root_id),
        }
    }
}

/// 一覧の並びは wire 契約ではなく表示上の都合。Host の自身 → Workspace の root、
/// Desktop の自身 → ローカル root の順に固定する。
fn origin_order(origin: ProcessOrigin) -> u8 {
    match origin {
        ProcessOrigin::Host => 0,
        ProcessOrigin::Desktop => 1,
    }
}

fn kind_order(kind: process::ProcessKind) -> u8 {
    use process::ProcessKind::*;
    match kind {
        Host => 0,
        Desktop => 1,
        Terminal => 2,
        TaskRun => 3,
        Agent => 4,
        LanguageServer => 5,
        Build => 6,
        Git => 7,
        Other => 8,
    }
}

struct UnavailableConnection;

impl ProcessConnection for UnavailableConnection {
    fn snapshot(&self, _: &WorkspaceId) -> Result<ProcessSnapshot, ProcessError> {
        Err(ProcessError::new(
            ProcessErrorKind::NotOwned,
            "process registry is unavailable for this connection",
        ))
    }

    fn terminate(&self, _: &WorkspaceId, _: &ProcessRootId) -> Result<(), ProcessError> {
        Err(ProcessError::new(
            ProcessErrorKind::NotOwned,
            "process registry is unavailable for this connection",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use process::{
        ProcessKind, ProcessOwner, ProcessRegistry, ProcessRoot, ProcessRootSpec, ProcessState,
    };

    struct HostFixture {
        snapshot: ProcessSnapshot,
        terminated: std::sync::Mutex<Vec<ProcessRootId>>,
        fail: std::sync::atomic::AtomicBool,
    }

    impl ProcessConnection for HostFixture {
        fn snapshot(&self, _: &WorkspaceId) -> Result<ProcessSnapshot, ProcessError> {
            if self.fail.load(std::sync::atomic::Ordering::Acquire) {
                return Err(ProcessError::new(ProcessErrorKind::Io, "host unavailable"));
            }
            Ok(self.snapshot.clone())
        }

        fn terminate(&self, _: &WorkspaceId, root_id: &ProcessRootId) -> Result<(), ProcessError> {
            self.terminated.lock().unwrap().push(root_id.clone());
            Ok(())
        }
    }

    struct LocalFixture {
        snapshot: ProcessSnapshot,
        terminated: std::sync::Mutex<Vec<ProcessRootId>>,
    }

    impl ProcessRegistry for LocalFixture {
        fn origin(&self) -> ProcessOrigin {
            ProcessOrigin::Desktop
        }

        fn register(&self, _: ProcessRootSpec) -> Result<ProcessRootId, ProcessError> {
            Err(ProcessError::new(ProcessErrorKind::Io, "unused"))
        }

        fn unregister(&self, _: &ProcessRootId) {}

        fn snapshot(&self, _: Option<&WorkspaceId>) -> ProcessSnapshot {
            self.snapshot.clone()
        }

        fn terminate(
            &self,
            _: Option<&WorkspaceId>,
            root_id: &ProcessRootId,
        ) -> Result<(), ProcessError> {
            self.terminated.lock().unwrap().push(root_id.clone());
            Ok(())
        }
    }

    fn root(origin: ProcessOrigin, pid: u32, kind: ProcessKind) -> ProcessRoot {
        ProcessRoot {
            root_id: ProcessRootId::try_new(format!("process-{pid}")).unwrap(),
            origin,
            kind,
            owner: ProcessOwner::new(kind, None),
            pid,
            label: "test".into(),
            started_at_ms: 1,
            state: ProcessState::Running,
            cpu_centipercent: 0,
            memory_bytes: 0,
            samples: Vec::new(),
            nodes: Vec::new(),
            terminable: true,
        }
    }

    #[test]
    fn host_and_desktop_values_are_merged_without_crossing_origins() {
        let workspace = WorkspaceId::new("workspace-1");
        let host = Arc::new(HostFixture {
            snapshot: ProcessSnapshot {
                sampled_at_ms: 10,
                roots: vec![root(ProcessOrigin::Host, 1, ProcessKind::Terminal)],
            },
            terminated: Default::default(),
            fail: Default::default(),
        });
        let local = Arc::new(LocalFixture {
            snapshot: ProcessSnapshot {
                sampled_at_ms: 20,
                roots: vec![root(ProcessOrigin::Desktop, 2, ProcessKind::LanguageServer)],
            },
            terminated: Default::default(),
        });
        let mut session = ProcessSession::new(host, Some(local));
        assert!(session.refresh(Some(&workspace)).unwrap());
        let snapshot = session.snapshot();
        assert_eq!(snapshot.sampled_at_ms, 20);
        assert_eq!(snapshot.roots.len(), 2);
        assert_eq!(snapshot.roots[0].origin, ProcessOrigin::Host);
        assert_eq!(snapshot.roots[1].origin, ProcessOrigin::Desktop);
        assert!(!session.refresh(Some(&workspace)).unwrap());
    }

    #[test]
    fn a_host_failure_keeps_the_previous_host_roots_and_reports_staleness() {
        let workspace = WorkspaceId::new("workspace-1");
        let host = Arc::new(HostFixture {
            snapshot: ProcessSnapshot {
                sampled_at_ms: 10,
                roots: vec![root(ProcessOrigin::Host, 1, ProcessKind::Terminal)],
            },
            terminated: Default::default(),
            fail: Default::default(),
        });
        let local = Arc::new(LocalFixture {
            snapshot: ProcessSnapshot {
                sampled_at_ms: 20,
                roots: vec![root(ProcessOrigin::Desktop, 2, ProcessKind::Desktop)],
            },
            terminated: Default::default(),
        });
        let mut session = ProcessSession::new(host.clone(), Some(local));
        session.refresh(Some(&workspace)).unwrap();
        assert!(session.host_error().is_none());

        host.fail.store(true, std::sync::atomic::Ordering::Release);
        session.refresh(Some(&workspace)).unwrap();
        let snapshot = session.snapshot();
        assert_eq!(snapshot.roots.len(), 2);
        assert!(
            snapshot
                .roots
                .iter()
                .any(|root| root.origin == ProcessOrigin::Host && root.pid == 1)
        );
        assert_eq!(session.host_error(), Some("host unavailable"));
    }

    #[test]
    fn a_workspace_switch_drops_stale_host_roots() {
        let first = WorkspaceId::new("workspace-1");
        let second = WorkspaceId::new("workspace-2");
        let host = Arc::new(HostFixture {
            snapshot: ProcessSnapshot {
                sampled_at_ms: 10,
                roots: vec![root(ProcessOrigin::Host, 1, ProcessKind::Terminal)],
            },
            terminated: Default::default(),
            fail: Default::default(),
        });
        let local = Arc::new(LocalFixture {
            snapshot: ProcessSnapshot {
                sampled_at_ms: 20,
                roots: vec![root(ProcessOrigin::Desktop, 2, ProcessKind::Desktop)],
            },
            terminated: Default::default(),
        });
        let mut session = ProcessSession::new(host.clone(), Some(local));
        session.refresh(Some(&first)).unwrap();
        assert!(
            session
                .snapshot()
                .roots
                .iter()
                .any(|root| root.origin == ProcessOrigin::Host)
        );

        host.fail.store(true, std::sync::atomic::Ordering::Release);
        session.refresh(Some(&second)).unwrap();
        assert!(
            session
                .snapshot()
                .roots
                .iter()
                .all(|root| root.origin != ProcessOrigin::Host)
        );
        assert_eq!(session.host_error(), Some("host unavailable"));
    }

    #[test]
    fn desktop_values_survive_a_host_snapshot_failure() {
        struct FailingHost;
        impl ProcessConnection for FailingHost {
            fn snapshot(&self, _: &WorkspaceId) -> Result<ProcessSnapshot, ProcessError> {
                Err(ProcessError::new(ProcessErrorKind::Io, "host unavailable"))
            }
            fn terminate(&self, _: &WorkspaceId, _: &ProcessRootId) -> Result<(), ProcessError> {
                Err(ProcessError::new(ProcessErrorKind::Io, "host unavailable"))
            }
        }
        let workspace = WorkspaceId::new("workspace-1");
        let local = Arc::new(LocalFixture {
            snapshot: ProcessSnapshot {
                sampled_at_ms: 20,
                roots: vec![root(ProcessOrigin::Desktop, 2, ProcessKind::Desktop)],
            },
            terminated: Default::default(),
        });
        let mut session = ProcessSession::new(Arc::new(FailingHost), Some(local));
        assert!(session.refresh(Some(&workspace)).unwrap());
        assert_eq!(session.snapshot().roots.len(), 1);
        assert_eq!(session.snapshot().roots[0].origin, ProcessOrigin::Desktop);
        assert_eq!(session.host_error(), Some("host unavailable"));
    }

    #[test]
    fn termination_is_sent_to_the_origin_that_owns_the_root() {
        let workspace = WorkspaceId::new("workspace-1");
        let host = Arc::new(HostFixture {
            snapshot: ProcessSnapshot::default(),
            terminated: Default::default(),
            fail: Default::default(),
        });
        let local = Arc::new(LocalFixture {
            snapshot: ProcessSnapshot::default(),
            terminated: Default::default(),
        });
        let mut session = ProcessSession::new(host.clone(), Some(local.clone()));
        let host_id = ProcessRootId::try_new("process-host").unwrap();
        let desktop_id = ProcessRootId::try_new("process-desktop").unwrap();
        session
            .terminate(Some(&workspace), ProcessOrigin::Host, &host_id)
            .unwrap();
        session
            .terminate(Some(&workspace), ProcessOrigin::Desktop, &desktop_id)
            .unwrap();
        assert_eq!(*host.terminated.lock().unwrap(), vec![host_id]);
        assert_eq!(*local.terminated.lock().unwrap(), vec![desktop_id]);
    }
}
