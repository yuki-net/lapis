//! Web の認証済み接続に紐づく Terminal セッション。

use std::{cell::RefCell, collections::HashMap};
use std::{path::Path, rc::Rc};

use client_api::{
    RequestBody, ResponseBody, TerminalRequest, TerminalResponse,
    WorkspaceRelativePath as ApiWorkspaceRelativePath,
};
use client_core::browser::BrowserConnection;
use editor_core::WorkspaceId;
use futures_channel::mpsc;
use futures_util::{FutureExt, StreamExt};
use terminal::{
    TerminalBackend, TerminalError, TerminalEvent, TerminalId, TerminalInput, TerminalProfileId,
    TerminalProfiles, TerminalScreen, TerminalSize, TerminalSnapshot, TerminalStatus,
};

use super::{BACKEND, SWITCHING, TerminalOutputStream};
use crate::{
    terminal_extension::{v1, web_projection::Projection},
    web_switch::{terminal_relative_cwd, validate_terminal_sessions},
};

pub struct ExtensionTerminalUpdate {
    pub screen: TerminalScreen,
    pub owner: Option<String>,
    pub exit: Option<v1::Exit>,
    pub read_error: Option<String>,
    pub write_error: Option<String>,
    /// 画面反映は済んだが、PTY 問い合わせへの応答が送れなかった場合。
    pub reply_error: Option<String>,
}

enum UiCommand {
    Input(TerminalInput),
    Resize(TerminalSize),
    Claim,
    Release,
    Close,
}

struct UiEntry {
    projection: Projection,
    commands: mpsc::Sender<UiCommand>,
    /// Host が払い出した ID。ローカル ID との重複表示を防ぐ。
    host_id: Option<String>,
}

struct UiRegistry {
    connection: Rc<BrowserConnection>,
    workspace: WorkspaceId,
    entries: HashMap<String, UiEntry>,
    discovered: Vec<TerminalSnapshot>,
    profiles: Option<TerminalProfiles>,
    changes: Vec<mpsc::UnboundedSender<()>>,
}

thread_local! {
    static UI_REGISTRY: RefCell<Option<Rc<RefCell<UiRegistry>>>> = const { RefCell::new(None) };
}

/// 既存 UI の同期 port を Web の非同期 Terminal 接続へ接続する。
#[derive(Clone, Copy)]
pub struct ExtensionTerminalBackend;

fn notify(registry: &mut UiRegistry) {
    registry
        .changes
        .retain(|sender| sender.unbounded_send(()).is_ok());
}

fn registry() -> Result<Rc<RefCell<UiRegistry>>, TerminalError> {
    let (connection, workspace) = connected().map_err(TerminalError::new)?;
    UI_REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        if let Some(existing) = slot.as_ref() {
            let state = existing.borrow();
            if Rc::ptr_eq(&state.connection, &connection) && state.workspace == workspace {
                return Ok(existing.clone());
            }
        }
        let state = Rc::new(RefCell::new(UiRegistry {
            connection,
            workspace,
            entries: HashMap::new(),
            discovered: Vec::new(),
            profiles: None,
            changes: Vec::new(),
        }));
        *slot = Some(state.clone());
        let active = state.clone();
        wasm_bindgen_futures::spawn_local(async move {
            loop {
                let (connection, workspace) = {
                    let state = active.borrow();
                    (state.connection.clone(), state.workspace.clone())
                };
                if ensure_current(&connection, &workspace).is_err() {
                    break;
                }
                if let Ok(profiles) = ExtensionTerminal::profiles().await
                    && ensure_current(&connection, &workspace).is_ok()
                {
                    let mut state = active.borrow_mut();
                    if state.profiles.as_ref() != Some(&profiles) {
                        state.profiles = Some(profiles);
                        notify(&mut state);
                    }
                }
                if let Ok(sessions) = ExtensionTerminal::list().await
                    && ensure_current(&connection, &workspace).is_ok()
                {
                    let mut state = active.borrow_mut();
                    let active_ids = state
                        .entries
                        .values()
                        .filter_map(|entry| entry.host_id.clone())
                        .collect::<Vec<_>>();
                    let found = sessions
                        .into_iter()
                        .filter(|session| {
                            !active_ids
                                .iter()
                                .any(|id| id.as_str() == session.id.as_str())
                        })
                        .map(|session| {
                            let cwd = Path::new(super::ROOT).join(&session.cwd);
                            TerminalSnapshot::empty(
                                session.id,
                                cwd,
                                TerminalStatus::Running,
                                session.screen.size,
                            )
                        })
                        .collect::<Vec<_>>();
                    if state.discovered != found {
                        state.discovered = found;
                        notify(&mut state);
                    }
                }
                gloo_timers::future::TimeoutFuture::new(2_000).await;
            }
        });
        Ok(state)
    })
}

