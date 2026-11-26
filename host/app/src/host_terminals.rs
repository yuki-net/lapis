//! Host が所有する Terminal の Client 向け接続。
//!
//! in-process Host は同じ process の `TerminalService` を直接呼び、リモート接続は
//! `RequestBody::Terminal` を送る。要求は Workspace と Workspace 相対 path だけを
//! 運び、応答の snapshot は Client 側の path へ戻してから UI へ渡す。

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use client_api::{
    RequestBody, ResponseBody, SessionId, TerminalEvent, TerminalId, TerminalInput,
    TerminalProfileId, TerminalProfiles, TerminalRequest, TerminalResponse, TerminalSnapshot,
    WorkspaceId, WorkspaceRelativePath,
};
use local_terminal::TerminalService;

use crate::{BackendClient, BackendHost, features::Connections};

/// in-process Host の Terminal を操作する呼び出し元。Desktop は 1 process で共有する。
const EMBEDDED: &str = "embedded";

struct Located {
    workspace: WorkspaceId,
    /// Client から見た Workspace root。応答の snapshot をこの下へ戻す。
    root: PathBuf,
    relative: Option<WorkspaceRelativePath>,
}

pub(super) struct HostTerminals {
    host: BackendClient,
    connections: Connections,
    embedded: Option<Arc<TerminalService>>,
    /// Terminal は Workspace を引数に取らない操作を持つため、開始・attach で対応を覚える。
    sessions: Mutex<BTreeMap<String, Located>>,
    /// 稼働中の Terminal 数。changes() の通知を続けるかの判断にだけ使う。
    count: Arc<AtomicUsize>,
    notification_pending: Arc<AtomicBool>,
    changes: Mutex<Option<futures_channel::mpsc::UnboundedReceiver<()>>>,
    _lifetime: Arc<()>,
}

impl HostTerminals {
    pub fn new(host: BackendClient) -> Self {
        let embedded = host.embedded_host().map(BackendHost::terminals_service);
        let count = Arc::new(AtomicUsize::new(0));
        let pending = Arc::new(AtomicBool::new(false));
        let lifetime = Arc::new(());
        let weak = Arc::downgrade(&lifetime);
        let thread_count = Arc::clone(&count);
        let thread_pending = Arc::clone(&pending);
        let (sender, receiver) = futures_channel::mpsc::unbounded();
        // UI は changes() を購読して poll する。Wire には通知を載せないため、
        // Terminal が存在する間だけ 100ms 間隔で 1 回分の通知を送る。
        std::thread::spawn(move || {
            while weak.upgrade().is_some() {
                if thread_count.load(Ordering::Acquire) > 0
                    && !thread_pending.swap(true, Ordering::AcqRel)
                    && sender.unbounded_send(()).is_err()
                {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });
        Self {
            connections: Connections::new(host.clone()),
            host,
            embedded,
            sessions: Mutex::new(BTreeMap::new()),
            count,
            notification_pending: pending,
            changes: Mutex::new(Some(receiver)),
            _lifetime: lifetime,
        }
    }

    /// `cwd` から Workspace と Workspace 相対 path を解決する。
    fn locate(&self, cwd: &Path) -> Result<Located, terminal::TerminalError> {
        let cwd = self
            .host
            .normalize_path(cwd)
            .map_err(|error| terminal::TerminalError::new(error.to_string()))?;
        let roots = self
            .host
            .roots()
            .map_err(|error| terminal::TerminalError::new(error.to_string()))?;
        let found = roots
            .into_iter()
            .filter(|(root, _)| cwd.starts_with(root))
            .max_by_key(|(root, _)| root.components().count());
        let (root, workspace) = match found {
            Some((root, workspace)) => (root, workspace),
            None => self
                .host
                .workspace_for_directory(&cwd)
                .map_err(|error| terminal::TerminalError::new(error.to_string()))?,
        };
        let relative = cwd
            .strip_prefix(&root)
            .ok()
            .and_then(|path| WorkspaceRelativePath::from_path(path).ok());
        Ok(Located {
            workspace,
            root,
            relative,
        })
    }

    fn remember(&self, id: &TerminalId, located: &Located) -> Result<(), terminal::TerminalError> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| terminal::TerminalError::new("Terminal state lock failed"))?;
        let previous = sessions.insert(
            id.as_str().to_owned(),
            Located {
                workspace: located.workspace.clone(),
                root: located.root.clone(),
                relative: None,
            },
        );
        if previous.is_none() {
            self.count.fetch_add(1, Ordering::Release);
        }
        Ok(())
    }

