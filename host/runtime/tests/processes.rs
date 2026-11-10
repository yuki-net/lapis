use backend_state::{
    BackendService, BackendState, EmbeddedClient, WorkspaceEntry, WorkspaceFileBackend,
    WorkspaceRegistration,
};
use client_api::{
    RequestBody, ResponseBody, SessionId, WorkspaceId, WorkspaceOperation as Operation,
    WorkspaceOperationRequest, WorkspaceOperationResponse as Outcome,
};
use document::{DocumentError, DocumentRepository, FileData, FileFingerprint};
use editor_core::WorkspaceId as CoreWorkspaceId;
use process::{
    ProcessError, ProcessErrorKind, ProcessKind, ProcessOwner, ProcessRegistry, ProcessRoot,
    ProcessRootId, ProcessRootSpec, ProcessSnapshot, ProcessState,
};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

struct EmptyFiles;

impl DocumentRepository for EmptyFiles {
    fn read_file(&self, _: &Path) -> Result<FileData, DocumentError> {
        Err(DocumentError::io("unused"))
    }
    fn write_file(
        &self,
        _: &Path,
        _: &[u8],
        _: Option<&FileFingerprint>,
    ) -> Result<FileFingerprint, DocumentError> {
        Err(DocumentError::io("unused"))
    }
    fn fingerprint(&self, _: &Path) -> Result<Option<FileFingerprint>, DocumentError> {
        Ok(None)
    }
}

impl WorkspaceFileBackend for EmptyFiles {
    fn create_directory(&self, _: &Path) -> Result<(), DocumentError> {
        Err(DocumentError::io("unsupported"))
    }
    fn list_children(&self, _: &Path) -> Result<Vec<WorkspaceEntry>, DocumentError> {
        Ok(Vec::new())
    }
}

#[derive(Default)]
struct FakeProcesses {
    roots: Mutex<Vec<(CoreWorkspaceId, ProcessRoot)>>,
    terminated: Mutex<Vec<ProcessRootId>>,
}

fn root(workspace: &str) -> (CoreWorkspaceId, ProcessRoot) {
    let workspace = CoreWorkspaceId::new(workspace);
    let id = ProcessRootId::try_new("process-1").unwrap();
    (
        workspace.clone(),
        ProcessRoot {
            root_id: id,
            origin: process::ProcessOrigin::Host,
            kind: ProcessKind::Terminal,
            owner: ProcessOwner::terminal("terminal-1"),
            pid: 42,
            label: "Terminal".into(),
            started_at_ms: 1,
            state: ProcessState::Running,
            cpu_centipercent: 50,
            memory_bytes: 1024,
            samples: Vec::new(),
            nodes: Vec::new(),
            terminable: true,
        },
    )
}

impl ProcessRegistry for FakeProcesses {
    fn origin(&self) -> process::ProcessOrigin {
        process::ProcessOrigin::Host
    }

    fn register(&self, spec: ProcessRootSpec) -> Result<ProcessRootId, ProcessError> {
        let id = ProcessRootId::try_new(format!("process-{}", spec.pid)).unwrap();
        self.roots.lock().unwrap().push((
            spec.workspace.expect("workspace root"),
            ProcessRoot {
                root_id: id.clone(),
                origin: process::ProcessOrigin::Host,
                kind: spec.kind,
                owner: spec.owner,
                pid: spec.pid,
                label: spec.label,
                started_at_ms: 1,
                state: ProcessState::Running,
                cpu_centipercent: 0,
                memory_bytes: 0,
                samples: Vec::new(),
                nodes: Vec::new(),
                terminable: true,
            },
        ));
        Ok(id)
    }

    fn unregister(&self, root_id: &ProcessRootId) {
        self.roots
            .lock()
            .unwrap()
            .retain(|(_, root)| &root.root_id != root_id);
    }

    fn snapshot(&self, workspace: Option<&CoreWorkspaceId>) -> ProcessSnapshot {
        let roots = self
            .roots
            .lock()
            .unwrap()
            .iter()
            .filter(|(owner, _)| workspace.is_none_or(|workspace| owner == workspace))
            .map(|(_, root)| root.clone())
            .collect();
        ProcessSnapshot {
            sampled_at_ms: 10,
            roots,
        }
    }