fn random_id() -> Result<TerminalId, TerminalError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| TerminalError::new(error.to_string()))?;
    Ok(TerminalId::new(format!(
        "client-{}",
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )))
}

fn send(id: &TerminalId, command: UiCommand) -> Result<(), TerminalError> {
    let registry = registry()?;
    let mut state = registry.borrow_mut();
    let entry = state
        .entries
        .get_mut(id.as_str())
        .ok_or_else(|| TerminalError::new("Terminal がありません"))?;
    if entry.projection.finished {
        return Err(TerminalError::new("Terminal は終了しています"));
    }
    entry
        .commands
        .try_send(command)
        .map_err(|_| TerminalError::new("Terminal の送信待ちが一杯です"))
}

fn create_actor(
    id: TerminalId,
    cwd: &Path,
    size: TerminalSize,
    attach: bool,
    profile: Option<String>,
) -> Result<TerminalSnapshot, TerminalError> {
    terminal_relative_cwd(cwd).map_err(TerminalError::new)?;
    if size.columns == 0 || size.rows == 0 || size.columns > 512 || size.rows > 256 {
        return Err(TerminalError::new("Terminal のサイズが不正です"));
    }
    let registry = registry()?;
    if let Some(existing) = registry.borrow().entries.get(id.as_str()) {
        return Ok(existing.projection.snapshot.clone());
    }
    if registry.borrow().entries.len() >= 64 {
        return Err(TerminalError::new("Terminal の数が上限に達しました"));
    }
    let path = cwd.to_owned();
    let snapshot = TerminalSnapshot::empty(id.clone(), path.clone(), TerminalStatus::Running, size);
    let (commands, receiver) = mpsc::channel(32);
    registry.borrow_mut().entries.insert(
        id.as_str().into(),
        UiEntry {
            projection: Projection::new(snapshot.clone()),
            commands,
            host_id: attach.then(|| id.as_str().to_owned()),
        },
    );
    let state = registry.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let result = if attach {
            ExtensionTerminal::attach(id.as_str(), &path).await
        } else {
            ExtensionTerminal::start_profile(&path, size, profile).await
        };
        match result {
            Ok(session) => run_actor(state, id, session, receiver).await,
            Err(error) => fail_actor(&state, &id, error),
        }
    });
    Ok(snapshot)
}

fn fail_actor(registry: &Rc<RefCell<UiRegistry>>, id: &TerminalId, error: String) {
    let mut state = registry.borrow_mut();
    if let Some(entry) = state.entries.get_mut(id.as_str()) {
        entry.projection.fail(error);
        notify(&mut state);
    }
}