    fn session(&self, id: &TerminalId) -> Result<Located, terminal::TerminalError> {
        self.sessions
            .lock()
            .map_err(|_| terminal::TerminalError::new("Terminal state lock failed"))?
            .get(id.as_str())
            .map(|located| Located {
                workspace: located.workspace.clone(),
                root: located.root.clone(),
                relative: None,
            })
            .ok_or_else(|| terminal::TerminalError::new("Terminal is not attached"))
    }

    fn request(
        &self,
        workspace: &WorkspaceId,
        request: TerminalRequest,
    ) -> Result<TerminalResponse, terminal::TerminalError> {
        let client = self
            .connections
            .connect(workspace.clone())
            .map_err(terminal::TerminalError::new)?;
        match client
            .request(RequestBody::Terminal(request))
            .map_err(|error| terminal::TerminalError::new(error.to_string()))?
        {
            ResponseBody::Terminal(response) => Ok(response),
            ResponseBody::Error(error) => Err(terminal::TerminalError::new(
                error
                    .detail
                    .unwrap_or_else(|| error.code.as_str().to_owned()),
            )),
            _ => Err(unexpected()),
        }
    }
}

fn unexpected() -> terminal::TerminalError {
    terminal::TerminalError::new("Unexpected terminal response")
}

/// Host が送る Workspace 相対 path を Client の path へ戻す。
fn client_path(root: &Path, relative: &Path) -> PathBuf {
    if relative.as_os_str().is_empty() {
        root.to_owned()
    } else {
        root.join(relative)
    }
}

fn rebase_snapshot(mut snapshot: TerminalSnapshot, root: &Path) -> TerminalSnapshot {
    snapshot.cwd = client_path(root, &snapshot.cwd);
    snapshot
}

fn rebase_event(event: TerminalEvent, root: &Path) -> TerminalEvent {
    match event {
        TerminalEvent::Snapshot(snapshot) => {
            TerminalEvent::Snapshot(rebase_snapshot(snapshot, root))
        }
        other => other,
    }
}

impl terminal::TerminalBackend for HostTerminals {
    fn start(
        &self,
        cwd: &Path,
        columns: u16,
        rows: u16,
    ) -> Result<TerminalId, terminal::TerminalError> {
        self.start_profile(cwd, columns, rows, None)
    }

    fn start_profile(
        &self,
        cwd: &Path,
        columns: u16,
        rows: u16,
        profile: Option<&TerminalProfileId>,
    ) -> Result<TerminalId, terminal::TerminalError> {
        let located = self.locate(cwd)?;
        let id = match &self.embedded {
            Some(service) => service.start(
                located.workspace.as_str(),
                EMBEDDED,
                cwd,
                columns,
                rows,
                profile,
            )?,
            None => {
                let response = self.request(
                    &located.workspace,
                    TerminalRequest::StartProfile {
                        workspace_id: located.workspace.clone(),
                        cwd: located.relative.clone(),
                        columns,
                        rows,
                        profile: profile.cloned(),
                    },
                )?;
                let TerminalResponse::Started { id } = response else {
                    return Err(unexpected());
                };
                id
            }
        };
        self.remember(&id, &located)?;
        Ok(id)
    }

    fn profiles(&self, cwd: &Path) -> Result<TerminalProfiles, terminal::TerminalError> {
        let located = self.locate(cwd)?;
        match &self.embedded {
            Some(service) => service.profiles_for(located.workspace.as_str(), cwd),
            None => {
                let response = self.request(
                    &located.workspace,
                    TerminalRequest::Profiles {
                        workspace_id: located.workspace.clone(),
                        cwd: located.relative.clone(),
                    },
                )?;
                let TerminalResponse::Profiles { profiles } = response else {
                    return Err(unexpected());
                };
                Ok(profiles)
            }
        }
    }

