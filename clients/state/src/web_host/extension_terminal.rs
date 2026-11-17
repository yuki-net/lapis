//! Web の認証済み接続に紐づく Terminal 拡張セッション。

use std::{path::Path, rc::Rc};

use client_core::browser::BrowserConnection;
use editor_core::WorkspaceId;
use terminal::{TerminalInput, TerminalScreen, TerminalSize};

use super::{BACKEND, SWITCHING, TerminalOutputStream, terminal_operation};
use crate::{
    terminal_extension::v1,
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
                program: shell.program,
                arguments: shell.arguments,
                environment: shell.environment,
                working_directory: cwd.clone(),
                columns: size.columns,
                rows: size.rows,
            },
        )
        .await?;
        if started.id.is_empty() || started.id.len() > 256 {
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