async fn run_actor(
    registry: Rc<RefCell<UiRegistry>>,
    id: TerminalId,
    mut session: ExtensionTerminal,
    mut commands: mpsc::Receiver<UiCommand>,
) {
    {
        let mut state = registry.borrow_mut();
        if let Some(entry) = state.entries.get_mut(id.as_str()) {
            entry.host_id = Some(session.id().to_owned());
        }
    }
    let mut commands_since_read = 0u8;
    loop {
        let command = commands.next().fuse();
        let timer = gloo_timers::future::TimeoutFuture::new(100).fuse();
        futures_util::pin_mut!(command, timer);
        let incoming = futures_util::select! {
            command = command => Some(command),
            _ = timer => None,
        };
        let timer_fired = incoming.is_none();
        if let Some(command) = incoming {
            let result = match command {
                Some(UiCommand::Input(input)) => session.input(&input).await.map(|_| None),
                Some(UiCommand::Resize(size)) => session.resize(size).await.map(Some),
                Some(UiCommand::Claim) => session.claim().await.map(|_| None),
                Some(UiCommand::Release) => session.release().await.map(|_| None),
                Some(UiCommand::Close) => {
                    match session.close().await {
                        Ok(()) => {
                            let mut state = registry.borrow_mut();
                            if state.entries.remove(id.as_str()).is_some() {
                                notify(&mut state);
                            }
                        }
                        Err(error) => fail_actor(&registry, &id, error),
                    }
                    break;
                }
                None => break,
            };
            match result {
                Ok(Some(screen)) => publish_screen(
                    &registry,
                    &id,
                    screen,
                    session.owner().map(str::to_owned),
                    None,
                ),
                Ok(None) => {}
                Err(error) => {
                    fail_actor(&registry, &id, error);
                    break;
                }
            }
            commands_since_read += 1;
        }
        if timer_fired || commands_since_read >= 4 {
            commands_since_read = 0;
            match session.read(0).await {
                Ok(update) => {
                    let error = update
                        .read_error
                        .or(update.write_error)
                        .or(update.reply_error);
                    let exited = update.exit.map(|exit| exit.code);
                    if let Some(error) = error {
                        publish_screen(&registry, &id, update.screen, update.owner, None);
                        fail_actor(&registry, &id, error);
                        break;
                    }
                    publish_screen(&registry, &id, update.screen, update.owner, exited);
                    if exited.is_some() {
                        break;
                    }
                }
                Err(error) => {
                    fail_actor(&registry, &id, error);
                    break;
                }
            }
        }
    }
}

fn publish_screen(
    registry: &Rc<RefCell<UiRegistry>>,
    id: &TerminalId,
    screen: TerminalScreen,
    owner: Option<String>,
    exit: Option<Option<i32>>,
) {
    let mut state = registry.borrow_mut();
    let Some(entry) = state.entries.get_mut(id.as_str()) else {
        return;
    };
    let changed = entry.projection.update(screen, owner, exit);
    if changed {
        notify(&mut state);
    }
}

