use client_api::*;
use remote::*;
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct HostProcess(Child);
impl Drop for HostProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn hello() -> HelloRequest {
    HelloRequest {
        protocol: ProtocolRange::exact(CURRENT_PROTOCOL),
        client_id: ClientId::try_new("standalone-test").unwrap(),
        client_name: "Test".to_owned(),
        client_kind: ClientKind::Desktop,
        requested_capabilities: host::supported_capabilities(),
    }
}

fn wait_for_exit(process: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = process.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(Instant::now() < deadline, "Host did not shut down");
        std::thread::sleep(Duration::from_millis(20));
    }
}

async fn request(client: &mut QuicClient, id: &str, body: RequestBody) -> ResponseBody {
    let request_id = RequestId::try_new(id).unwrap();
    client
        .send(&ClientMessage::Request(RequestEnvelope {
            request_id: request_id.clone(),
            body,
        }))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match client.recv().await.unwrap() {
                ServerMessage::Event(_) => {}
                ServerMessage::Response(response) => {
                    assert_eq!(response.request_id, request_id);
                    return response.body;
                }
                _ => panic!("unexpected response"),
            }
        }
    })
    .await
    .unwrap()
}

#[test]
fn owner_console_registers_workspace_and_issues_user_pairing() {
    let directory = tempfile::tempdir().unwrap();
    let other = directory.path().join("other workspace");
    std::fs::create_dir(&other).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_lapis-host"));
    command
        .arg("--workspace")
        .arg(directory.path())
        .arg("--certificate")
        .arg(directory.path().join("host.der"))
        // 開発用 Host が 4787 を使っていても衝突しないよう、空きポートを選ばせる。
        .arg("--listen")
        .arg("127.0.0.1:0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut process = HostProcess(command.spawn().unwrap());
    let mut input = process.0.stdin.take().unwrap();
    let mut output = BufReader::new(process.0.stdout.take().unwrap());
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    line.clear();
    output.read_line(&mut line).unwrap();
    writeln!(input, "register {}", other.display()).unwrap();
    input.flush().unwrap();
    line.clear();
    output.read_line(&mut line).unwrap();
    assert!(line.trim().starts_with("workspace=workspace-"));
    writeln!(input, "pair-user").unwrap();
    input.flush().unwrap();
    let mut token = zeroize::Zeroizing::new(String::new());
    output.read_line(&mut token).unwrap();
    assert!(token.starts_with("User session pairing token (1 hour, one use): "));
    assert_eq!(token.trim().rsplit(' ').next().unwrap().len(), 64);
    writeln!(input, "quit").unwrap();
    input.flush().unwrap();
}

#[test]
fn independent_host_keeps_running_after_client_disconnect_and_rejects_certificate_overwrite() {
    let directory = tempfile::tempdir().unwrap();
    let certificate = directory.path().join("host.der");
    let mut command = Command::new(env!("CARGO_BIN_EXE_lapis-host"));
    command
        .arg("--workspace")
        .arg(directory.path())
        .arg("--certificate")
        .arg(&certificate)
        .arg("--listen")
        .arg("127.0.0.1:0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut process = HostProcess(command.spawn().unwrap());
    let mut input = process.0.stdin.take().unwrap();
    let mut output = BufReader::new(process.0.stdout.take().unwrap());
    let mut startup = String::new();
    output.read_line(&mut startup).unwrap();
    let address = startup
        .split_whitespace()
        .find_map(|value| value.strip_prefix("address="))
        .unwrap()
        .parse()
        .unwrap();
    let workspace = WorkspaceId::try_new(
        startup
            .split_whitespace()
            .find_map(|value| value.strip_prefix("workspace="))
            .unwrap(),
    )
    .unwrap();
    let mut commands = String::new();
    output.read_line(&mut commands).unwrap();
    writeln!(input, "pair").unwrap();
    input.flush().unwrap();
    let mut invitation = zeroize::Zeroizing::new(String::new());
    output.read_line(&mut invitation).unwrap();
    let hex = invitation.trim().rsplit(' ').next().unwrap();
    let mut token = zeroize::Zeroizing::new([0u8; 32]);
    assert_eq!(hex.len(), 64);
    for (index, byte) in token.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).unwrap();
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut client = QuicClient::connect(
            address,
            "lapis-host",
            std::fs::read(&certificate).unwrap().into(),
            RemoteLimits::default(),
        )
        .await
        .unwrap();
        client
            .send(&ClientMessage::Pair(PairRequest::new(hello(), &token)))
            .await
            .unwrap();
        let ServerMessage::Paired(mut paired) =
            tokio::time::timeout(Duration::from_secs(5), client.recv())
                .await
                .unwrap()
                .unwrap()
        else {
            panic!("pairing failed")
        };
        assert!(matches!(
            request(
                &mut client,
                "connect",
                RequestBody::WorkspaceConnect(WorkspaceConnectRequest {
                    workspace_id: workspace.clone(),
                })
            )
            .await,
            ResponseBody::WorkspaceConnect(_)
        ));
        let ResponseBody::TerminalStart(started) = request(
            &mut client,
            "start",
            RequestBody::TerminalStart(TerminalStartRequest {
                workspace_id: workspace.clone(),
                cwd: None,
                command: None,
                profile: None,
                size: TerminalSize {
                    columns: 80,
                    rows: 24,
                },
            }),
        )
        .await
        else {
            panic!("PTY start failed")
        };
        drop(client);
        let mut reconnected = QuicClient::connect(
            address,
            "lapis-host",
            std::fs::read(&certificate).unwrap().into(),
            RemoteLimits::default(),
        )
        .await
        .unwrap();
        let credential = paired.credential_id().unwrap();
        let secret = zeroize::Zeroizing::new(paired.take_secret().unwrap());
        reconnected
            .send(&ClientMessage::Authenticate(AuthenticateRequest::new(
                hello(),
                workspace.clone(),
                &credential,
                &secret,
            )))
            .await
            .unwrap();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(5), reconnected.recv())
                .await
                .unwrap()
                .unwrap(),
            ServerMessage::Authenticated(_)
        ));
        assert!(matches!(
            request(
                &mut reconnected,
                "connect",
                RequestBody::WorkspaceConnect(WorkspaceConnectRequest {
                    workspace_id: workspace.clone()
                })
            )
            .await,
            ResponseBody::WorkspaceConnect(_)
        ));
        let ResponseBody::SnapshotResync(response) = request(
            &mut reconnected,
            "snapshot",
            RequestBody::SnapshotResync(SnapshotRequest {
                workspace_id: workspace,
                reason: SnapshotReason::Reconnect,
            }),
        )
        .await
        else {
            panic!("snapshot failed")
        };
        assert!(
            response
                .snapshot
                .terminals
                .iter()
                .any(
                    |terminal| terminal.terminal_id == started.terminal.terminal_id
                        && terminal.status == TerminalStatus::Running
                )
        );
        assert!(matches!(
            request(
                &mut reconnected,
                "terminate",
                RequestBody::TerminalTerminate(TerminalTerminateRequest {
                    terminal_id: started.terminal.terminal_id
                })
            )
            .await,
            ResponseBody::TerminalTerminate(_)
        ));
    });
    assert!(process.0.try_wait().unwrap().is_none());
    writeln!(input, "quit").unwrap();
    input.flush().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = process.0.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(Instant::now() < deadline, "Host did not shut down");
        std::thread::sleep(Duration::from_millis(20));
    }
    let result = Command::new(env!("CARGO_BIN_EXE_lapis-host"))
        .arg("--workspace")
        .arg(directory.path())
        .arg("--certificate")
        .arg(&certificate)
        .output()
        .unwrap();
    assert!(!result.status.success());
}

