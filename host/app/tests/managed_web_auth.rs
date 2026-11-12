use client_api::*;
use remote::*;
use std::{
    io::{BufRead, BufReader},
    net::SocketAddr,
    process::{Child, Command, Stdio},
    time::Duration,
};

struct ManagedHost {
    process: Child,
    bootstrap: serde_json::Value,
}

impl ManagedHost {
    fn start(root: &std::path::Path, state: &std::path::Path) -> Self {
        Self::start_with_web(root, state, None)
    }

    fn start_with_web(
        root: &std::path::Path,
        state: &std::path::Path,
        web: Option<(&std::path::Path, &std::path::Path)>,
    ) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_lapis-host"));
        command
            .args([
                std::ffi::OsStr::new("--managed-host"),
                root.as_os_str(),
                state.as_os_str(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some((executable, dist)) = web {
            command
                .env("LAPIS_WEB_BRIDGE_EXECUTABLE", executable)
                .env("LAPIS_WEB_DIST", dist);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut process = command.spawn().unwrap();
        let mut line = String::new();
        BufReader::new(process.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let bootstrap = serde_json::from_str(&line).expect("managed Host bootstrap");
        Self { process, bootstrap }
    }

    async fn connect(&self) -> QuicClient {
        let address: SocketAddr = self.bootstrap["address"].as_str().unwrap().parse().unwrap();
        let certificate: Vec<u8> =
            serde_json::from_value(self.bootstrap["certificate"].clone()).unwrap();
        QuicClient::connect(
            address,
            "lapis-host",
            certificate.into(),
            RemoteLimits::default(),
        )
        .await
        .unwrap()
    }

    async fn owner(&self) -> QuicClient {
        let mut client = self.connect().await;
        let id =
            CredentialId::parse(self.bootstrap["credential"].as_str().unwrap().to_owned()).unwrap();
        let bytes: Vec<u8> = serde_json::from_value(self.bootstrap["secret"].clone()).unwrap();
        let secret = zeroize::Zeroizing::new(<[u8; 32]>::try_from(bytes).unwrap());
        let hello = hello(self.bootstrap["client"].as_str().unwrap());
        client
            .send(&ClientMessage::Authenticate(AuthenticateRequest::new_auto(
                hello, &id, &secret,
            )))
            .await
            .unwrap();
        assert!(matches!(
            receive(&mut client).await,
            ServerMessage::Authenticated(_)
        ));
        client
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "LAPIS_TEST_WEB_BRIDGE にビルド済み bridge を指定して実行する"]
async fn managed_host_owns_web_publication_and_stops_it_on_exit() {
    use futures_util::{SinkExt, StreamExt};
    use std::io::{Read, Write};
    use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};
    let executable = std::path::PathBuf::from(
        std::env::var_os("LAPIS_TEST_WEB_BRIDGE").expect("bridge executable"),
    );
    let root = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let dist = tempfile::tempdir().unwrap();
    let listen_ip = std::env::var("LAPIS_TEST_WEB_ADDRESS").unwrap_or_else(|_| "127.0.0.1".into());
    std::fs::write(dist.path().join("index.html"), "managed web").unwrap();
    let host =
        ManagedHost::start_with_web(root.path(), state.path(), Some((&executable, dist.path())));
    let mut owner = host.owner().await;
    assert!(matches!(
        request(
            &mut owner,
            "invalid",
            RequestBody::HostWebStart(HostWebStartRequest {
                address: "0.0.0.0".into(),
                port: 0
            })
        )
        .await,
        ResponseBody::Error(_)
    ));
    assert!(matches!(
        request(
            &mut owner,
            "status",
            RequestBody::HostWebStatus(HostWebStatusRequest {})
        )
        .await,
        ResponseBody::HostWebStatus(HostWebStatus { url: None, .. })
    ));
    let response = request(
        &mut owner,
        "start",
        RequestBody::HostWebStart(HostWebStartRequest {
            address: listen_ip.clone(),
            port: 0,
        }),
    )
    .await;
    let ResponseBody::HostWebStart(status) = response else {
        panic!("start web: {response:?}")
    };
    let url = status.url.unwrap();
    let address: SocketAddr = url
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .parse()
        .unwrap();
    let mut stream =
        std::net::TcpStream::connect_timeout(&address, Duration::from_secs(2)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    write!(
        stream,
        "GET / HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.contains("managed web"));
    let ResponseBody::HostInvite(invite) = request(
        &mut owner,
        "web-invite",
        RequestBody::HostInvite(HostInviteRequest {}),
    )
    .await
    else {
        panic!("web invite")
    };
    let mut token = zeroize::Zeroizing::new([0u8; 32]);
    for (i, byte) in token.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&invite.token[i * 2..i * 2 + 2], 16).unwrap();
    }
    let mut ws_request = format!("ws://{address}/remote")
        .into_client_request()
        .unwrap();
    ws_request
        .headers_mut()
        .insert("origin", format!("http://{address}").parse().unwrap());
    ws_request
        .headers_mut()
        .insert("sec-websocket-protocol", "lapis.v0".parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(ws_request).await.unwrap();
    let mut web_hello = hello("web-socket-test");
    web_hello.client_kind = ClientKind::Web;
    let pair = ClientMessage::Pair(PairRequest::new(web_hello, &token));
    socket
        .send(Message::Text(serde_json::to_string(&pair).unwrap().into()))
        .await
        .unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let response: ServerMessage = serde_json::from_str(reply.to_text().unwrap()).unwrap();
    assert!(matches!(response, ServerMessage::Paired(_)));
    // 同じ公開アドレスに、別の招待で二台目を登録しても一台目は接続を維持する。
    let ResponseBody::HostInvite(invite) = request(
        &mut owner,
        "second-web-invite",
        RequestBody::HostInvite(HostInviteRequest {}),
    )
    .await
    else {
        panic!("second web invite")
    };
    for (i, byte) in token.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&invite.token[i * 2..i * 2 + 2], 16).unwrap();
    }
    let mut second_request = format!("ws://{address}/remote")
        .into_client_request()
        .unwrap();
    second_request
        .headers_mut()
        .insert("origin", format!("http://{address}").parse().unwrap());
    second_request
        .headers_mut()
        .insert("sec-websocket-protocol", "lapis.v0".parse().unwrap());
    let (mut second_socket, _) = tokio_tungstenite::connect_async(second_request)
        .await
        .unwrap();
    let mut second_hello = hello("second-web-socket-test");
    second_hello.client_kind = ClientKind::Web;
    let pair = ClientMessage::Pair(PairRequest::new(second_hello, &token));
    second_socket
        .send(Message::Text(serde_json::to_string(&pair).unwrap().into()))
        .await
        .unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(5), second_socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(
        serde_json::from_str::<ServerMessage>(reply.to_text().unwrap()).unwrap(),
        ServerMessage::Paired(_)
    ));
    for (index, client) in [&mut socket, &mut second_socket].into_iter().enumerate() {
        let message = ClientMessage::Request(RequestEnvelope {
            request_id: RequestId::try_new(format!("list-{index}")).unwrap(),
            body: RequestBody::WorkspaceList(WorkspaceListRequest {}),
        });
        client
            .send(Message::Text(
                serde_json::to_string(&message).unwrap().into(),
            ))
            .await
            .unwrap();
        let reply = tokio::time::timeout(Duration::from_secs(5), client.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(matches!(
            serde_json::from_str::<ServerMessage>(reply.to_text().unwrap()).unwrap(),
            ServerMessage::Response(ResponseEnvelope {
                body: ResponseBody::WorkspaceList(_),
                ..
            })
        ));
    }
    second_socket.close(None).await.unwrap();
    socket.close(None).await.unwrap();
    assert!(matches!(
        request(
            &mut owner,
            "stop",
            RequestBody::HostWebStop(HostWebStopRequest {})
        )
        .await,
        ResponseBody::HostWebStop(HostWebStatus { url: None, .. })
    ));
    assert!(std::net::TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_err());
    let ResponseBody::HostWebStart(status) = request(
        &mut owner,
        "restart",
        RequestBody::HostWebStart(HostWebStartRequest {
            address: listen_ip,
            port: 0,
        }),
    )
    .await
    else {
        panic!("restart web")
    };
    let address: SocketAddr = status
        .url
        .unwrap()
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .parse()
        .unwrap();
    // 接続が残っていても Host 終了とともに公開を終了する。
    let _open_connection = std::net::TcpStream::connect(address).unwrap();
    drop(owner);
    drop(host);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::net::TcpStream::connect_timeout(&address, Duration::from_millis(100)).is_ok() {
        assert!(
            std::time::Instant::now() < deadline,
            "Web outlived the Host"
        );
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    let host =
        ManagedHost::start_with_web(root.path(), state.path(), Some((&executable, dist.path())));
    let mut owner = host.owner().await;
    let ResponseBody::HostWebStatus(status) = request(
        &mut owner,
        "restored",
        RequestBody::HostWebStatus(HostWebStatusRequest {}),
    )
    .await
    else {
        panic!("restored status")
    };
    assert_eq!(
        status.url.as_deref(),
        Some(format!("http://{address}/").as_str())
    );
    assert!(matches!(
        request(
            &mut owner,
            "stop-restored",
            RequestBody::HostWebStop(HostWebStopRequest {})
        )
        .await,
        ResponseBody::HostWebStop(HostWebStatus { url: None, .. })
    ));
    drop(owner);
    drop(host);
    let host =
        ManagedHost::start_with_web(root.path(), state.path(), Some((&executable, dist.path())));
    let mut owner = host.owner().await;
    assert!(matches!(
        request(
            &mut owner,
            "stopped",
            RequestBody::HostWebStatus(HostWebStatusRequest {})
        )
        .await,
        ResponseBody::HostWebStatus(HostWebStatus { url: None, .. })
    ));
}

impl Drop for ManagedHost {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

fn hello(id: &str) -> HelloRequest {
    HelloRequest {
        protocol: ProtocolRange::exact(CURRENT_PROTOCOL),
        client_id: ClientId::try_new(id).unwrap(),
        client_name: "Web test".into(),
        client_kind: ClientKind::Desktop,
        requested_capabilities: host::supported_capabilities(),
    }
}

async fn receive(client: &mut QuicClient) -> ServerMessage {
    tokio::time::timeout(Duration::from_secs(10), client.recv())
        .await
        .unwrap()
        .unwrap()
}

async fn request(client: &mut QuicClient, id: &str, body: RequestBody) -> ResponseBody {
    client
        .send(&ClientMessage::Request(RequestEnvelope {
            request_id: RequestId::try_new(id).unwrap(),
            body,
        }))
        .await
        .unwrap();
    loop {
        if let ServerMessage::Response(response) = receive(client).await {
            return response.body;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn managed_host_persists_web_pairing_and_revocation_across_crashes() {
    let root = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let host = ManagedHost::start(root.path(), state.path());
    let mut owner = host.owner().await;
    let ResponseBody::HostInvite(invite) = request(
        &mut owner,
        "invite",
        RequestBody::HostInvite(HostInviteRequest {}),
    )
    .await
    else {
        panic!("invite")
    };
    let mut token = zeroize::Zeroizing::new([0u8; 32]);
    for (i, byte) in token.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&invite.token[i * 2..i * 2 + 2], 16).unwrap();
    }
    let mut web = host.connect().await;
    web.send(&ClientMessage::Pair(PairRequest::new(
        hello("web-restart-test"),
        &token,
    )))
    .await
    .unwrap();
    let ServerMessage::Paired(mut paired) = receive(&mut web).await else {
        panic!("pair")
    };
    let credential = paired.credential_id().unwrap();
    let secret = zeroize::Zeroizing::new(paired.take_secret().unwrap());
    drop(web);
    drop(owner);
    drop(host);

    let host = ManagedHost::start(root.path(), state.path());
    let mut web = host.connect().await;
    web.send(&ClientMessage::Authenticate(AuthenticateRequest::new_auto(
        hello("web-restart-test"),
        &credential,
        &secret,
    )))
    .await
    .unwrap();
    assert!(matches!(
        receive(&mut web).await,
        ServerMessage::Authenticated(_)
    ));
    let mut owner = host.owner().await;
    let ResponseBody::HostDeviceList(list) = request(
        &mut owner,
        "devices",
        RequestBody::HostDeviceList(HostDeviceListRequest {}),
    )
    .await
    else {
        panic!("devices")
    };
    let device = list
        .devices
        .iter()
        .find(|device| device.client_id == "web-restart-test")
        .unwrap();
    assert!(matches!(
        request(
            &mut owner,
            "revoke",
            RequestBody::HostDeviceRevoke(HostDeviceRevokeRequest {
                device_id: device.id.clone()
            })
        )
        .await,
        ResponseBody::HostDeviceRevoke(HostDeviceRevokeResponse { revoked: true })
    ));
    drop(web);
    drop(owner);
    drop(host);

    let host = ManagedHost::start(root.path(), state.path());
    let mut web = host.connect().await;
    web.send(&ClientMessage::Authenticate(AuthenticateRequest::new_auto(
        hello("web-restart-test"),
        &credential,
        &secret,
    )))
    .await
    .unwrap();
    assert!(matches!(receive(&mut web).await, ServerMessage::Error(_)));
    let mut owner = host.owner().await;
    let ResponseBody::HostInvite(invite) = request(
        &mut owner,
        "reinvite",
        RequestBody::HostInvite(HostInviteRequest {}),
    )
    .await
    else {
        panic!("reinvite")
    };
    let mut token = zeroize::Zeroizing::new([0u8; 32]);
    for (i, byte) in token.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&invite.token[i * 2..i * 2 + 2], 16).unwrap();
    }
    let mut replacement = host.connect().await;
    replacement
        .send(&ClientMessage::Pair(PairRequest::new(
            hello("web-new-device"),
            &token,
        )))
        .await
        .unwrap();
    assert!(matches!(
        receive(&mut replacement).await,
        ServerMessage::Paired(_)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "LAPIS_TEST_WEB_BRIDGE、LAPIS_TEST_WEB_DIST、LAPIS_TEST_CHROME を指定して実行する"]
async fn browser_invitation_connects_and_reloads_on_tailscale_http() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let bridge =
        std::path::PathBuf::from(std::env::var_os("LAPIS_TEST_WEB_BRIDGE").expect("bridge"));
    let dist = std::path::PathBuf::from(std::env::var_os("LAPIS_TEST_WEB_DIST").expect("web dist"));
    let chrome = std::path::PathBuf::from(std::env::var_os("LAPIS_TEST_CHROME").expect("chrome"));
    let bind = std::env::var("LAPIS_TEST_WEB_ADDRESS").unwrap_or_else(|_| "127.0.0.1".into());
    let loopback = bind.parse::<std::net::Ipv4Addr>().unwrap().is_loopback();
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("README.md"), "# Web test\n").unwrap();
    let state = tempfile::tempdir().unwrap();
    let profile = tempfile::tempdir().unwrap();
    let host = ManagedHost::start_with_web(root.path(), state.path(), Some((&bridge, &dist)));
    let mut owner = host.owner().await;
    let ResponseBody::HostWebStart(status) = request(
        &mut owner,
        "start",
        RequestBody::HostWebStart(HostWebStartRequest {
            address: bind,
            port: 0,
        }),
    )
    .await
    else {
        panic!("Web start failed")
    };
    let ResponseBody::HostInvite(invite) = request(
        &mut owner,
        "invite",
        RequestBody::HostInvite(HostInviteRequest {}),
    )
    .await
    else {
        panic!("invitation failed")
    };
    let url = zeroize::Zeroizing::new(format!(
        "{}#/invite?token={}",
        status.url.unwrap(),
        invite.token
    ));

    let mut command = Command::new(chrome);
    command
        .args([
            "--headless=new",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-gpu",
            "--use-angle=swiftshader",
            "--enable-unsafe-swiftshader",
            "--no-proxy-server",
            "--remote-debugging-port=0",
        ])
        .arg(format!("--user-data-dir={}", profile.path().display()))
        .arg("about:blank")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    struct Browser(Child);
    impl Drop for Browser {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let _browser = Browser(command.spawn().unwrap());
    let port_file = profile.path().join("DevToolsActivePort");
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let endpoint = loop {
        if let Ok(content) = std::fs::read_to_string(&port_file) {
            let mut lines = content.lines();
            if let (Some(port), Some(path)) = (lines.next(), lines.next()) {
                break format!("ws://127.0.0.1:{port}{path}");
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "browser did not start"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let (mut socket, _) = tokio_tungstenite::connect_async(endpoint).await.unwrap();
    async fn call(
        socket: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        id: u32,
        session: Option<&str>,
        method: &str,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let mut message = serde_json::json!({"id": id, "method": method, "params": params});
        if let Some(session) = session {
            message["sessionId"] = session.into();
        }
        socket
            .send(Message::Text(message.to_string().into()))
            .await
            .unwrap();
        loop {
            let reply = tokio::time::timeout(Duration::from_secs(10), socket.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let Ok(value) =
                serde_json::from_str::<serde_json::Value>(reply.to_text().unwrap_or(""))
            else {
                continue;
            };
            if value["id"].as_u64() == Some(u64::from(id)) {
                assert!(value.get("error").is_none(), "browser command failed");
                return value["result"].clone();
            }
        }
    }
    let target = call(
        &mut socket,
        1,
        None,
        "Target.createTarget",
        serde_json::json!({"url":"about:blank"}),
    )
    .await;
    let attached = call(
        &mut socket,
        2,
        None,
        "Target.attachToTarget",
        serde_json::json!({"targetId":target["targetId"],"flatten":true}),
    )
    .await;
    let session = attached["sessionId"].as_str().unwrap().to_owned();
    call(
        &mut socket,
        3,
        Some(&session),
        "Page.enable",
        serde_json::json!({}),
    )
    .await;
    call(
        &mut socket,
        4,
        Some(&session),
        "Runtime.enable",
        serde_json::json!({}),
    )
    .await;
    call(
        &mut socket,
        5,
        Some(&session),
        "Page.navigate",
        serde_json::json!({"url":url.as_str()}),
    )
    .await;
    let mut next_id = 10;
    let mut first_origin = None;
    for reload in [false, true] {
        if reload {
            call(
                &mut socket,
                next_id,
                Some(&session),
                "Page.reload",
                serde_json::json!({}),
            )
            .await;
            next_id += 1;
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(45);
        loop {
            let state = call(&mut socket, next_id, Some(&session), "Runtime.evaluate", serde_json::json!({"expression":"JSON.stringify({state:document.body?.dataset.lapis,path:location.pathname,hash:location.hash,origin:performance.timeOrigin,secure:window.isSecureContext})","returnByValue":true})).await;
            next_id += 1;
            let data = state["result"]["value"]
                .as_str()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());
            if let Some(data) = data {
                assert_ne!(data["state"], "error", "Web client initialization failed");
                if data["state"] == "ready"
                    && data["path"] != "/"
                    && (!reload || data["origin"].as_f64() != first_origin)
                {
                    assert!(!data["hash"].as_str().unwrap_or("").contains("token"));
                    assert_eq!(data["secure"], loopback);
                    if !reload {
                        first_origin = data["origin"].as_f64();
                    }
                    break;
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Web client did not become ready"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
    let ResponseBody::HostDeviceList(list) = request(
        &mut owner,
        "web-devices",
        RequestBody::HostDeviceList(HostDeviceListRequest {}),
    )
    .await
    else {
        panic!("devices")
    };
    let device = list
        .devices
        .iter()
        .find(|device| device.kind == "web" && device.status == "approved")
        .expect("browser device");
    assert!(matches!(
        request(
            &mut owner,
            "web-revoke",
            RequestBody::HostDeviceRevoke(HostDeviceRevokeRequest {
                device_id: device.id.clone()
            })
        )
        .await,
        ResponseBody::HostDeviceRevoke(HostDeviceRevokeResponse { revoked: true })
    ));
    call(
        &mut socket,
        next_id,
        Some(&session),
        "Page.reload",
        serde_json::json!({}),
    )
    .await;
    next_id += 1;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let state = call(&mut socket, next_id, Some(&session), "Runtime.evaluate", serde_json::json!({"expression":"Boolean(document.querySelector('#login-error:not([hidden])'))","returnByValue":true})).await;
        next_id += 1;
        if state["result"]["value"] == true {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "revoked browser was not rejected"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let ResponseBody::HostInvite(invite) = request(
        &mut owner,
        "web-reinvite",
        RequestBody::HostInvite(HostInviteRequest {}),
    )
    .await
    else {
        panic!("reinvite")
    };
    let replacement = zeroize::Zeroizing::new(format!(
        "{}#/invite?token={}",
        url.split('#').next().unwrap(),
        invite.token
    ));
    call(
        &mut socket,
        next_id,
        Some(&session),
        "Page.navigate",
        serde_json::json!({"url":replacement.as_str()}),
    )
    .await;
    next_id += 1;
    let deadline = std::time::Instant::now() + Duration::from_secs(45);
    loop {
        let state = call(&mut socket, next_id, Some(&session), "Runtime.evaluate", serde_json::json!({"expression":"document.body?.dataset.lapis === 'ready' && location.pathname !== '/'","returnByValue":true})).await;
        next_id += 1;
        if state["result"]["value"] == true {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "reinvited browser did not reconnect"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let _ = socket
        .send(Message::Text(
            serde_json::json!({"id":next_id,"method":"Browser.close"})
                .to_string()
                .into(),
        ))
        .await;
}