impl TerminalBackend for ExtensionTerminalBackend {
    fn start(&self, cwd: &Path, columns: u16, rows: u16) -> Result<TerminalId, TerminalError> {
        let id = random_id()?;
        create_actor(
            id.clone(),
            cwd,
            TerminalSize::new(columns, rows),
            false,
            None,
        )?;
        Ok(id)
    }
    fn start_profile(
        &self,
        cwd: &Path,
        columns: u16,
        rows: u16,
        profile: Option<&TerminalProfileId>,
    ) -> Result<TerminalId, TerminalError> {
        let id = random_id()?;
        create_actor(
            id.clone(),
            cwd,
            TerminalSize::new(columns, rows),
            false,
            profile.map(|id| id.as_str().to_owned()),
        )?;
        Ok(id)
    }
    fn profiles(&self, cwd: &Path) -> Result<TerminalProfiles, TerminalError> {
        terminal_relative_cwd(cwd).map_err(TerminalError::new)?;
        registry()?
            .borrow()
            .profiles
            .clone()
            .ok_or_else(|| TerminalError::new("Terminal profile の取得中です"))
    }
    fn list(&self, _cwd: &Path) -> Result<Vec<TerminalSnapshot>, TerminalError> {
        let registry = registry()?;
        let state = registry.borrow();
        Ok(state.discovered.clone())
    }
    fn attach(&self, cwd: &Path, id: &TerminalId) -> Result<TerminalSnapshot, TerminalError> {
        let registry = registry()?;
        let size = registry
            .borrow()
            .discovered
            .iter()
            .find(|item| item.id == *id)
            .map(|item| item.screen.size)
            .unwrap_or(TerminalSize::new(80, 24));
        create_actor(id.clone(), cwd, size, true, None)
    }
    fn input(&self, id: &TerminalId, input: &TerminalInput) -> Result<(), TerminalError> {
        send(id, UiCommand::Input(input.clone()))
    }
    fn resize(&self, id: &TerminalId, columns: u16, rows: u16) -> Result<(), TerminalError> {
        send(id, UiCommand::Resize(TerminalSize::new(columns, rows)))
    }
    fn claim(&self, id: &TerminalId) -> Result<(), TerminalError> {
        send(id, UiCommand::Claim)
    }
    fn release(&self, id: &TerminalId) -> Result<(), TerminalError> {
        send(id, UiCommand::Release)
    }
    fn terminate(&self, id: &TerminalId) -> Result<(), TerminalError> {
        send(id, UiCommand::Close)
    }
    fn poll(&self, id: &TerminalId) -> Result<Vec<TerminalEvent>, TerminalError> {
        let registry = registry()?;
        let mut state = registry.borrow_mut();
        let (events, finished) = {
            let entry = state
                .entries
                .get_mut(id.as_str())
                .ok_or_else(|| TerminalError::new("Terminal がありません"))?;
            (
                std::mem::take(&mut entry.projection.events),
                entry.projection.finished,
            )
        };
        if finished {
            state.entries.remove(id.as_str());
        }
        Ok(events)
    }
    fn changes(&self) -> Option<mpsc::UnboundedReceiver<()>> {
        let registry = registry().ok()?;
        let (sender, receiver) = mpsc::unbounded();
        registry.borrow_mut().changes.push(sender);
        Some(receiver)
    }
}

/// Future の呼び出し元が保持する。破棄・通信切断では PTY を終了しない。
pub struct ExtensionTerminal {
    connection: Rc<BrowserConnection>,
    workspace: WorkspaceId,
    id: String,
    cwd: String,
    stream: TerminalOutputStream,
    closed: bool,
}

fn connected() -> Result<(Rc<BrowserConnection>, WorkspaceId), String> {
    if SWITCHING.with(|switching| switching.get()) {
        return Err("Workspace切替中です".into());
    }
    BACKEND
        .with(|backend| {
            let backend = backend.borrow();
            let state = backend.as_ref().ok_or("Host に接続していません")?;
            if !state.extensions_connected {
                return Err("Host に接続していません");
            }
            Ok((state.connection.clone(), state.workspace.clone()))
        })
        .map_err(str::to_owned)
}

fn ensure_current(
    connection: &Rc<BrowserConnection>,
    workspace: &WorkspaceId,
) -> Result<(), String> {
    let (current, current_workspace) = connected()?;
    if !Rc::ptr_eq(connection, &current) || workspace != &current_workspace {
        return Err("Host または Workspace が変更されました".into());
    }
    Ok(())
}

/// UI の仮想 root 配下の path を、専用 wire の Workspace 相対 path へ変換する。
fn parse_cwd(value: &str) -> Result<Option<ApiWorkspaceRelativePath>, String> {
    if value == "." || value.is_empty() {
        return Ok(None);
    }
    ApiWorkspaceRelativePath::parse(value)
        .map(Some)
        .map_err(|_| "Terminal の作業パスが不正です".into())
}

async fn call(
    connection: &Rc<BrowserConnection>,
    workspace: &WorkspaceId,
    request: TerminalRequest,
) -> Result<TerminalResponse, String> {
    ensure_current(connection, workspace)?;
    let response = super::request(connection, RequestBody::Terminal(request)).await?;
    ensure_current(connection, workspace)?;
    let ResponseBody::Terminal(response) = response else {
        return Err("Terminal の応答が不正です".into());
    };
    Ok(response)
}

