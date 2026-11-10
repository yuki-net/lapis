//! Lapis が spawn したプロセスのローカル・レジストリ。
//!
//! OS 全体のプロセス一覧は公開しない。登録された root PID と、その PID から辿れる
//! 子孫だけをサンプリング対象にし、終了操作も同じ root 単位で受ける。

use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use editor_core::WorkspaceId;
use process::{
    ProcessError, ProcessErrorKind, ProcessNode, ProcessOrigin, ProcessOwner, ProcessRegistry,
    ProcessRoot, ProcessRootId, ProcessRootSpec, ProcessSample, ProcessSnapshot, ProcessState,
};
use sysinfo::{Pid, ProcessesToUpdate, System};

/// 既定の計測間隔。Resource Manager の表示更新より十分細かく、OS 負荷を抑える。
pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
/// 1 root あたりの保持サンプル数（既定間隔で 60 秒分）。
pub const SAMPLE_RETENTION: usize = 60;
/// 終了した root を一覧に残す時間。状態を確認できる猶予として 60 秒。
pub const EXITED_RETENTION: Duration = Duration::from_secs(60);

/// 子孫探索の暴走を防ぐ上限。通常の Terminal / Task ツリーでは到達しない。
const MAX_TREE_NODES: usize = 512;

pub struct LocalProcessRegistry {
    inner: Arc<Mutex<RegistryState>>,
    stop: Mutex<Option<mpsc::Sender<()>>>,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
}

struct RegistryState {
    origin: ProcessOrigin,
    roots: BTreeMap<ProcessRootId, RootState>,
    next_root: AtomicU64,
    sample_limit: usize,
    exited_retention: Duration,
}

struct RootState {
    spec: ProcessRootSpec,
    started_at_ms: u64,
    /// 登録時に確定した OS 起動時刻（秒）。確定できない root は登録しない。
    started_at_secs: u64,
    state: ProcessState,
    exited_at: Option<Instant>,
    cpu_centipercent: u32,
    memory_bytes: u64,
    samples: VecDeque<ProcessSample>,
    nodes: Vec<ProcessNode>,
}

impl RootState {
    fn public(&self, origin: ProcessOrigin, root_id: &ProcessRootId) -> ProcessRoot {
        ProcessRoot {
            root_id: root_id.clone(),
            origin,
            kind: self.spec.kind,
            owner: self.spec.owner.clone(),
            pid: self.spec.pid,
            label: self.spec.label.clone(),
            started_at_ms: self.started_at_ms,
            state: self.state,
            cpu_centipercent: self.cpu_centipercent,
            memory_bytes: self.memory_bytes,
            samples: self.samples.iter().cloned().collect(),
            nodes: self.nodes.clone(),
            terminable: !self.spec.kind.is_self(),
        }
    }
}

impl LocalProcessRegistry {
    pub fn new(origin: ProcessOrigin) -> Arc<Self> {
        Self::with_settings(origin, SAMPLE_INTERVAL, SAMPLE_RETENTION, EXITED_RETENTION)
    }

    /// テストや短寿命の Host で計測間隔を変える。
    pub fn with_interval(origin: ProcessOrigin, interval: Duration) -> Arc<Self> {
        Self::with_settings(origin, interval, SAMPLE_RETENTION, EXITED_RETENTION)
    }

    pub fn with_settings(
        origin: ProcessOrigin,
        interval: Duration,
        sample_limit: usize,
        exited_retention: Duration,
    ) -> Arc<Self> {
        let interval = interval.max(Duration::from_millis(10));
        let state = Arc::new(Mutex::new(RegistryState {
            origin,
            roots: BTreeMap::new(),
            next_root: AtomicU64::new(1),
            sample_limit: sample_limit.max(1),
            exited_retention,
        }));
        let registry = Arc::new(Self {
            inner: state,
            stop: Mutex::new(None),
            worker: Mutex::new(None),
        });
        registry
            .register(ProcessRootSpec {
                workspace: None,
                kind: match origin {
                    ProcessOrigin::Host => process::ProcessKind::Host,
                    ProcessOrigin::Desktop => process::ProcessKind::Desktop,
                },
                owner: ProcessOwner::new(
                    match origin {
                        ProcessOrigin::Host => process::ProcessKind::Host,
                        ProcessOrigin::Desktop => process::ProcessKind::Desktop,
                    },
                    None,
                ),
                pid: std::process::id(),
                label: match origin {
                    ProcessOrigin::Host => "Host".to_owned(),
                    ProcessOrigin::Desktop => "Desktop".to_owned(),
                },
            })
            .expect("self process must be registrable");

        let (stop, receiver) = mpsc::channel();
        let sampler_state = registry.inner.clone();
        let worker = thread::Builder::new()
            .name("lapis-process-sampler".to_owned())
            .spawn(move || {
                let mut system = System::new();
                while let Err(mpsc::RecvTimeoutError::Timeout) = receiver.recv_timeout(interval) {
                    sample(&sampler_state, &mut system);
                }
            })
            .ok();
        if let Ok(mut slot) = registry.stop.lock() {
            *slot = Some(stop);
        }
        if let Ok(mut slot) = registry.worker.lock() {
            *slot = worker;
        }
        registry
    }
}

