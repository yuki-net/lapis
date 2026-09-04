use std::sync::Arc;
use std::time::Duration;

use lapis_app_services::{
    ConversationSession, EditorSession, GitSession, LspSession, SettingsSession, TaskSession,
    TerminalSession, WorkspaceSearchSession,
};
use lapis_backend_state::{BackendService, BackendState, WorkspaceRegistration};
use lapis_client_api::{SessionId, WorkspaceId};
use lapis_persistence::{LocalConversationRepository, LocalGlobalSettingsRepository};
use lapis_platform::{
    BackendClient, ConnectionGate, LocalGitBackend, LocalLspBackend, LocalTaskBackend,
    LocalTerminalBackend, LocalWorkspaceRepository, LocalWorkspaceSearchBackend,
    LocalWorkspaceStateRepository, LoopbackBackend, NativeWorkspaceDialog, run_task_worker,
};
use lapis_remote::{
    AuthConfig, AuthPolicy, BackendRemoteHandler, CredentialLifetime, PairingLifetime, RemoteAuth,
    RemoteLimits, RemoteServer, RemoteServerConfig, Tls13ServerConfig,
};
use lapis_workspace::WorkspaceStateRepository;

fn main() {
    let arguments = std::env::args_os().collect::<Vec<_>>();
    if arguments.get(1).map(std::ffi::OsString::as_os_str)
        == Some(std::ffi::OsStr::new("--task-worker"))
    {
        let Some(spec_path) = arguments.get(2) else {
            eprintln!("Task worker spec がありません");
            std::process::exit(2);
        };
        if let Err(error) = run_task_worker(std::path::Path::new(&spec_path)) {
            eprintln!("Task worker failed: {error}");
            std::process::exit(1);
        }
        return;
    }
    if arguments.get(1).map(std::ffi::OsString::as_os_str)
        == Some(std::ffi::OsStr::new("--task-smoke"))
    {
        run_task_smoke(&arguments);
        return;
    }
    let use_loopback = arguments.iter().any(|value| value == "--loopback");
    let connection = ConnectionGate::connected();
    let local_files = Arc::new(LocalWorkspaceRepository);
    let local_terminal = Arc::new(LocalTerminalBackend::default());
    let state_repository = Arc::new(LocalWorkspaceStateRepository::user_default());
    let workspace_root = arguments
        .iter()
        .position(|value| value == "--workspace")
        .and_then(|index| arguments.get(index + 1).map(std::path::PathBuf::from))
        .or_else(|| {
            state_repository
                .load()
                .ok()
                .flatten()
                .and_then(|snapshot| snapshot.root)
        });
    let backend_setup = workspace_root.and_then(|root| {
        let workspace_id =
            WorkspaceId::try_new("workspace-default").expect("default workspace id must be valid");
        let registration = WorkspaceRegistration::new(
            workspace_id.clone(),
            root.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("Workspace"),
            root.clone(),
            local_files.clone(),
        )
        .with_terminal(local_terminal.clone());
        let state = match BackendState::new([registration]) {
            Ok(state) => state,
            Err(error) => {
                eprintln!("Backend state setup failed: {error}");
                return None;
            }
        };
        let service = match BackendService::start(state) {
            Ok(service) => service,
            Err(error) => {
                eprintln!("Backend service setup failed: {error}");
                return None;
            }
        };
        let client = match BackendClient::connect(
            service.clone(),
            SessionId::try_new("desktop-local").expect("local session id must be valid"),
            workspace_id,
            root,
            local_files.clone(),
        ) {
            Ok(client) => client,
            Err(error) => {
                eprintln!("Local backend client setup failed: {error}");
                return None;
            }
        };
        Some((service, client))
    });
    let workspace_repository: Arc<dyn lapis_app_services::WorkspaceRepository> =
        if let Some((_, client)) = &backend_setup {
            Arc::new(client.workspace_repository())
        } else if use_loopback {
            Arc::new(LoopbackBackend::new(
                local_files.clone(),
                connection.clone(),
            ))
        } else {
            local_files.clone()
        };
    let mut session = EditorSession::new(
        workspace_repository,
        Arc::new(NativeWorkspaceDialog),
        state_repository,
    );
    if let Some(index) = arguments.iter().position(|value| value == "--workspace") {
        let Some(root) = arguments.get(index + 1).map(std::path::PathBuf::from) else {
            eprintln!("Usage: lapis --workspace <directory>");
            std::process::exit(2);
        };
        let root = root.canonicalize().unwrap_or(root);
        if let Err(error) = session.open_workspace(root) {
            eprintln!("Workspace open failed: {error}");
            std::process::exit(1);
        }
    }
    if let Some(index) = arguments.iter().position(|value| value == "--open-file") {
        let Some(path) = arguments.get(index + 1).map(std::path::PathBuf::from) else {
            eprintln!("Usage: lapis --open-file <path>");
            std::process::exit(2);
        };
        let path = if path.is_absolute() {
            path
        } else {
            session
                .workspace_root()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join(path)
        };
        if let Err(error) = session.open_path(path) {
            eprintln!("File open failed: {error}");
            std::process::exit(1);
        }
    }
    let conversation_session = ConversationSession::new(
        Arc::new(LocalConversationRepository::user_default()),
        session.snapshot(),
    );
    let settings_session =
        SettingsSession::load(Arc::new(LocalGlobalSettingsRepository::user_default()))
            .unwrap_or_else(|error| {
                eprintln!("Global settings load failed: {error}");
                SettingsSession::load(Arc::new(LocalGlobalSettingsRepository::new(
                    std::env::temp_dir().join("lapis-settings-fallback.json"),
                )))
                .expect("temporary global settings must be available")
            });
    if let Err(error) = conversation_session.restore_matching_workspace(&mut session) {
        eprintln!("Conversation restore failed: {error}");
    }
    let task_session = if use_loopback {
        TaskSession::new(Arc::new(LoopbackBackend::new(
            Arc::new(LocalTaskBackend::user_default()),
            connection.clone(),
        )))
    } else {
        TaskSession::new(Arc::new(LocalTaskBackend::user_default()))
    };
    let git_session = if use_loopback {
        GitSession::new(Arc::new(LoopbackBackend::new(
            Arc::new(LocalGitBackend::user_default()),
            connection.clone(),
        )))
    } else {
        GitSession::new(Arc::new(LocalGitBackend::user_default()))
    };
    let lsp_session = if use_loopback {
        LspSession::new(Arc::new(LoopbackBackend::new(
            Arc::new(LocalLspBackend::default()),
            connection.clone(),
        )))
    } else {
        LspSession::new(Arc::new(LocalLspBackend::default()))
    };
    let mut terminal_session = if let Some((_, client)) = &backend_setup {
        TerminalSession::new(Arc::new(client.terminal_backend()))
    } else if use_loopback {
        TerminalSession::new(Arc::new(LoopbackBackend::new(
            local_terminal.clone(),
            connection.clone(),
        )))
    } else {
        TerminalSession::new(local_terminal.clone())
    };
    let search_session = if use_loopback {
        WorkspaceSearchSession::new(Arc::new(LoopbackBackend::new(
            Arc::new(LocalWorkspaceSearchBackend),
            connection,
        )))
    } else {
        WorkspaceSearchSession::new(Arc::new(LocalWorkspaceSearchBackend))
    };

    let mut _remote_server: Option<RemoteServer> = None;
    if let Some((backend_service, _)) = &backend_setup {
        let auth_config = AuthConfig::new(
            PairingLifetime::new(300).expect("valid pairing lifetime"),
            CredentialLifetime::new(86400).expect("valid credential lifetime"),
        );
        let mut auth = RemoteAuth::system(auth_config, AuthPolicy::new(Default::default()));
        auth.enable();
        let shared_auth = Arc::new(std::sync::Mutex::new(auth));
        let handler = Arc::new(BackendRemoteHandler::new(backend_service.clone()));
        if let Ok(tls) = Tls13ServerConfig::generate_self_signed(vec![
            "localhost".to_owned(),
            "127.0.0.1".to_owned(),
        ]) {
            let server_config = RemoteServerConfig::new(
                "127.0.0.1:0"
                    .parse()
                    .expect("valid loopback socket address"),
                tls,
                RemoteLimits::default(),
            );
            if let Ok(server) = RemoteServer::start(server_config, shared_auth, handler) {
                _remote_server = Some(server);
            }
        }
    }

    let show_terminal = arguments.iter().any(|value| value == "--show-terminal");
    if show_terminal
        && let Some(root) = session.workspace_root()
        && let Err(error) = terminal_session.start(root, 120, 30)
    {
        eprintln!("Terminal start failed: {error}");
    }
    let initial_view = lapis_desktop_ui::InitialView {
        empty_window: false,
        show_tasks: arguments.iter().any(|value| value == "--show-tasks"),
        show_terminal,
        show_problems: arguments.iter().any(|value| value == "--show-problems"),
    };
    let desktop_services = lapis_desktop_ui::DesktopServices::new(
        task_session,
        git_session,
        lsp_session,
        terminal_session,
        search_session,
        conversation_session,
        settings_session,
    );
    lapis_desktop_ui::run(session, desktop_services, initial_view);
}