impl ExtensionTerminal {
    pub async fn profiles() -> Result<TerminalProfiles, String> {
        let (connection, workspace) = connected()?;
        let response = call(
            &connection,
            &workspace,
            TerminalRequest::Profiles {
                workspace_id: super::api_workspace(&workspace),
                cwd: None,
            },
        )
        .await?;
        let TerminalResponse::Profiles { profiles } = response else {
            return Err("Terminal profile の応答が不正です".into());
        };
        Ok(profiles)
    }

    pub async fn list() -> Result<Vec<TerminalSnapshot>, String> {
        let (connection, workspace) = connected()?;
        let response = call(
            &connection,
            &workspace,
            TerminalRequest::List {
                workspace_id: super::api_workspace(&workspace),
                cwd: None,
            },
        )
        .await?;
        let TerminalResponse::Sessions { sessions } = response else {
            return Err("Terminal 一覧の応答が不正です".into());
        };
        validate_terminal_sessions(&sessions)?;
        Ok(sessions)
    }

    pub async fn attach(id: &str, path: &Path) -> Result<Self, String> {
        let relative = terminal_relative_cwd(path)?;
        let (connection, workspace) = connected()?;
        let response = call(
            &connection,
            &workspace,
            TerminalRequest::Attach {
                workspace_id: super::api_workspace(&workspace),
                cwd: parse_cwd(&relative)?,
                id: TerminalId::new(id),
            },
        )
        .await?;
        let TerminalResponse::Attached { snapshot } = response else {
            return Err("Terminal の attach 応答が不正です".into());
        };
        Self::from_snapshot(connection, workspace, relative, snapshot)
    }

    pub async fn start_profile(
        path: &Path,
        size: TerminalSize,
        profile: Option<String>,
    ) -> Result<Self, String> {
        if size.columns == 0 || size.rows == 0 || size.columns > 512 || size.rows > 256 {
            return Err("Terminal のサイズが不正です".into());
        }
        let relative = terminal_relative_cwd(path)?;
        let (connection, workspace) = connected()?;
        let response = call(
            &connection,
            &workspace,
            TerminalRequest::StartProfile {
                workspace_id: super::api_workspace(&workspace),
                cwd: parse_cwd(&relative)?,
                columns: size.columns,
                rows: size.rows,
                profile: profile.map(TerminalProfileId::new),
            },
        )
        .await?;
        let TerminalResponse::Started { id } = response else {
            return Err("Terminal の開始応答が不正です".into());
        };
        let attached = call(
            &connection,
            &workspace,
            TerminalRequest::Attach {
                workspace_id: super::api_workspace(&workspace),
                cwd: parse_cwd(&relative)?,
                id,
            },
        )
        .await?;
        let TerminalResponse::Attached { snapshot } = attached else {
            return Err("Terminal の attach 応答が不正です".into());
        };
        Self::from_snapshot(connection, workspace, relative, snapshot)
    }