impl Drop for LocalProcessRegistry {
    fn drop(&mut self) {
        if let Ok(stop) = self.stop.get_mut()
            && let Some(stop) = stop.take()
        {
            let _ = stop.send(());
        }
        if let Ok(worker) = self.worker.get_mut()
            && let Some(worker) = worker.take()
        {
            let _ = worker.join();
        }
    }
}

impl ProcessRegistry for LocalProcessRegistry {
    fn origin(&self) -> ProcessOrigin {
        self.inner
            .lock()
            .map(|state| state.origin)
            .unwrap_or(ProcessOrigin::Host)
    }

    fn register(&self, spec: ProcessRootSpec) -> Result<ProcessRootId, ProcessError> {
        if spec.pid == 0 {
            return Err(ProcessError::new(
                ProcessErrorKind::Io,
                "process pid must be greater than zero",
            ));
        }
        // 最初のサンプル前でも PID 再利用を判定できるよう、登録時に開始時刻を確定する。
        // 確定できない root は所有対象にしない（別プロセスを誤って管理しないため）。
        let started_at_secs = process_start_time(spec.pid).ok_or_else(|| {
            ProcessError::new(
                ProcessErrorKind::Io,
                "process identity could not be verified",
            )
        })?;
        let mut state = self
            .inner
            .lock()
            .map_err(|_| ProcessError::new(ProcessErrorKind::Io, "process registry unavailable"))?;
        let sequence = state.next_root.fetch_add(1, Ordering::Relaxed);
        let root_id = ProcessRootId::try_new(format!(
            "process-{}-{}-{sequence}",
            std::process::id(),
            spec.pid
        ))
        .map_err(|_| ProcessError::new(ProcessErrorKind::Io, "process root id overflow"))?;
        state.roots.insert(
            root_id.clone(),
            RootState {
                spec,
                started_at_ms: started_at_secs.saturating_mul(1_000),
                started_at_secs,
                state: ProcessState::Running,
                exited_at: None,
                cpu_centipercent: 0,
                memory_bytes: 0,
                samples: VecDeque::new(),
                nodes: Vec::new(),
            },
        );
        Ok(root_id)
    }

    fn unregister(&self, root_id: &ProcessRootId) {
        if let Ok(mut state) = self.inner.lock() {
            state.roots.remove(root_id);
        }
    }

    fn snapshot(&self, workspace: Option<&WorkspaceId>) -> ProcessSnapshot {
        let Ok(state) = self.inner.lock() else {
            return ProcessSnapshot::default();
        };
        let roots = state
            .roots
            .iter()
            .filter(|(_, root)| {
                root.spec.workspace.is_none()
                    || workspace
                        .is_some_and(|workspace| root.spec.workspace.as_ref() == Some(workspace))
            })
            .map(|(root_id, root)| root.public(state.origin, root_id))
            .collect();
        ProcessSnapshot {
            sampled_at_ms: now_ms(),
            roots,
        }
    }

    fn terminate(
        &self,
        workspace: Option<&WorkspaceId>,
        root_id: &ProcessRootId,
    ) -> Result<(), ProcessError> {
        let (pid, started_at_secs) = {
            let state = self.inner.lock().map_err(|_| {
                ProcessError::new(ProcessErrorKind::Io, "process registry unavailable")
            })?;
            let root = state.roots.get(root_id).ok_or_else(|| {
                ProcessError::new(ProcessErrorKind::NotFound, "process root was not found")
            })?;
            if root.spec.kind.is_self() {
                return Err(ProcessError::new(
                    ProcessErrorKind::NotOwned,
                    "Host and Desktop processes cannot be terminated here",
                ));
            }
            if root.spec.workspace.as_ref() != workspace {
                return Err(ProcessError::new(
                    ProcessErrorKind::NotOwned,
                    "process root belongs to another workspace",
                ));
            }
            if root.state == ProcessState::Exited {
                return Ok(());
            }
            (root.spec.pid, root.started_at_secs)
        };
        terminate_tree(pid, started_at_secs)?;
        if let Ok(mut state) = self.inner.lock()
            && let Some(root) = state.roots.get_mut(root_id)
        {
            root.state = ProcessState::Exited;
            root.cpu_centipercent = 0;
            root.memory_bytes = 0;
            root.exited_at.get_or_insert_with(Instant::now);
        }
        Ok(())
    }
}