    fn terminate(
        &self,
        workspace: Option<&CoreWorkspaceId>,
        root_id: &ProcessRootId,
    ) -> Result<(), ProcessError> {
        let mut roots = self.roots.lock().unwrap();
        let Some((owner, root)) = roots.iter_mut().find(|(_, root)| &root.root_id == root_id)
        else {
            return Err(ProcessError::new(
                ProcessErrorKind::NotFound,
                "process root was not found",
            ));
        };
        if workspace.is_some_and(|workspace| owner != workspace) {
            return Err(ProcessError::new(
                ProcessErrorKind::NotOwned,
                "process root belongs to another workspace",
            ));
        }
        root.state = ProcessState::Exited;
        self.terminated.lock().unwrap().push(root_id.clone());
        Ok(())
    }
}

fn body(workspace: &WorkspaceId, operation: Operation) -> RequestBody {
    RequestBody::WorkspaceExecute(WorkspaceOperationRequest {
        workspace_id: workspace.clone(),
        operation,
    })
}

fn connect(service: BackendService, session: &str, workspace: &str) -> EmbeddedClient {
    EmbeddedClient::connect(
        service,
        SessionId::try_new(session).unwrap(),
        WorkspaceId::try_new(workspace).unwrap(),
    )
    .unwrap()
}

#[test]
fn process_operations_are_scoped_to_the_connected_workspace() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let processes = Arc::new(FakeProcesses::default());
    processes.roots.lock().unwrap().push(root("first"));
    let mut second_root = root("second");
    second_root.1.root_id = ProcessRootId::try_new("process-2").unwrap();
    processes.roots.lock().unwrap().push(second_root);
    let registrations = [("first", &first), ("second", &second)].map(|(name, directory)| {
        WorkspaceRegistration::new(
            WorkspaceId::try_new(name).unwrap(),
            name,
            directory.path().to_owned(),
            Arc::new(EmptyFiles),
        )
        .with_processes(processes.clone())
    });
    let service = BackendService::start(BackendState::new(registrations).unwrap()).unwrap();
    let client = connect(service, "client", "first");

    let ResponseBody::WorkspaceExecute(Outcome::Processes(snapshot)) = client
        .request(body(&client.workspace_id().clone(), Operation::Processes))
        .unwrap()
    else {
        panic!("process snapshot was not returned");
    };
    assert_eq!(snapshot.roots.len(), 1);
    assert_eq!(
        snapshot.roots[0].owner,
        ProcessOwner::terminal("terminal-1")
    );

    let first_id = snapshot.roots[0].root_id.clone();
    let response = client
        .request(body(
            &client.workspace_id().clone(),
            Operation::TerminateProcess {
                root_id: first_id.clone(),
            },
        ))
        .unwrap();
    assert_eq!(response, ResponseBody::WorkspaceExecute(Outcome::Completed));
    assert_eq!(*processes.terminated.lock().unwrap(), vec![first_id]);

    let second_id = ProcessRootId::try_new("process-2").unwrap();
    let ResponseBody::Error(error) = client
        .request(body(
            &client.workspace_id().clone(),
            Operation::TerminateProcess { root_id: second_id },
        ))
        .unwrap()
    else {
        panic!("another workspace's process must be rejected");
    };
    assert_eq!(error.code.as_str(), client_api::FORBIDDEN);
}

#[test]
fn process_operations_are_unsupported_without_a_registry() {
    let directory = tempfile::tempdir().unwrap();
    let registration = WorkspaceRegistration::new(
        WorkspaceId::try_new("first").unwrap(),
        "first",
        directory.path().to_owned(),
        Arc::new(EmptyFiles),
    );
    let service = BackendService::start(BackendState::new([registration]).unwrap()).unwrap();
    let client = connect(service, "client", "first");
    let ResponseBody::Error(error) = client
        .request(body(&client.workspace_id().clone(), Operation::Processes))
        .unwrap()
    else {
        panic!("process registry must be required");
    };
    assert_eq!(error.code.as_str(), client_api::UNSUPPORTED);
}