fn run_task_smoke(arguments: &[std::ffi::OsString]) {
    let Some(workspace_root) = arguments.get(2).map(std::path::PathBuf::from) else {
        eprintln!("Usage: lapis --task-smoke <workspace> [prompt]");
        std::process::exit(2);
    };
    let prompt = arguments
        .get(3)
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "ファイルを変更せず、Lapis とだけ回答してください。".to_owned());
    let auto_interact = arguments.iter().any(|value| value == "--auto-interact");
    let cancel_on_running = arguments.iter().any(|value| value == "--cancel-on-running");
    let mode = if arguments.iter().any(|value| value == "--plan") {
        lapis_task_runner::TaskMode::Plan
    } else {
        lapis_task_runner::TaskMode::Default
    };
    let mut session = TaskSession::new(Arc::new(LocalTaskBackend::user_default()));
    let execution_id = match session.start_codex_with_mode(workspace_root, prompt, mode) {
        Ok(id) => id,
        Err(error) => {
            eprintln!("Task start failed: {error}");
            std::process::exit(1);
        }
    };
    println!("execution={execution_id}");
    let mut last_sequence = 0;
    let mut last_controlled_status = None;
    for _ in 0..900 {
        std::thread::sleep(Duration::from_millis(200));
        if let Err(error) = session.refresh() {
            eprintln!("Task refresh failed: {error}");
            std::process::exit(1);
        }
        let Some(record) = session
            .records()
            .iter()
            .find(|record| record.execution.id == execution_id)
        else {
            continue;
        };
        for event in &record.events {
            if event.sequence <= last_sequence {
                continue;
            }
            println!("{} {}", event.sequence, event.event.display_text());
            last_sequence = event.sequence;
        }
        let execution_status = record.execution.status;
        if (auto_interact || cancel_on_running) && last_controlled_status != Some(execution_status)
        {
            let control = match execution_status {
                lapis_task_runner::ExecutionStatus::Running if cancel_on_running => {
                    Some(lapis_task_runner::TaskControl::Cancel)
                }
                lapis_task_runner::ExecutionStatus::WaitingForInput => {
                    auto_interact.then(|| lapis_task_runner::TaskControl::Reply {
                        text: "青".to_owned(),
                    })
                }
                lapis_task_runner::ExecutionStatus::WaitingForApproval => {
                    auto_interact.then_some(lapis_task_runner::TaskControl::Decline)
                }
                _ => None,
            };
            if let Some(control) = control {
                if let Err(error) = session.control(&execution_id, control) {
                    eprintln!("Task control failed: {error}");
                    std::process::exit(1);
                }
                last_controlled_status = Some(execution_status);
            }
        }
        if record.execution.status.is_terminal() {
            println!("status={}", record.execution.status.label());
            let expected = if cancel_on_running {
                lapis_task_runner::ExecutionStatus::Cancelled
            } else {
                lapis_task_runner::ExecutionStatus::Succeeded
            };
            if record.execution.status != expected {
                std::process::exit(1);
            }
            return;
        }
    }
    eprintln!("Task smoke timed out");
    std::process::exit(1);
}