fn sample(state: &Mutex<RegistryState>, system: &mut System) {
    system.refresh_processes(ProcessesToUpdate::All);
    let Ok(mut state) = state.lock() else {
        return;
    };
    let now = now_ms();
    let sample_limit = state.sample_limit;
    let children = children_by_parent(system);
    let mut remove = Vec::new();
    for root in state.roots.values_mut() {
        let Some(process) = system.process(Pid::from_u32(root.spec.pid)) else {
            mark_exited(root);
            continue;
        };
        let started_at_secs = process.start_time();
        if root.started_at_secs != started_at_secs {
            // PID が再利用された。別プロセスを所有ツリーとして扱わない。
            mark_exited(root);
            continue;
        }
        let (nodes, cpu_centipercent, memory_bytes) =
            collect_tree(system, &children, root.spec.pid);
        if nodes.is_empty() {
            mark_exited(root);
            continue;
        }
        root.state = ProcessState::Running;
        root.exited_at = None;
        root.cpu_centipercent = cpu_centipercent;
        root.memory_bytes = memory_bytes;
        root.nodes = nodes;
        root.samples.push_back(ProcessSample {
            at_ms: now,
            cpu_centipercent,
            memory_bytes,
        });
        while root.samples.len() > sample_limit {
            root.samples.pop_front();
        }
    }
    for (root_id, root) in &state.roots {
        if root.state == ProcessState::Exited
            && root
                .exited_at
                .is_some_and(|at| at.elapsed() >= state.exited_retention)
            && !root.spec.kind.is_self()
        {
            remove.push(root_id.clone());
        }
    }
    for root_id in remove {
        state.roots.remove(&root_id);
    }
}

fn mark_exited(root: &mut RootState) {
    root.state = ProcessState::Exited;
    root.exited_at.get_or_insert_with(Instant::now);
    root.cpu_centipercent = 0;
    root.memory_bytes = 0;
}

fn children_by_parent(system: &System) -> HashMap<u32, Vec<u32>> {
    let mut children = HashMap::<u32, Vec<u32>>::new();
    for process in system.processes().values() {
        if let Some(parent) = process.parent() {
            children
                .entry(parent.as_u32())
                .or_default()
                .push(process.pid().as_u32());
        }
    }
    children
}

fn collect_tree(
    system: &System,
    children: &HashMap<u32, Vec<u32>>,
    root_pid: u32,
) -> (Vec<ProcessNode>, u32, u64) {
    let mut nodes = Vec::new();
    let mut seen = HashSet::new();
    let mut pending = VecDeque::from([(root_pid, 0_u16, None)]);
    let mut cpu_centipercent = 0_u32;
    let mut memory: u64 = 0;
    while let Some((pid, depth, parent_pid)) = pending.pop_front() {
        if nodes.len() >= MAX_TREE_NODES || !seen.insert(pid) {
            continue;
        }
        let Some(process) = system.process(Pid::from_u32(pid)) else {
            continue;
        };
        let process_cpu = (process.cpu_usage() * 100.0).round() as u32;
        let process_memory = process.memory();
        cpu_centipercent = cpu_centipercent.saturating_add(process_cpu);
        memory = memory.saturating_add(process_memory);
        nodes.push(ProcessNode {
            pid,
            parent_pid,
            name: process.name().to_string_lossy().into_owned(),
            depth,
            cpu_centipercent: process_cpu,
            memory_bytes: process_memory,
            started_at_ms: process.start_time().saturating_mul(1_000),
        });
        if let Some(children) = children.get(&pid) {
            for child in children {
                pending.push_back((*child, depth.saturating_add(1), Some(pid)));
            }
        }
    }
    nodes.sort_by_key(|node| (node.depth, node.pid));
    (nodes, cpu_centipercent, memory)
}