    fn list(&self, cwd: &Path) -> Result<Vec<TerminalSnapshot>, terminal::TerminalError> {
        let located = self.locate(cwd)?;
        match &self.embedded {
            Some(service) => service.list(located.workspace.as_str(), cwd),
            None => {
                let response = self.request(
                    &located.workspace,
                    TerminalRequest::List {
                        workspace_id: located.workspace.clone(),
                        cwd: located.relative.clone(),
                    },
                )?;
                let TerminalResponse::Sessions { sessions } = response else {
                    return Err(unexpected());
                };
                let sessions = sessions
                    .into_iter()
                    .map(|snapshot| {
                        self.remember(&snapshot.id, &located).ok();
                        rebase_snapshot(snapshot, &located.root)
                    })
                    .collect();
                Ok(sessions)
            }
        }
    }

    fn attach(
        &self,
        cwd: &Path,
        id: &TerminalId,
    ) -> Result<TerminalSnapshot, terminal::TerminalError> {
        let located = self.locate(cwd)?;
        let snapshot = match &self.embedded {
            Some(service) => service.attach(located.workspace.as_str(), EMBEDDED, id)?,
            None => {
                let response = self.request(
                    &located.workspace,
                    TerminalRequest::Attach {
                        workspace_id: located.workspace.clone(),
                        cwd: located.relative.clone(),
                        id: id.clone(),
                    },
                )?;
                let TerminalResponse::Attached { snapshot } = response else {
                    return Err(unexpected());
                };
                rebase_snapshot(snapshot, &located.root)
            }
        };
        self.remember(id, &located)?;
        Ok(snapshot)
    }

    fn input(&self, id: &TerminalId, input: &TerminalInput) -> Result<(), terminal::TerminalError> {
        let located = self.session(id)?;
        match &self.embedded {
            Some(service) => service.input(located.workspace.as_str(), EMBEDDED, id, input),
            None => self.complete(TerminalRequest::Input {
                workspace_id: located.workspace,
                id: id.clone(),
                input: input.clone(),
            }),
        }
    }

    fn resize(
        &self,
        id: &TerminalId,
        columns: u16,
        rows: u16,
    ) -> Result<(), terminal::TerminalError> {
        let located = self.session(id)?;
        match &self.embedded {
            Some(service) => {
                service.resize(located.workspace.as_str(), EMBEDDED, id, columns, rows)
            }
            None => self.complete(TerminalRequest::Resize {
                workspace_id: located.workspace,
                id: id.clone(),
                columns,
                rows,
            }),
        }
    }

    fn poll(&self, id: &TerminalId) -> Result<Vec<TerminalEvent>, terminal::TerminalError> {
        self.notification_pending.store(false, Ordering::Release);
        let located = self.session(id)?;
        match &self.embedded {
            Some(service) => service.poll(located.workspace.as_str(), EMBEDDED, id),
            None => {
                let response = self.request(
                    &located.workspace,
                    TerminalRequest::Poll {
                        workspace_id: located.workspace.clone(),
                        id: id.clone(),
                    },
                )?;
                let TerminalResponse::Events { events } = response else {
                    return Err(unexpected());
                };
                Ok(events
                    .into_iter()
                    .map(|event| rebase_event(event, &located.root))
                    .collect())
            }
        }
    }

    fn terminate(&self, id: &TerminalId) -> Result<(), terminal::TerminalError> {
        let located = self.session(id)?;
        match &self.embedded {
            Some(service) => service.terminate(located.workspace.as_str(), EMBEDDED, id),
            None => self.complete(TerminalRequest::Terminate {
                workspace_id: located.workspace,
                id: id.clone(),
            }),
        }?;
        if let Ok(mut sessions) = self.sessions.lock()
            && sessions.remove(id.as_str()).is_some()
        {
            self.count.fetch_sub(1, Ordering::Release);
        }
        Ok(())
    }

    fn claim(&self, id: &TerminalId) -> Result<(), terminal::TerminalError> {
        let located = self.session(id)?;
        match &self.embedded {
            Some(service) => service.claim(located.workspace.as_str(), EMBEDDED, id),
            None => self.complete(TerminalRequest::Claim {
                workspace_id: located.workspace,
                id: id.clone(),
            }),
        }
    }

