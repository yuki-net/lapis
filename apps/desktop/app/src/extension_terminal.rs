//! Workspace ごとの拡張接続を Desktop のターミナル画面へつなぐ。

use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use client_api::WorkspaceId;
use desktop_client::terminal_extension::{NativeBackend, NativeTransport};
use host::BackendClient;
use terminal::{
    TerminalBackend, TerminalError, TerminalEvent, TerminalId, TerminalInput, TerminalProfileId,
    TerminalProfiles, TerminalSnapshot,
};

pub struct ExtensionTerminals {
    host: BackendClient,
    workspaces: Mutex<HashMap<WorkspaceId, Arc<NativeBackend>>>,
    sessions: Mutex<HashMap<String, Arc<NativeBackend>>>,
    count: Arc<AtomicUsize>,
    pending: Arc<AtomicBool>,
    changes: Mutex<Option<futures_channel::mpsc::UnboundedReceiver<()>>>,
    _lifetime: Arc<()>,
}

impl ExtensionTerminals {
    pub fn new(host: BackendClient) -> Self {
        let count = Arc::new(AtomicUsize::new(0));
        let pending = Arc::new(AtomicBool::new(false));
        let lifetime = Arc::new(());
        let weak = Arc::downgrade(&lifetime);
        let thread_count = Arc::clone(&count);
        let thread_pending = Arc::clone(&pending);
        let (sender, receiver) = futures_channel::mpsc::unbounded();
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
            host,
            workspaces: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            count,
            pending,
            changes: Mutex::new(Some(receiver)),
            _lifetime: lifetime,
        }
    }

    fn workspace(&self, cwd: &Path) -> Result<Arc<NativeBackend>, TerminalError> {
        let (root, workspace) = self.host.workspace_for_directory(cwd).map_err(error)?;
        let mut entries = self.workspaces.lock().map_err(error)?;
        if let Some(backend) = entries.get(&workspace) {
            return Ok(Arc::clone(backend));
        }
        let connection = self
            .host
            .open_extension_connection(workspace.clone())
            .map_err(error)?;
        let transport: NativeTransport = Arc::new(move |request| {
            connection
                .dispatch(request)
                .map_err(|error| error.to_string())
        });
        let backend = Arc::new(NativeBackend::for_workspace(transport, root));
        entries.insert(workspace, Arc::clone(&backend));
        Ok(backend)
    }

    fn remember(&self, id: &TerminalId, backend: &Arc<NativeBackend>) -> Result<(), TerminalError> {
        let mut sessions = self.sessions.lock().map_err(error)?;
        if let Some(existing) = sessions.get(id.as_str()) {
            if !Arc::ptr_eq(existing, backend) {
                return Err(TerminalError::new("Terminal session ID collision"));
            }
        } else {
            sessions.insert(id.as_str().to_owned(), Arc::clone(backend));
            self.count.fetch_add(1, Ordering::Release);
        }
        Ok(())
    }

    fn session(&self, id: &TerminalId) -> Result<Arc<NativeBackend>, TerminalError> {
        self.sessions
            .lock()
            .map_err(error)?
            .get(id.as_str())
            .cloned()
            .ok_or_else(|| TerminalError::new("Terminal session not attached"))
    }
}

fn error(value: impl std::fmt::Display) -> TerminalError {
    TerminalError::new(value.to_string())
}

impl TerminalBackend for ExtensionTerminals {
    fn start(&self, cwd: &Path, columns: u16, rows: u16) -> Result<TerminalId, TerminalError> {
        let backend = self.workspace(cwd)?;
        let id = backend.start(cwd, columns, rows)?;
        self.remember(&id, &backend)?;
        Ok(id)
    }

    fn start_profile(
        &self,
        cwd: &Path,
        columns: u16,
        rows: u16,
        profile: Option<&TerminalProfileId>,
    ) -> Result<TerminalId, TerminalError> {
        let backend = self.workspace(cwd)?;
        let id = backend.start_profile(cwd, columns, rows, profile)?;
        self.remember(&id, &backend)?;
        Ok(id)
    }

    fn profiles(&self, cwd: &Path) -> Result<TerminalProfiles, TerminalError> {
        self.workspace(cwd)?.profiles(cwd)
    }

    fn list(&self, cwd: &Path) -> Result<Vec<TerminalSnapshot>, TerminalError> {
        let backend = self.workspace(cwd)?;
        let sessions = backend.list(cwd)?;
        for session in &sessions {
            self.remember(&session.id, &backend)?;
        }
        Ok(sessions)
    }