    fn from_snapshot(
        connection: Rc<BrowserConnection>,
        workspace: WorkspaceId,
        cwd: String,
        snapshot: TerminalSnapshot,
    ) -> Result<Self, String> {
        let id = snapshot.id.as_str().to_owned();
        if id.is_empty() || id.len() > 256 {
            return Err("Terminal session ID が不正です".into());
        }
        Ok(Self {
            connection,
            workspace,
            id,
            cwd,
            stream: TerminalOutputStream::new(snapshot),
            closed: false,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn working_directory(&self) -> &str {
        &self.cwd
    }
    pub fn owner(&self) -> Option<&str> {
        self.stream.owner()
    }

    fn ensure_open(&self) -> Result<(), String> {
        if self.closed {
            return Err("Terminal は終了しています".into());
        }
        ensure_current(&self.connection, &self.workspace)
    }

    pub async fn read(&mut self, _timeout_ms: u32) -> Result<ExtensionTerminalUpdate, String> {
        self.ensure_open()?;
        let response = call(
            &self.connection,
            &self.workspace,
            TerminalRequest::Poll {
                workspace_id: super::api_workspace(&self.workspace),
                id: TerminalId::new(self.id.clone()),
            },
        )
        .await?;
        let TerminalResponse::Events { events } = response else {
            return Err("Terminal の poll 応答が不正です".into());
        };
        if !self.stream.apply(events)? {
            // 差分が飛んだ。Host の snapshot から画面を作り直す。
            self.resync_screen().await?;
        }
        Ok(ExtensionTerminalUpdate {
            screen: self.stream.screen().clone(),
            owner: self.stream.owner().map(str::to_owned),
            exit: self.stream.take_exit().map(|code| v1::Exit { code }),
            read_error: self.stream.take_failure(),
            write_error: None,
            reply_error: None,
        })
    }

    async fn resync_screen(&mut self) -> Result<(), String> {
        let response = call(
            &self.connection,
            &self.workspace,
            TerminalRequest::Attach {
                workspace_id: super::api_workspace(&self.workspace),
                cwd: parse_cwd(&self.cwd)?,
                id: TerminalId::new(self.id.clone()),
            },
        )
        .await?;
        let TerminalResponse::Attached { snapshot } = response else {
            return Err("Terminal の attach 応答が不正です".into());
        };
        self.stream.reset(snapshot);
        Ok(())
    }

    async fn complete(&self, request: TerminalRequest) -> Result<(), String> {
        match call(&self.connection, &self.workspace, request).await? {
            TerminalResponse::Completed => Ok(()),
            _ => Err("Terminal の応答が不正です".into()),
        }
    }

    pub async fn input(&self, input: &TerminalInput) -> Result<(), String> {
        self.ensure_open()?;
        self.complete(TerminalRequest::Input {
            workspace_id: super::api_workspace(&self.workspace),
            id: TerminalId::new(self.id.clone()),
            input: input.clone(),
        })
        .await
    }

    pub async fn resize(&mut self, size: TerminalSize) -> Result<TerminalScreen, String> {
        self.ensure_open()?;
        // 先にサイズを検証し、拒否された要求で Host を変更しない。
        if size.columns == 0 || size.rows == 0 || size.columns > 512 || size.rows > 256 {
            return Err("Terminal のサイズが不正です".into());
        }
        self.complete(TerminalRequest::Resize {
            workspace_id: super::api_workspace(&self.workspace),
            id: TerminalId::new(self.id.clone()),
            columns: size.columns,
            rows: size.rows,
        })
        .await?;
        Ok(self.stream.screen().clone())
    }

    pub async fn claim(&mut self) -> Result<(), String> {
        self.ensure_open()?;
        self.complete(TerminalRequest::Claim {
            workspace_id: super::api_workspace(&self.workspace),
            id: TerminalId::new(self.id.clone()),
        })
        .await?;
        let owner = self
            .connection
            .session_id()
            .map(|session| session.as_str().to_owned());
        self.stream.set_owner(owner);
        Ok(())
    }

    pub async fn release(&mut self) -> Result<(), String> {
        self.ensure_open()?;
        self.complete(TerminalRequest::Release {
            workspace_id: super::api_workspace(&self.workspace),
            id: TerminalId::new(self.id.clone()),
        })
        .await?;
        self.stream.set_owner(None);
        Ok(())
    }

    pub async fn close(&mut self) -> Result<(), String> {
        self.ensure_open()?;
        self.complete(TerminalRequest::Terminate {
            workspace_id: super::api_workspace(&self.workspace),
            id: TerminalId::new(self.id.clone()),
        })
        .await?;
        self.closed = true;
        Ok(())
    }
}