    fn release(&self, id: &TerminalId) -> Result<(), terminal::TerminalError> {
        let located = self.session(id)?;
        match &self.embedded {
            Some(service) => service.release(located.workspace.as_str(), EMBEDDED, id),
            None => self.complete(TerminalRequest::Release {
                workspace_id: located.workspace,
                id: id.clone(),
            }),
        }
    }

    fn changes(&self) -> Option<futures_channel::mpsc::UnboundedReceiver<()>> {
        self.changes.lock().ok()?.take()
    }
}

impl HostTerminals {
    fn complete(&self, request: TerminalRequest) -> Result<(), terminal::TerminalError> {
        let workspace = request.workspace_id().clone();
        match self.request(&workspace, request)? {
            TerminalResponse::Completed => Ok(()),
            _ => Err(unexpected()),
        }
    }
}

/// リモート接続の Terminal 要求を Host の service で実行する。
pub(crate) fn dispatch(
    host: &BackendHost,
    session: &SessionId,
    workspace: &WorkspaceId,
    request: TerminalRequest,
) -> ResponseBody {
    if request.workspace_id() != workspace {
        return error(client_api::WORKSPACE_MISMATCH);
    }
    let Some(root) = host.workspace_root(workspace) else {
        return error(client_api::NOT_FOUND);
    };
    let Some(cwd) = resolve(&root, request.cwd()) else {
        return error(client_api::INVALID_PATH);
    };
    let service = host.terminals_service();
    let caller = session.as_str();
    let name = workspace.as_str();
    let result = match request {
        TerminalRequest::Start { columns, rows, .. } => service
            .start(name, caller, &cwd, columns, rows, None)
            .map(|id| TerminalResponse::Started { id }),
        TerminalRequest::StartProfile {
            columns,
            rows,
            profile,
            ..
        } => service
            .start(name, caller, &cwd, columns, rows, profile.as_ref())
            .map(|id| TerminalResponse::Started { id }),
        TerminalRequest::Profiles { .. } => service
            .profiles_for(name, &cwd)
            .map(|profiles| TerminalResponse::Profiles { profiles }),
        TerminalRequest::List { .. } => service
            .list(name, &cwd)
            .map(|sessions| TerminalResponse::Sessions { sessions }),
        TerminalRequest::Attach { id, .. } => service
            .attach(name, caller, &id)
            .map(|snapshot| TerminalResponse::Attached { snapshot }),
        TerminalRequest::Input { id, input, .. } => service
            .input(name, caller, &id, &input)
            .map(|()| completed()),
        TerminalRequest::Resize {
            id, columns, rows, ..
        } => service
            .resize(name, caller, &id, columns, rows)
            .map(|()| completed()),
        TerminalRequest::Poll { id, .. } => service
            .poll(name, caller, &id)
            .map(|events| TerminalResponse::Events { events }),
        TerminalRequest::Terminate { id, .. } => {
            service.terminate(name, caller, &id).map(|()| completed())
        }
        TerminalRequest::Claim { id, .. } => service.claim(name, caller, &id).map(|()| completed()),
        TerminalRequest::Release { id, .. } => {
            service.release(name, caller, &id).map(|()| completed())
        }
    };
    match result {
        Ok(response) => ResponseBody::Terminal(relative(response, &root)),
        Err(message) => ResponseBody::Error(
            client_api::ProtocolError::new(
                client_api::ErrorCode::try_new(client_api::INTERNAL).expect("built-in error"),
            )
            .with_detail(message.to_string()),
        ),
    }
}

fn completed() -> TerminalResponse {
    TerminalResponse::Completed
}

fn resolve(root: &Path, relative: Option<&WorkspaceRelativePath>) -> Option<PathBuf> {
    let path = match relative {
        Some(relative) => root.join(relative.as_str()),
        None => root.to_owned(),
    };
    let resolved = path.canonicalize().ok()?;
    resolved.starts_with(root).then_some(resolved)
}