fn terminate_tree(pid: u32, started_at_secs: u64) -> Result<(), ProcessError> {
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::All);
    let Some(root) = system.process(Pid::from_u32(pid)) else {
        return Ok(());
    };
    // 所有した時点の開始時刻と一致しない PID は再利用された別プロセス。
    if root.start_time() != started_at_secs {
        return Ok(());
    }
    // Windows は git_action と同じ taskkill /T で子孫を含めて止める。
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let status = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if status.is_ok_and(|status| status.success()) {
            return Ok(());
        }
    }
    // 子孫を先に止める。列挙は root の現在の親子関係だけを対象にする。
    let (nodes, _, _) = collect_tree(&system, &children_by_parent(&system), pid);
    let mut failed = false;
    for node in nodes.iter().rev().filter(|node| node.pid != pid) {
        if let Some(process) = system.process(Pid::from_u32(node.pid))
            && !process.kill()
            && system.process(Pid::from_u32(node.pid)).is_some()
        {
            failed = true;
        }
    }
    if let Some(root) = system.process(Pid::from_u32(pid))
        && !root.kill()
    {
        failed = true;
    }
    if failed {
        return Err(ProcessError::new(
            ProcessErrorKind::Io,
            "failed to terminate a process in the owned tree",
        ));
    }
    Ok(())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn process_start_time(pid: u32) -> Option<u64> {
    let pid = Pid::from_u32(pid);
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]));
    system
        .process(pid)
        .map(|process| process.start_time())
        .filter(|started| *started > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use process::ProcessKind;
    use std::{process::Command, time::Instant};

    fn spawn_sleeper() -> std::process::Child {
        #[cfg(windows)]
        {
            Command::new("powershell.exe")
                .args(["-NoProfile", "-Command", "Start-Sleep -Seconds 30"])
                .spawn()
                .expect("spawn powershell sleeper")
        }
        #[cfg(not(windows))]
        {
            Command::new("sh")
                .args(["-c", "sleep 30"])
                .spawn()
                .expect("spawn sh sleeper")
        }
    }

    fn spawn_tree() -> std::process::Child {
        #[cfg(windows)]
        {
            Command::new("powershell.exe")
                .args([
                    "-NoProfile",
                    "-Command",
                    "$p = Start-Process -FilePath powershell.exe -ArgumentList '-NoProfile','-Command','Start-Sleep -Seconds 30' -PassThru; Start-Sleep -Seconds 30",
                ])
                .spawn()
                .expect("spawn powershell process tree")
        }
        #[cfg(not(windows))]
        {
            Command::new("sh")
                .args(["-c", "sleep 30 & sleep 30"])
                .spawn()
                .expect("spawn sh process tree")
        }
    }

    fn process_alive(pid: u32) -> bool {
        let mut system = System::new();
        system.refresh_processes(ProcessesToUpdate::All);
        system
            .process(Pid::from_u32(pid))
            // 親が wait するまでの zombie は実行中ではない。
            .is_some_and(|process| process.status() != sysinfo::ProcessStatus::Zombie)
    }

    fn wait_until(mut condition: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if condition() {
                return true;
            }
            thread::sleep(Duration::from_millis(25));
        }
        false
    }

    fn root(registry: &LocalProcessRegistry, workspace: &str, pid: u32) -> ProcessRootId {
        registry
            .register(ProcessRootSpec {
                workspace: Some(WorkspaceId::new(workspace)),
                kind: ProcessKind::Terminal,
                owner: ProcessOwner::terminal("terminal-test"),
                pid,
                label: "test".into(),
            })
            .unwrap()
    }

    #[test]
    fn snapshot_contains_only_registered_roots_and_their_children() {
        let registry =
            LocalProcessRegistry::with_interval(ProcessOrigin::Host, Duration::from_millis(25));
        let workspace = WorkspaceId::new("workspace-a");
        let mut child = spawn_sleeper();
        let root_id = root(&registry, "workspace-a", child.id());

        assert!(wait_until(|| {
            let snapshot = registry.snapshot(Some(&workspace));
            snapshot.roots.iter().any(|root| {
                root.root_id == root_id
                    && root.state == ProcessState::Running
                    && !root.samples.is_empty()
            })
        }));
        let snapshot = registry.snapshot(Some(&workspace));
        assert!(
            snapshot
                .roots
                .iter()
                .any(|root| root.kind == ProcessKind::Host && !root.terminable)
        );
        let owned = snapshot
            .roots
            .iter()
            .find(|root| root.root_id == root_id)
            .expect("registered root is listed");
        assert_eq!(owned.origin, ProcessOrigin::Host);
        assert_eq!(owned.owner, ProcessOwner::terminal("terminal-test"));
        assert!(owned.terminable);
        assert!(!owned.nodes.is_empty());

        registry.terminate(Some(&workspace), &root_id).unwrap();
        assert!(wait_until(|| child.try_wait().unwrap().is_some()));
    }

    #[test]
    fn termination_includes_descendants() {
        let registry =
            LocalProcessRegistry::with_interval(ProcessOrigin::Host, Duration::from_millis(25));
        let workspace = WorkspaceId::new("workspace-a");
        let mut child = spawn_tree();
        let root_id = root(&registry, "workspace-a", child.id());
        assert!(wait_until(|| {
            registry
                .snapshot(Some(&workspace))
                .roots
                .iter()
                .find(|root| root.root_id == root_id)
                .is_some_and(|root| root.nodes.len() >= 2)
        }));
        let pids = registry
            .snapshot(Some(&workspace))
            .roots
            .iter()
            .find(|root| root.root_id == root_id)
            .expect("registered root is listed")
            .nodes
            .iter()
            .map(|node| node.pid)
            .collect::<Vec<_>>();
        registry.terminate(Some(&workspace), &root_id).unwrap();
        assert!(wait_until(|| child.try_wait().unwrap().is_some()));
        assert!(wait_until(|| pids.iter().all(|pid| !process_alive(*pid))));
    }

    #[test]
    fn termination_refuses_a_reused_pid_identity() {
        let pid = std::process::id();
        let started = process_start_time(pid).expect("current process start time");
        terminate_tree(pid, started.saturating_add(1)).unwrap();
        assert!(process_alive(pid));
    }

    #[test]
    fn registration_rejects_a_process_whose_identity_is_unknown() {
        let registry =
            LocalProcessRegistry::with_interval(ProcessOrigin::Host, Duration::from_millis(25));
        let error = registry
            .register(ProcessRootSpec {
                workspace: Some(WorkspaceId::new("workspace-a")),
                kind: ProcessKind::Terminal,
                owner: ProcessOwner::terminal("terminal-test"),
                pid: u32::MAX,
                label: "unknown".into(),
            })
            .unwrap_err();
        assert_eq!(error.kind(), ProcessErrorKind::Io);
        assert!(
            registry
                .snapshot(None)
                .roots
                .iter()
                .all(|root| root.pid != u32::MAX)
        );
    }

    #[test]
    fn another_workspace_cannot_terminate_or_see_the_root() {
        let registry =
            LocalProcessRegistry::with_interval(ProcessOrigin::Host, Duration::from_millis(25));
        let mut child = spawn_sleeper();
        let root_id = root(&registry, "workspace-b", child.id());

        assert!(
            registry
                .snapshot(Some(&WorkspaceId::new("workspace-a")))
                .roots
                .iter()
                .all(|root| root.root_id != root_id)
        );
        let error = registry
            .terminate(Some(&WorkspaceId::new("workspace-a")), &root_id)
            .unwrap_err();
        assert_eq!(error.kind(), ProcessErrorKind::NotOwned);
        assert!(child.try_wait().unwrap().is_none());
        child.kill().unwrap();
        let _ = child.wait();
    }

    #[test]
    fn host_and_desktop_roots_are_not_terminable() {
        let registry = LocalProcessRegistry::new(ProcessOrigin::Desktop);
        let snapshot = registry.snapshot(None);
        let own = snapshot
            .roots
            .iter()
            .find(|root| root.kind == ProcessKind::Desktop)
            .unwrap();
        assert!(!own.terminable);
        let error = registry.terminate(None, &own.root_id).unwrap_err();
        assert_eq!(error.kind(), ProcessErrorKind::NotOwned);
    }

    #[test]
    fn exited_roots_are_kept_for_a_short_while_then_removed() {
        let registry = LocalProcessRegistry::with_settings(
            ProcessOrigin::Host,
            Duration::from_millis(25),
            SAMPLE_RETENTION,
            Duration::from_millis(100),
        );
        let workspace = WorkspaceId::new("workspace-a");
        let mut child = spawn_sleeper();
        let pid = child.id();
        let root_id = root(&registry, "workspace-a", pid);
        assert!(wait_until(|| registry
            .snapshot(Some(&workspace))
            .roots
            .iter()
            .any(
                |root| root.root_id == root_id && root.state == ProcessState::Running
            )));
        child.kill().unwrap();
        let _ = child.wait();
        assert!(wait_until(|| registry
            .snapshot(Some(&workspace))
            .roots
            .iter()
            .any(
                |root| root.root_id == root_id && root.state == ProcessState::Exited
            )));
        assert!(wait_until(|| registry
            .snapshot(Some(&workspace))
            .roots
            .iter()
            .all(|root| root.root_id != root_id)));
    }
}