#[test]
fn host_connection_info_matches_the_running_host() {
    let directory = tempfile::tempdir().unwrap();
    let certificate = directory.path().join("host.der");
    let connection = directory.path().join("dev").join("connection.json");
    let invitation = directory.path().join("dev").join("desktop.json");

    // 指定しない起動では接続情報を作らない。
    let mut command = Command::new(env!("CARGO_BIN_EXE_lapis-host"));
    command
        .arg("--workspace")
        .arg(directory.path())
        .arg("--certificate")
        .arg(&certificate)
        .arg("--listen")
        .arg("127.0.0.1:0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut process = HostProcess(command.spawn().unwrap());
    let mut input = process.0.stdin.take().unwrap();
    let mut output = BufReader::new(process.0.stdout.take().unwrap());
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    line.clear();
    output.read_line(&mut line).unwrap();
    assert!(!connection.exists());
    writeln!(input, "quit").unwrap();
    input.flush().unwrap();
    wait_for_exit(&mut process.0);

    let mut command = Command::new(env!("CARGO_BIN_EXE_lapis-host"));
    command
        .arg("--workspace")
        .arg(directory.path())
        .arg("--certificate")
        .arg(&certificate)
        .arg("--replace-certificate")
        .arg("--listen")
        .arg("127.0.0.1:0")
        .arg("--connection-info")
        .arg(&connection)
        .arg("--invitation")
        .arg(&invitation)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut process = HostProcess(command.spawn().unwrap());
    let mut input = process.0.stdin.take().unwrap();
    let mut output = BufReader::new(process.0.stdout.take().unwrap());
    let mut startup = String::new();
    output.read_line(&mut startup).unwrap();
    let address = startup
        .split_whitespace()
        .find_map(|value| value.strip_prefix("address="))
        .unwrap()
        .parse::<std::net::SocketAddr>()
        .unwrap();
    let workspace = startup
        .split_whitespace()
        .find_map(|value| value.strip_prefix("workspace="))
        .unwrap()
        .to_owned();
    let mut commands = String::new();
    output.read_line(&mut commands).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !connection.is_file() {
        assert!(Instant::now() < deadline, "接続情報が書かれません");
        std::thread::sleep(Duration::from_millis(20));
    }

    let info = wire::read_connection(&connection).unwrap();
    assert_eq!(info.version, wire::HOST_CONNECTION_VERSION);
    assert_eq!(info.address, address);
    assert_eq!(info.server_name, "lapis-host");
    assert_eq!(info.workspace_id, workspace);
    assert_eq!(info.certificate, std::fs::read(&certificate).unwrap());
    let raw = String::from_utf8(std::fs::read(&connection).unwrap()).unwrap();
    assert!(!raw.contains("token") && !raw.contains("secret"), "{raw}");

    let deadline = Instant::now() + Duration::from_secs(5);
    while !invitation.is_file() {
        assert!(Instant::now() < deadline, "招待が書かれません");
        std::thread::sleep(Duration::from_millis(20));
    }
    let (certificate_der, token) = read_host_invitation(&invitation, address, &workspace);
    assert_eq!(certificate_der, std::fs::read(&certificate).unwrap());
    assert_eq!(token.len(), 32);

    writeln!(input, "quit").unwrap();
    input.flush().unwrap();
    wait_for_exit(&mut process.0);
    assert!(!connection.exists(), "正常終了で接続情報を消す");
    assert!(!invitation.exists(), "正常終了で招待を消す");

    // 再起動すると前回のtokenを置き換える。
    let mut command = Command::new(env!("CARGO_BIN_EXE_lapis-host"));
    command
        .arg("--workspace")
        .arg(directory.path())
        .arg("--certificate")
        .arg(&certificate)
        .arg("--replace-certificate")
        .arg("--listen")
        .arg("127.0.0.1:0")
        .arg("--invitation")
        .arg(&invitation)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut process = HostProcess(command.spawn().unwrap());
    let mut input = process.0.stdin.take().unwrap();
    let mut output = BufReader::new(process.0.stdout.take().unwrap());
    let mut startup = String::new();
    output.read_line(&mut startup).unwrap();
    let address = startup
        .split_whitespace()
        .find_map(|value| value.strip_prefix("address="))
        .unwrap()
        .parse::<std::net::SocketAddr>()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !invitation.is_file() {
        assert!(Instant::now() < deadline, "招待が書き直されません");
        std::thread::sleep(Duration::from_millis(20));
    }
    let (_, replaced) = read_host_invitation(&invitation, address, &workspace);
    assert_ne!(token, replaced);
    writeln!(input, "quit").unwrap();
    input.flush().unwrap();
    wait_for_exit(&mut process.0);
    assert!(!invitation.exists());
}

/// 招待が指す接続先と、証明書・tokenの実体を返す。
fn read_host_invitation(
    path: &std::path::Path,
    address: std::net::SocketAddr,
    workspace: &str,
) -> (Vec<u8>, Vec<u8>) {
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(value["version"], 1);
    assert_eq!(value["address"], address.to_string());
    assert_eq!(value["server_name"], "lapis-host");
    assert_eq!(value["workspace_id"], workspace);
    let bytes = |field: &str| -> Vec<u8> {
        value[field]
            .as_array()
            .unwrap()
            .iter()
            .map(|byte| u8::try_from(byte.as_u64().unwrap()).unwrap())
            .collect()
    };
    (bytes("certificate"), bytes("token"))
}