/// 応答の snapshot から Host の絶対 path を除き、Workspace 相対 path にする。
fn relative(response: TerminalResponse, root: &Path) -> TerminalResponse {
    let relative_snapshot = |mut snapshot: TerminalSnapshot| {
        snapshot.cwd = snapshot
            .cwd
            .strip_prefix(root)
            .map(Path::to_owned)
            .unwrap_or_default();
        snapshot
    };
    match response {
        TerminalResponse::Started { id } => TerminalResponse::Started { id },
        TerminalResponse::Profiles { profiles } => TerminalResponse::Profiles { profiles },
        TerminalResponse::Sessions { sessions } => TerminalResponse::Sessions {
            sessions: sessions.into_iter().map(relative_snapshot).collect(),
        },
        TerminalResponse::Attached { snapshot } => TerminalResponse::Attached {
            snapshot: relative_snapshot(snapshot),
        },
        TerminalResponse::Events { events } => TerminalResponse::Events {
            events: events
                .into_iter()
                .map(|event| match event {
                    TerminalEvent::Snapshot(snapshot) => {
                        TerminalEvent::Snapshot(relative_snapshot(snapshot))
                    }
                    other => other,
                })
                .collect(),
        },
        TerminalResponse::Completed => TerminalResponse::Completed,
    }
}

