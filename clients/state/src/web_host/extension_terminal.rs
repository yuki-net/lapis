//! Web の認証済み接続に紐づく Terminal 拡張セッション。

use std::{cell::RefCell, collections::HashMap};
use std::{path::Path, rc::Rc};

use client_core::browser::BrowserConnection;
use editor_core::WorkspaceId;
use futures_channel::mpsc;
use futures_util::{FutureExt, StreamExt};
use terminal::{
    TerminalBackend, TerminalError, TerminalEvent, TerminalId, TerminalInput, TerminalScreen,
    TerminalSize, TerminalSnapshot, TerminalStatus,
};

use super::{BACKEND, SWITCHING, TerminalOutputStream, terminal_operation};
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
}

struct UiRegistry {
    connection: Rc<BrowserConnection>,
    workspace: WorkspaceId,
    entries: HashMap<String, UiEntry>,
    discovered: Vec<TerminalSnapshot>,
    changes: Vec<mpsc::UnboundedSender<()>>,
}

thread_local! {
    static UI_REGISTRY: RefCell<Option<Rc<RefCell<UiRegistry>>>> = const { RefCell::new(None) };
}

/// 既存 UI の同期 port を Web の非同期 Terminal 拡張に接続する。
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
                if let Ok(sessions) = ExtensionTerminal::list().await
                    && ensure_current(&connection, &workspace).is_ok()
                {
                    let found = sessions
                        .into_iter()
                        .map(|session| {
                            TerminalSnapshot::empty(
                                TerminalId::new(session.id),
                                Path::new(super::ROOT).join(session.working_directory),
                                TerminalStatus::Running,
                                TerminalSize::new(session.columns, session.rows),
                            )
                        })
                        .collect::<Vec<_>>();
                    let mut state = active.borrow_mut();
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
        },
    );
    let state = registry.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let result = if attach {
            ExtensionTerminal::attach(id.as_str()).await
        } else {
            ExtensionTerminal::start_with_id(&path, size, None, Some(id.as_str().into())).await
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
        create_actor(id.clone(), cwd, TerminalSize::new(columns, rows), false)?;
        Ok(id)
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
        create_actor(id.clone(), cwd, size, true)
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
    owner: Option<String>,
    stream: TerminalOutputStream,
    renderer: terminal_portable_renderer::Renderer,
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

async fn call<T: serde::Serialize, R: serde::de::DeserializeOwned>(
    connection: &Rc<BrowserConnection>,
    workspace: &WorkspaceId,
    operation: &str,
    input: &T,
) -> Result<R, String> {
    ensure_current(connection, workspace)?;
    let result = terminal_operation(operation, input).await;
    ensure_current(connection, workspace)?;
    result
}

impl ExtensionTerminal {
    pub async fn list() -> Result<Vec<v1::Session>, String> {
        let (connection, workspace) = connected()?;
        let response: v1::ListResponse =
            call(&connection, &workspace, v1::LIST, &v1::ListRequest {}).await?;
        validate_terminal_sessions(&response.sessions)?;
        Ok(response.sessions)
    }

    pub async fn attach(id: &str) -> Result<Self, String> {
        let (connection, workspace) = connected()?;
        let session = Self::list()
            .await?
            .into_iter()
            .find(|session| session.id == id)
            .ok_or("Terminal session がありません")?;
        ensure_current(&connection, &workspace)?;
        let info: v1::InfoResponse =
            call(&connection, &workspace, v1::INFO, &v1::InfoRequest {}).await?;
        let renderer = terminal_portable_renderer::Renderer::with_conpty(
            TerminalSize::new(session.columns, session.rows),
            info.pty == v1::PtyFlavor::Conpty,
        )?;
        let attached: v1::OwnerResponse = call(
            &connection,
            &workspace,
            v1::ATTACH,
            &v1::IdRequest { id: id.into() },
        )
        .await?;
        let stream = TerminalOutputStream::new(id.into())?;
        ensure_current(&connection, &workspace)?;
        Ok(Self {
            connection,
            workspace,
            id: id.into(),
            cwd: session.working_directory,
            owner: attached.owner,
            stream,
            renderer,
            closed: false,
        })
    }

    pub async fn start(
        path: &Path,
        size: TerminalSize,
        shell: Option<v1::ShellCommand>,
    ) -> Result<Self, String> {
        Self::start_with_id(path, size, shell, None).await
    }

    pub async fn start_with_id(
        path: &Path,
        size: TerminalSize,
        shell: Option<v1::ShellCommand>,
        requested_id: Option<String>,
    ) -> Result<Self, String> {
        let cwd = terminal_relative_cwd(path)?;
        let (connection, workspace) = connected()?;
        let info: v1::InfoResponse =
            call(&connection, &workspace, v1::INFO, &v1::InfoRequest {}).await?;
        let renderer = terminal_portable_renderer::Renderer::with_conpty(
            size,
            info.pty == v1::PtyFlavor::Conpty,
        )?;
        let shell = shell.unwrap_or(info.default_shell);
        let started: v1::StartResponse = call(
            &connection,
            &workspace,
            v1::START,
            &v1::StartRequest {
                requested_id: requested_id.clone(),
                program: shell.program,
                arguments: shell.arguments,
                environment: shell.environment,
                working_directory: cwd.clone(),
                columns: size.columns,
                rows: size.rows,
            },
        )
        .await?;
        if started.id.is_empty()
            || started.id.len() > 256
            || requested_id.as_ref().is_some_and(|id| id != &started.id)
        {
            return Err("Terminal session ID が不正です".into());
        }
        let stream = TerminalOutputStream::new(started.id.clone())?;
        ensure_current(&connection, &workspace)?;
        Ok(Self {
            connection,
            workspace,
            id: started.id,
            cwd,
            owner: started.owner,
            stream,
            renderer,
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
        self.owner.as_deref()
    }

    fn ensure_open(&self) -> Result<(), String> {
        if self.closed {
            return Err("Terminal は終了しています".into());
        }
        ensure_current(&self.connection, &self.workspace)
    }

    pub async fn read(&mut self, timeout_ms: u32) -> Result<ExtensionTerminalUpdate, String> {
        self.ensure_open()?;
        let batch = self.stream.read(v1::MAX_READ_BYTES, timeout_ms).await?;
        if let Err(error) = self.ensure_open() {
            self.stream.discard(batch)?;
            return Err(error);
        }
        let mut result = None;
        self.stream.apply(batch, |output| {
            let reply = self.renderer.process(&output.bytes);
            result = Some((self.renderer.snapshot(), reply, output.clone()));
            Ok(())
        })?;
        let (screen, reply, output) = result.expect("successful apply supplies output");
        self.owner = output.owner.clone();
        let session = self.connection.session_id();
        let is_owner = session
            .as_ref()
            .is_some_and(|session| output.owner.as_deref() == Some(session.as_str()));
        let reply_error = if is_owner && !reply.is_empty() {
            self.write_bytes(&reply).await.err()
        } else {
            None
        };
        Ok(ExtensionTerminalUpdate {
            screen,
            owner: output.owner,
            exit: output.exit,
            read_error: output.read_error,
            write_error: output.write_error,
            reply_error,
        })
    }

    async fn write_bytes(&self, bytes: &[u8]) -> Result<(), String> {
        self.ensure_open()?;
        for chunk in bytes.chunks(v1::MAX_READ_BYTES as usize) {
            call::<_, ()>(
                &self.connection,
                &self.workspace,
                v1::WRITE,
                &v1::WriteRequest {
                    id: self.id.clone(),
                    bytes: chunk.to_vec(),
                },
            )
            .await?;
        }
        Ok(())
    }

    pub async fn input(&self, input: &TerminalInput) -> Result<(), String> {
        self.ensure_open()?;
        self.write_bytes(&self.renderer.input(input)?).await
    }

    pub async fn resize(&mut self, size: TerminalSize) -> Result<TerminalScreen, String> {
        self.ensure_open()?;
        // 先にサイズを検証し、拒否された要求で Host を変更しない。
        if size.columns == 0 || size.rows == 0 || size.columns > 512 || size.rows > 256 {
            return Err("Terminal のサイズが不正です".into());
        }
        call::<_, ()>(
            &self.connection,
            &self.workspace,
            v1::RESIZE,
            &v1::ResizeRequest {
                id: self.id.clone(),
                columns: size.columns,
                rows: size.rows,
            },
        )
        .await?;
        self.renderer.resize(size)?;
        Ok(self.renderer.snapshot())
    }

    async fn owner_operation(&mut self, operation: &str) -> Result<(), String> {
        self.ensure_open()?;
        let response: v1::OwnerResponse = call(
            &self.connection,
            &self.workspace,
            operation,
            &v1::IdRequest {
                id: self.id.clone(),
            },
        )
        .await?;
        self.owner = response.owner;
        Ok(())
    }
    pub async fn claim(&mut self) -> Result<(), String> {
        self.owner_operation(v1::CLAIM).await
    }
    pub async fn release(&mut self) -> Result<(), String> {
        self.owner_operation(v1::RELEASE).await
    }
    pub async fn detach(&mut self) -> Result<(), String> {
        self.owner_operation(v1::DETACH).await?;
        self.closed = true;
        Ok(())
    }
    pub async fn close(&mut self) -> Result<(), String> {
        self.ensure_open()?;
        call::<_, ()>(
            &self.connection,
            &self.workspace,
            v1::CLOSE,
            &v1::IdRequest {
                id: self.id.clone(),
            },
        )
        .await?;
        self.closed = true;
        Ok(())
    }
}