    fn attach(&self, cwd: &Path, id: &TerminalId) -> Result<TerminalSnapshot, TerminalError> {
        let backend = self.workspace(cwd)?;
        let snapshot = backend.attach(cwd, id)?;
        self.remember(id, &backend)?;
        Ok(snapshot)
    }

    fn input(&self, id: &TerminalId, input: &TerminalInput) -> Result<(), TerminalError> {
        self.session(id)?.input(id, input)
    }
    fn resize(&self, id: &TerminalId, columns: u16, rows: u16) -> Result<(), TerminalError> {
        self.session(id)?.resize(id, columns, rows)
    }
    fn poll(&self, id: &TerminalId) -> Result<Vec<TerminalEvent>, TerminalError> {
        self.pending.store(false, Ordering::Release);
        self.session(id)?.poll(id)
    }
    fn terminate(&self, id: &TerminalId) -> Result<(), TerminalError> {
        self.session(id)?.terminate(id)?;
        if self
            .sessions
            .lock()
            .map_err(error)?
            .remove(id.as_str())
            .is_some()
        {
            self.count.fetch_sub(1, Ordering::Release);
        }
        Ok(())
    }
    fn claim(&self, id: &TerminalId) -> Result<(), TerminalError> {
        self.session(id)?.claim(id)
    }
    fn release(&self, id: &TerminalId) -> Result<(), TerminalError> {
        self.session(id)?.release(id)
    }
    fn changes(&self) -> Option<futures_channel::mpsc::UnboundedReceiver<()>> {
        self.changes.lock().ok()?.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspaces_reuse_connections_and_keep_session_routes_separate() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let nested = first.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        let host = host::BackendHost::start().unwrap();
        host.register_workspace(first.path()).unwrap();
        host.register_workspace(second.path()).unwrap();
        let backend = ExtensionTerminals::new(BackendClient::from(host));
        let one = backend.workspace(first.path()).unwrap();
        let nested_backend = backend.workspace(&nested).unwrap();
        let two = backend.workspace(second.path()).unwrap();
        assert!(Arc::ptr_eq(&one, &nested_backend));
        assert!(!Arc::ptr_eq(&one, &two));
        let id = TerminalId::new("session-one");
        backend.remember(&id, &one).unwrap();
        backend.remember(&id, &one).unwrap();
        assert!(Arc::ptr_eq(&backend.session(&id).unwrap(), &one));
        assert!(backend.remember(&id, &two).is_err());
        assert_eq!(backend.count.load(Ordering::Acquire), 1);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "Wasm component と拡張ホストの事前ビルドが必要"]
    fn desktop_terminal_runs_real_extension_pty() {
        use client_api::HostExtensionSetCapabilityRequest;
        use desktop_client::TerminalSession;
        use std::{
            process::{Command, Stdio},
            time::Instant,
        };

        const CHILD: &str = "LAPIS_TERMINAL_INTEGRATION_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let temp = tempfile::tempdir().unwrap();
            let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
            let executable = std::env::current_exe().unwrap();
            let copied_test = temp.path().join("desktop-test.exe");
            std::fs::copy(executable, &copied_test).unwrap();
            std::fs::copy(
                root.join("target/debug/lapis-extension-host.exe"),
                temp.path().join("lapis-extension-host.exe"),
            )
            .unwrap();
            let mut child = Command::new(copied_test)
                .args([
                    "extension_terminal::tests::desktop_terminal_runs_real_extension_pty",
                    "--exact",
                    "--ignored",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(CHILD, "1")
                .env_remove("RUST_TEST_NOCAPTURE")
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap();
            // 子テストの起動・出力・色付き出力の各待機と初期化時間を含める。
            let deadline = Instant::now() + Duration::from_secs(120);
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() {
                    break status;
                }
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("desktop extension PTY child test did not finish");
                }
                std::thread::sleep(Duration::from_millis(100));
            };
            assert!(status.success(), "desktop extension PTY child test failed");
            return;
        }

        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let state = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let input = tempfile::tempdir().unwrap();
        std::fs::copy(
            root.join("extensions/terminal/component/extension.json"),
            input.path().join("extension.json"),
        )
        .unwrap();
        std::fs::copy(
            root.join("extensions/terminal/component/contributions.json"),
            input.path().join("contributions.json"),
        )
        .unwrap();
        std::fs::copy(
            root.join("extensions/terminal/component/views.json"),
            input.path().join("views.json"),
        )
        .unwrap();
        std::fs::copy(
            root.join(
                "target/terminal-component/wasm32-wasip2/debug/lapis_terminal_component.wasm",
            ),
            input.path().join("extension.wasm"),
        )
        .unwrap();
        let package = state.path().join("terminal.lapis");
        assert!(
            Command::new("python")
                .current_dir(&root)
                .args(["-m", "scripts.extensions.package"])
                .arg(input.path())
                .arg(&package)
                .arg("--development")
                .status()
                .unwrap()
                .success()
        );
        let host =
            host::BackendHost::start_development_with_state_directory(state.path().to_owned())
                .unwrap();
        host.install_extension(&package, true).unwrap();
        let digest = host
            .extension_capabilities("lapis.terminal")
            .unwrap()
            .manifest_digest;
        host.set_extension_capability(&HostExtensionSetCapabilityRequest {
            id: "lapis.terminal".into(),
            manifest_digest: digest,
            capability: "process.pty".into(),
            version: "1.0.0".into(),
            approved: true,
        })
        .unwrap();
        drop(host);
        let host =
            host::BackendHost::start_development_with_state_directory(state.path().to_owned())
                .unwrap();
        let workspace_id = host.register_workspace(workspace.path()).unwrap();
        let backend = Arc::new(ExtensionTerminals::new(BackendClient::from(host.clone())));
        let mut terminals = TerminalSession::new(backend);
        let start_deadline = Instant::now() + Duration::from_secs(20);
        let id = loop {
            match terminals.start(workspace.path(), 80, 24) {
                Ok(id) => break id,
                Err(error) => {
                    assert!(
                        Instant::now() < start_deadline,
                        "terminal startup failed: {error}"
                    );
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        };
        let profiles = terminals.profiles(workspace.path()).unwrap();
        assert_eq!(
            profiles.default.as_ref().map(|id| id.as_str()),
            Some("powershell")
        );
        assert!(
            profiles
                .profiles
                .iter()
                .any(|profile| profile.id.as_str() == "cmd")
        );
        assert!(
            terminals
                .start_profile(
                    workspace.path(),
                    80,
                    24,
                    Some(terminal::TerminalProfileId::new("unknown"))
                )
                .is_err()
        );
        let command = "[console]::WriteLine('lapis'+'proof')\r";
        terminals.input(&id, command).unwrap();
        let output_deadline = Instant::now() + Duration::from_secs(20);
        let mut last_input = Instant::now();
        while !terminals.terminals()[0]
            .screen
            .plain_text()
            .contains("lapisproof")
        {
            assert!(Instant::now() < output_deadline, "terminal output missing");
            // shell の起動が遅い環境では先頭の入力が失われるため、未出力なら再送する。
            if last_input.elapsed() >= Duration::from_secs(2) {
                terminals.input(&id, command).unwrap();
                last_input = Instant::now();
            }
            terminals.refresh().unwrap();
            std::thread::sleep(Duration::from_millis(50));
        }
        terminals.input(&id, "[Console]::OutputEncoding=[Text.Encoding]::UTF8;[Console]::WriteLine(('{0}[31m{1}{0}[0m' -f [char]27,(([string][char]0x65e5)+[char]0x672c)))\r").unwrap();
        let color_deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let screen = &terminals.terminals()[0].screen;
            let colored_japanese = screen.rows.iter().any(|row| {
                row.cells.windows(2).any(|cells| {
                    cells[0].text == "日"
                        && cells[0].foreground == terminal::TerminalColor::Indexed(1)
                })
            });
            if colored_japanese {
                break;
            }
            assert!(
                Instant::now() < color_deadline,
                "colored Japanese output missing: {:?}",
                screen.plain_text()
            );
            terminals.refresh().unwrap();
            std::thread::sleep(Duration::from_millis(50));
        }
        let original_owner = terminals.owner(&id).unwrap().to_owned();
        let other = ExtensionTerminals::new(BackendClient::from(host.clone()));
        assert!(
            other
                .list(workspace.path())
                .unwrap()
                .iter()
                .any(|item| item.id == id)
        );
        other.attach(workspace.path(), &id).unwrap();
        other.claim(&id).unwrap();
        terminals.refresh().unwrap();
        assert_ne!(terminals.owner(&id), Some(original_owner.as_str()));
        drop(other);
        let disconnect_deadline = Instant::now() + Duration::from_secs(10);
        loop {
            terminals.refresh().unwrap();
            if terminals.owner(&id).is_none() {
                break;
            }
            assert!(
                Instant::now() < disconnect_deadline,
                "owner was not released"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        terminals.claim(&id).unwrap();
        terminals.resize(&id, 100, 30).unwrap();
        terminals.refresh().unwrap();
        assert_eq!(terminals.terminals()[0].screen.size.columns, 100);
        terminals.terminate_and_forget(&id).unwrap();
        assert!(terminals.terminals().is_empty());
        let cmd = terminals
            .start_profile(
                workspace.path(),
                80,
                24,
                Some(terminal::TerminalProfileId::new("cmd")),
            )
            .unwrap();
        terminals.input(&cmd, "echo lapis-cmd-proof\r").unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while !terminals.terminals()[0]
            .screen
            .plain_text()
            .contains("lapis-cmd-proof")
        {
            assert!(Instant::now() < deadline, "cmd profile output missing");
            terminals.refresh().unwrap();
            std::thread::sleep(Duration::from_millis(50));
        }
        terminals.terminate_and_forget(&cmd).unwrap();

        use desktop_client::terminal_extension::{request, response, v1};
        let connection = BackendClient::from(host.clone())
            .open_extension_connection(workspace_id)
            .unwrap();
        let stable = format!("client-{}", "a".repeat(32));
        let start = v1::StartRequest {
            requested_id: Some(stable.clone()),
            program: "cmd.exe".into(),
            arguments: vec!["/D".into(), "/C".into(), "ping -n 30 127.0.0.1 >nul".into()],
            environment: Default::default(),
            working_directory: ".".into(),
            columns: 80,
            rows: 24,
        };
        let result: v1::StartResponse = response(
            v1::START,
            connection
                .dispatch(request(v1::START, &start).unwrap())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(result.id, stable);
        host.set_extension_enabled("lapis.terminal", false).unwrap();
        let listing = host.extensions().unwrap();
        let terminal = listing
            .extensions
            .iter()
            .find(|entry| entry.id == "lapis.terminal")
            .unwrap();
        assert!(!terminal.enabled);
        assert_eq!(terminal.active, Some(true));
        assert!(
            listing
                .contributions
                .iter()
                .any(|entry| entry.extension_id == "lapis.terminal" && entry.views.is_some())
        );
        let declaration = &listing
            .contributions
            .iter()
            .find(|entry| entry.extension_id == "lapis.terminal")
            .unwrap()
            .declaration;
        for name in [
            "font-family",
            "font-size",
            "line-height-percent",
            "caret-shape",
            "default-profile",
        ] {
            assert!(
                declaration
                    .settings
                    .iter()
                    .any(|setting| setting.definition.name == name)
            );
        }
        // 次回起動までは、既存の接続と実行中セッションをそのまま使える。
        let listed: v1::ListResponse = response(
            v1::LIST,
            connection
                .dispatch(request(v1::LIST, &v1::ListRequest {}).unwrap())
                .unwrap(),
        )
        .unwrap();
        assert!(listed.sessions.iter().any(|session| session.id == stable));
        let end = v1::IdRequest { id: stable };
        let _: () = response(
            v1::CLOSE,
            connection
                .dispatch(request(v1::CLOSE, &end).unwrap())
                .unwrap(),
        )
        .unwrap();
        drop(connection);
        drop(terminals);
        drop(host);

        let disabled =
            host::BackendHost::start_development_with_state_directory(state.path().to_owned())
                .unwrap();
        let listing = disabled.extensions().unwrap();
        let terminal = listing
            .extensions
            .iter()
            .find(|entry| entry.id == "lapis.terminal")
            .unwrap();
        assert!(!terminal.enabled);
        assert_eq!(terminal.active, Some(false));
        assert!(
            listing
                .contributions
                .iter()
                .all(|entry| entry.extension_id != "lapis.terminal")
        );
        disabled.register_workspace(workspace.path()).unwrap();
        let backend = ExtensionTerminals::new(BackendClient::from(disabled.clone()));
        assert!(backend.start(workspace.path(), 80, 24).is_err());
        drop(backend);
        disabled
            .set_extension_enabled("lapis.terminal", true)
            .unwrap();
        drop(disabled);

        let enabled =
            host::BackendHost::start_development_with_state_directory(state.path().to_owned())
                .unwrap();
        enabled.register_workspace(workspace.path()).unwrap();
        let backend = ExtensionTerminals::new(BackendClient::from(enabled.clone()));
        let deadline = Instant::now() + Duration::from_secs(20);
        let id = loop {
            match backend.start(workspace.path(), 80, 24) {
                Ok(id) => break id,
                Err(error) => {
                    assert!(
                        Instant::now() < deadline,
                        "terminal restart failed: {error}"
                    );
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        };
        let listing = enabled.extensions().unwrap();
        assert_eq!(
            listing
                .extensions
                .iter()
                .find(|entry| entry.id == "lapis.terminal")
                .unwrap()
                .active,
            Some(true)
        );
        assert!(
            listing
                .contributions
                .iter()
                .any(|entry| entry.extension_id == "lapis.terminal" && entry.views.is_some())
        );
        backend.terminate(&id).unwrap();
    }
}