fn error(code: &str) -> ResponseBody {
    ResponseBody::Error(client_api::ProtocolError::new(
        client_api::ErrorCode::try_new(code).expect("built-in error"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use terminal::{TerminalKey, TerminalKeyEvent, TerminalRevision, TerminalScreen};

    fn dispatch(
        host: &BackendHost,
        session: &SessionId,
        workspace: &WorkspaceId,
        body: RequestBody,
    ) -> TerminalResponse {
        // Client と同じ envelope を通して wire の往復を検証する。
        let envelope = client_api::RequestEnvelope {
            request_id: client_api::RequestId::try_new("terminal-request").unwrap(),
            body,
        };
        let encoded = serde_json::to_string(&envelope).unwrap();
        let decoded: client_api::RequestEnvelope = serde_json::from_str(&encoded).unwrap();
        let RequestBody::Terminal(request) = decoded.body else {
            panic!("expected a terminal request");
        };
        let response = super::dispatch(host, session, workspace, request);
        let ResponseBody::Terminal(response) = response else {
            panic!("terminal request failed: {response:?}");
        };
        let response = client_api::ResponseEnvelope {
            request_id: envelope.request_id,
            body: ResponseBody::Terminal(response),
        };
        let encoded = serde_json::to_string(&response).unwrap();
        assert!(
            !encoded.contains(&host_root(host, workspace).to_string_lossy().into_owned()),
            "wire response must not contain a host path: {encoded}"
        );
        let decoded: client_api::ResponseEnvelope = serde_json::from_str(&encoded).unwrap();
        let ResponseBody::Terminal(response) = decoded.body else {
            panic!("expected a terminal response");
        };
        response
    }

    fn host_root(host: &BackendHost, workspace: &WorkspaceId) -> PathBuf {
        host.workspace_root(workspace)
            .expect("workspace is registered")
    }

    struct Screen {
        model: TerminalScreen,
        revision: TerminalRevision,
    }

    impl Screen {
        fn apply(&mut self, events: Vec<TerminalEvent>) {
            for event in events {
                match event {
                    TerminalEvent::Delta(delta) if delta.revision == self.revision + 1 => {
                        self.model.apply_delta(&delta).unwrap();
                        self.revision = delta.revision;
                    }
                    TerminalEvent::Snapshot(snapshot) if snapshot.revision >= self.revision => {
                        self.model = snapshot.screen;
                        self.revision = snapshot.revision;
                    }
                    _ => {}
                }
            }
        }
    }

    fn poll(
        host: &BackendHost,
        session: &SessionId,
        workspace: &WorkspaceId,
        id: &TerminalId,
        screen: &mut Screen,
    ) -> String {
        let response = dispatch(
            host,
            session,
            workspace,
            RequestBody::Terminal(TerminalRequest::Poll {
                workspace_id: workspace.clone(),
                id: id.clone(),
            }),
        );
        let TerminalResponse::Events { events } = response else {
            panic!("expected poll events");
        };
        screen.apply(events);
        screen.model.plain_text()
    }

    #[test]
    fn embedded_host_terminals_start_and_read_a_session_through_the_wire() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let host = BackendHost::start().unwrap();
        let workspace = host.register_workspace(&root).unwrap();
        let session = SessionId::try_new("terminal-test-session").unwrap();

        let profiles = match dispatch(
            &host,
            &session,
            &workspace,
            RequestBody::Terminal(TerminalRequest::Profiles {
                workspace_id: workspace.clone(),
                cwd: None,
            }),
        ) {
            TerminalResponse::Profiles { profiles } => profiles,
            _ => panic!("expected profiles"),
        };
        let profile = profiles.default.clone().expect("default profile is set");
        assert!(profiles.find(&profile).is_some());

        let id = match dispatch(
            &host,
            &session,
            &workspace,
            RequestBody::Terminal(TerminalRequest::StartProfile {
                workspace_id: workspace.clone(),
                cwd: None,
                columns: 160,
                rows: 40,
                profile: Some(profile),
            }),
        ) {
            TerminalResponse::Started { id } => id,
            _ => panic!("expected a started terminal"),
        };

        let snapshot = match dispatch(
            &host,
            &session,
            &workspace,
            RequestBody::Terminal(TerminalRequest::Attach {
                workspace_id: workspace.clone(),
                cwd: None,
                id: id.clone(),
            }),
        ) {
            TerminalResponse::Attached { snapshot } => snapshot,
            _ => panic!("expected an attached terminal"),
        };
        assert!(
            snapshot.cwd.as_os_str().is_empty(),
            "the root snapshot path must stay workspace-relative"
        );
        let mut screen = Screen {
            model: snapshot.screen,
            revision: snapshot.revision,
        };

        // ConPTY の初期化問い合わせが終わるまで待ってから入力する。
        let deadline = Instant::now() + Duration::from_secs(10);
        while screen.model.plain_text().trim().is_empty() {
            poll(&host, &session, &workspace, &id, &mut screen);
            assert!(Instant::now() < deadline, "terminal did not become ready");
            std::thread::sleep(Duration::from_millis(20));
        }
        for input in [
            TerminalInput::Text("echo lapis-wire".to_owned()),
            TerminalInput::Key(TerminalKeyEvent {
                key: TerminalKey::Enter,
                modifiers: Default::default(),
            }),
        ] {
            let TerminalResponse::Completed = dispatch(
                &host,
                &session,
                &workspace,
                RequestBody::Terminal(TerminalRequest::Input {
                    workspace_id: workspace.clone(),
                    id: id.clone(),
                    input,
                }),
            ) else {
                panic!("expected a completed input");
            };
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let text = poll(&host, &session, &workspace, &id, &mut screen);
            if text.contains("lapis-wire") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "terminal output did not contain the typed line: {text:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }

        let TerminalResponse::Completed = dispatch(
            &host,
            &session,
            &workspace,
            RequestBody::Terminal(TerminalRequest::Terminate {
                workspace_id: workspace.clone(),
                id: id.clone(),
            }),
        ) else {
            panic!("expected a completed termination");
        };
        let list = match dispatch(
            &host,
            &session,
            &workspace,
            RequestBody::Terminal(TerminalRequest::List {
                workspace_id: workspace.clone(),
                cwd: None,
            }),
        ) {
            TerminalResponse::Sessions { sessions } => sessions,
            _ => panic!("expected sessions"),
        };
        assert!(list.is_empty());
    }

    #[test]
    fn remote_terminal_requests_are_rejected_for_another_workspace() {
        let directory = tempfile::tempdir().unwrap();
        let host = BackendHost::start().unwrap();
        let workspace = host.register_workspace(directory.path()).unwrap();
        let other = WorkspaceId::try_new("workspace-other").unwrap();
        let session = SessionId::try_new("terminal-test-session").unwrap();
        let response = super::dispatch(
            &host,
            &session,
            &workspace,
            TerminalRequest::List {
                workspace_id: other,
                cwd: None,
            },
        );
        let ResponseBody::Error(error) = response else {
            panic!("expected a workspace mismatch");
        };
        assert_eq!(error.code.as_str(), client_api::WORKSPACE_MISMATCH);
    }
}
