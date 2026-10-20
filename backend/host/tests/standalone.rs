use lapis_client_api::*;
use lapis_remote::*;
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

fn hello() -> ClientHello {
    ClientHello {
        protocol: ProtocolRange::exact(CURRENT_PROTOCOL),
        client_id: ClientId::try_new("standalone-test").unwrap(),
        client_name: "Test".to_owned(),
        client_kind: ClientKind::Desktop,
        requested_capabilities: lapis_host::supported_capabilities(),
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
fn independent_host_keeps_running_after_client_disconnect_and_rejects_certificate_overwrite() {
    let directory = tempfile::tempdir().unwrap();
    let certificate = directory.path().join("host.der");
    let mut command = Command::new(env!("CARGO_BIN_EXE_lapis-host"));
    command
        .arg("--workspace")
        .arg(directory.path())
        .arg("--certificate")
        .arg(&certificate)
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
