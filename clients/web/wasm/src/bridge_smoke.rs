use client_api::{
    AuthenticateRequest, CURRENT_PROTOCOL, CapabilityId, CapabilitySet, ClientId, ClientKind,
    ClientMessage, HelloRequest, PairRequest, ProtocolRange, RequestBody, ResponseBody,
    ServerMessage, WorkspaceConnectRequest, WorkspaceId,
};
use futures_util::StreamExt;
use wasm_bindgen::prelude::*;

async fn wait_document_edit(
    events: &mut futures_channel::mpsc::Receiver<client_api::EventEnvelope>,
    document_id: &client_api::DocumentId,
    expected_revision: client_api::Revision,
) -> Result<(), JsValue> {
    let wait = async {
        while let Some(event) = events.next().await {
            if matches!(&event.body, client_api::EventBody::DocumentEdited { document_id: id, revision, .. } if id == document_id && *revision == expected_revision)
            {
                return Ok(());
            }
        }
        Err(JsValue::from_str("event stream closed"))
    };
    match futures_util::future::select(
        Box::pin(wait),
        Box::pin(gloo_timers::future::TimeoutFuture::new(5000)),
    )
    .await
    {
        futures_util::future::Either::Left((result, _)) => result,
        futures_util::future::Either::Right(_) => Err(JsValue::from_str("document event missing")),
    }
}

/// 隔離したHostとBridgeへの接続検証用。通常ビルドには含まれない。
#[wasm_bindgen]
pub async fn bridge_smoke(
    url: String,
    token: String,
    workspace: String,
    other_workspace: String,
    other_root: String,
) -> Result<String, JsValue> {
    let token = zeroize::Zeroizing::new(token);
    let bytes = token.as_bytes();
    if bytes.len() != 64 {
        return Err(JsValue::from_str("invalid token"));
    }
    let mut secret = zeroize::Zeroizing::new([0u8; 32]);
    for (index, pair) in bytes.chunks_exact(2).enumerate() {
        let digit = |v: u8| match v {
            b'0'..=b'9' => Some(v - b'0'),
            b'a'..=b'f' => Some(v - b'a' + 10),
            _ => None,
        };
        secret[index] = (digit(pair[0]).ok_or_else(|| JsValue::from_str("invalid token"))? << 4)
            | digit(pair[1]).ok_or_else(|| JsValue::from_str("invalid token"))?;
    }
    let hello = HelloRequest {
        protocol: ProtocolRange::exact(CURRENT_PROTOCOL),
        client_id: ClientId::try_new("wasm-smoke").unwrap(),
        client_name: "wasm-smoke".into(),
        client_kind: ClientKind::Desktop,
        requested_capabilities: CapabilitySet::try_new([
            CapabilityId::try_new("workspace.list").unwrap(),
            CapabilityId::try_new("workspace.connect").unwrap(),
            CapabilityId::try_new("workspace.switch").unwrap(),
            CapabilityId::try_new("workspace.register").unwrap(),
            CapabilityId::try_new("host.manage").unwrap(),
            CapabilityId::try_new("workspace.sync").unwrap(),
            CapabilityId::try_new("workspace.workbench.write").unwrap(),
            CapabilityId::try_new("workspace.documents.read").unwrap(),
            CapabilityId::try_new("workspace.documents.write").unwrap(),
            CapabilityId::try_new("workspace.terminal.start").unwrap(),
            CapabilityId::try_new("workspace.terminal.control").unwrap(),
        ])
        .unwrap(),
    };
    let workspace =
        WorkspaceId::try_new(workspace).map_err(|_| JsValue::from_str("invalid workspace"))?;
    let other_workspace = WorkspaceId::try_new(other_workspace)
        .map_err(|_| JsValue::from_str("invalid other workspace"))?;
    let auth = ClientMessage::Pair(PairRequest::new(hello.clone(), &secret));
    drop(secret);
    let (mut connection, result) = crate::connection::BrowserConnection::connect(&url, auth)
        .await
        .map_err(|e| JsValue::from_str(&e))?;
    let ServerMessage::Paired(mut paired) = result else {
        return Err(JsValue::from_str("unexpected authentication reply"));
    };
    if !paired
        .hello
        .granted_capabilities
        .contains(&CapabilityId::try_new("workspace.switch").unwrap())
    {
        return Err(JsValue::from_str("workspace switch was not granted"));
    }
    if paired
        .hello
        .granted_capabilities
        .contains(&CapabilityId::try_new("host.manage").unwrap())
    {
        return Err(JsValue::from_str("user session was given host management"));
    }
    let credential_id = paired
        .credential_id()
        .map_err(|_| JsValue::from_str("invalid credential"))?;
    let secret = zeroize::Zeroizing::new(
        paired
            .take_secret()
            .map_err(|_| JsValue::from_str("invalid credential secret"))?,
    );
    let auth = ClientMessage::Authenticate(AuthenticateRequest::new(
        hello.clone(),
        workspace.clone(),
        &credential_id,
        &secret,
    ));
    let (mut other, _) = crate::connection::BrowserConnection::connect(&url, auth)
        .await
        .map_err(|e| JsValue::from_str(&e))?;
    let mut events = connection
        .take_events()
        .ok_or_else(|| JsValue::from_str("event stream unavailable"))?;
    let mut other_events = other
        .take_events()
        .ok_or_else(|| JsValue::from_str("other event stream unavailable"))?;
    let body = client_core::AsyncHostConnection::request(
        &connection,
        RequestBody::WorkspaceConnect(WorkspaceConnectRequest {
            workspace_id: workspace.clone(),
        }),
    )
    .await
    .map_err(|e| JsValue::from_str(&e))?;
    if !matches!(body, ResponseBody::WorkspaceConnect(_)) {
        return Err(JsValue::from_str("workspace connection rejected"));
    }
    let body = client_core::AsyncHostConnection::request(
        &other,
        RequestBody::WorkspaceConnect(WorkspaceConnectRequest {
            workspace_id: workspace.clone(),
        }),
    )
    .await
    .map_err(|e| JsValue::from_str(&e))?;
    if !matches!(body, ResponseBody::WorkspaceConnect(_)) {
        return Err(JsValue::from_str("other workspace connection rejected"));
    }
    let mut first_sync = client_core::workbench::AsyncWorkbenchSync::new(&connection);
    let mut second_sync = client_core::workbench::AsyncWorkbenchSync::new(&other);
    first_sync
        .open(workspace.clone())
        .await
        .map_err(|e| JsValue::from_str(&e))?;
    second_sync
        .open(workspace.clone())
        .await
        .map_err(|e| JsValue::from_str(&e))?;
    let view = workbench::ConversationViewState {
        show_panel_controls: false,
        workspace: Some(workbench::SharedWorkspaceView {
            workspace_id: editor_core::WorkspaceId::new(workspace.as_str()),
            open_documents: vec![workbench::SharedDocumentView {
                document_id: None,
                path: Some(client_api::WorkspaceRelativePath::parse("src/画面.rs").unwrap()),
                cursor_char: 2,
                selection_start: 1,
                selection_end: 2,
                scroll_x: 0.0,
                scroll_y: 1.0,
                draft_content: Some("未保存😀".into()),
            }],
            active_path: Some(client_api::WorkspaceRelativePath::parse("src/画面.rs").unwrap()),
            active_document_id: None,
        }),
        ..Default::default()
    };
    let workbench::WorkbenchUpdate::Updated(updated) = first_sync
        .save(view.clone())
        .await
        .map_err(|e| JsValue::from_str(&e))?
    else {
        return Err(JsValue::from_str("workbench write failed"));
    };
    let event = futures_util::future::select(
        Box::pin(events.next()),
        Box::pin(gloo_timers::future::TimeoutFuture::new(5000)),
    )
    .await;
    if !matches!(event, futures_util::future::Either::Left((Some(event), _)) if matches!(event.body, client_api::EventBody::WorkbenchChanged { revision, .. } if revision == updated.revision))
    {
        return Err(JsValue::from_str("workbench event missing"));
    }
    let event = futures_util::future::select(
        Box::pin(other_events.next()),
        Box::pin(gloo_timers::future::TimeoutFuture::new(5000)),
    )
    .await;
    if !matches!(event, futures_util::future::Either::Left((Some(event), _)) if matches!(event.body, client_api::EventBody::WorkbenchChanged { revision, .. } if revision == updated.revision))
    {
        return Err(JsValue::from_str("other workbench event missing"));
    }
    let next = workbench::ConversationViewState::default();
    let stale = second_sync
        .save(next.clone())
        .await
        .map_err(|e| JsValue::from_str(&e))?;
    if !matches!(stale, workbench::WorkbenchUpdate::Conflict(ref current) if current.revision == updated.revision && current.view.as_ref() == Some(&view))
    {
        return Err(JsValue::from_str("stale write was not rejected"));
    }
    if second_sync.pending() != Some(&next) {
        return Err(JsValue::from_str("conflicting local view was lost"));
    }
    let remote = second_sync
        .accept_remote()
        .map_err(|e| JsValue::from_str(&e))?;
    if remote.view.as_ref() != Some(&view) {
        return Err(JsValue::from_str("other client did not observe view"));
    }
    let workbench::WorkbenchUpdate::Updated(updated) = second_sync
        .save(next.clone())
        .await
        .map_err(|e| JsValue::from_str(&e))?
    else {
        return Err(JsValue::from_str("other workbench write failed"));
    };
    let event = futures_util::future::select(
        Box::pin(events.next()),
        Box::pin(gloo_timers::future::TimeoutFuture::new(5000)),
    )
    .await;
    if !matches!(event, futures_util::future::Either::Left((Some(event), _)) if matches!(event.body, client_api::EventBody::WorkbenchChanged { revision, .. } if revision == updated.revision))
    {
        return Err(JsValue::from_str("reverse workbench event missing"));
    }
    let current = first_sync
        .reconnect(&other)
        .await
        .map_err(|e| JsValue::from_str(&e))?
        .ok_or_else(|| JsValue::from_str("first client did not refresh"))?;
    if current.view.as_ref() != Some(&next) {
        return Err(JsValue::from_str("first client did not observe view"));
    }
    drop(first_sync);
    drop(second_sync);
    let created = client_core::AsyncHostConnection::request(
        &connection,
        RequestBody::DocumentUntitled(client_api::DocumentUntitledRequest {
            workspace_id: workspace.clone(),
        }),
    )
    .await
    .map_err(|e| JsValue::from_str(&e))?;
    let ResponseBody::DocumentUntitled(created) = created else {
        return Err(JsValue::from_str("document creation failed"));
    };
    let document_id = created.document.document_id;
    let original_revision = created.document.revision;
    let attached = client_core::AsyncHostConnection::request(
        &other,
        RequestBody::DocumentInspect(client_api::DocumentInspectRequest {
            document_id: document_id.clone(),
        }),
    )
    .await
    .map_err(|e| JsValue::from_str(&e))?;
    if !matches!(attached, ResponseBody::DocumentInspect(_)) {
        return Err(JsValue::from_str("document attach failed"));
    }
    let edit = || {
        client_api::DocumentTransaction::try_new(vec![
            client_api::DocumentTextEdit::try_new(0, 0, "共有された未保存入力").unwrap(),
        ])
        .unwrap()
    };
    let edited = client_core::AsyncHostConnection::request(
        &connection,
        RequestBody::DocumentEdit(client_api::DocumentEditRequest {
            document_id: document_id.clone(),
            base_revision: original_revision,
            transaction: edit(),
        }),
    )
    .await
    .map_err(|e| JsValue::from_str(&e))?;
    let ResponseBody::DocumentEdit(edited) = edited else {
        return Err(JsValue::from_str("document edit failed"));
    };
    wait_document_edit(&mut other_events, &document_id, edited.revision).await?;
    let stale = client_core::AsyncHostConnection::request(
        &other,
        RequestBody::DocumentEdit(client_api::DocumentEditRequest {
            document_id: document_id.clone(),
            base_revision: original_revision,
            transaction: edit(),
        }),
    )
    .await
    .map_err(|e| JsValue::from_str(&e))?;
    if !matches!(stale, ResponseBody::Error(ref error) if error.code.as_str() == client_api::REVISION_CONFLICT)
    {
        return Err(JsValue::from_str("stale document edit was not rejected"));
    }
    let end = "共有された未保存入力".chars().count() as u64;
    let transaction = client_api::DocumentTransaction::try_new(vec![
        client_api::DocumentTextEdit::try_new(end, end, "・続き").unwrap(),
    ])
    .unwrap();
    let reverse = client_core::AsyncHostConnection::request(
        &other,
        RequestBody::DocumentEdit(client_api::DocumentEditRequest {
            document_id: document_id.clone(),
            base_revision: edited.revision,
            transaction,
        }),
    )
    .await
    .map_err(|e| JsValue::from_str(&e))?;
    let ResponseBody::DocumentEdit(reverse) = reverse else {
        return Err(JsValue::from_str("reverse document edit failed"));
    };
    wait_document_edit(&mut events, &document_id, reverse.revision).await?;
    let started = client_core::AsyncHostConnection::request(
        &connection,
        RequestBody::TerminalStart(client_api::TerminalStartRequest {
            workspace_id: workspace.clone(),
            cwd: None,
            command: None,
            shell: Default::default(),
            size: client_api::TerminalSize {
                columns: 80,
                rows: 24,
            },
        }),
    )
    .await
    .map_err(|e| JsValue::from_str(&e))?;
    let ResponseBody::TerminalStart(started) = started else {
        return Err(JsValue::from_str("terminal start failed"));
    };
    let terminal_id = started.terminal.terminal_id;
    drop(connection);
    drop(other);
    gloo_timers::future::TimeoutFuture::new(250).await;
    let auth = ClientMessage::Authenticate(AuthenticateRequest::new(
        hello.clone(),
        other_workspace.clone(),
        &credential_id,
        &secret,
    ));
    let (switched, _) = crate::connection::BrowserConnection::connect(&url, auth)
        .await
        .map_err(|e| JsValue::from_str(&e))?;
    let connected = client_core::AsyncHostConnection::request(
        &switched,
        RequestBody::WorkspaceConnect(WorkspaceConnectRequest {
            workspace_id: other_workspace.clone(),
        }),
    )
    .await
    .map_err(|e| JsValue::from_str(&e))?;
    if !matches!(connected, ResponseBody::WorkspaceConnect(_)) {
        return Err(JsValue::from_str("other workspace connection failed"));
    }
    let list = client_core::AsyncHostConnection::request(
        &switched,
        RequestBody::WorkspaceList(client_api::WorkspaceListRequest::default()),
    )
    .await
    .map_err(|e| JsValue::from_str(&e))?;
    if !matches!(list, ResponseBody::WorkspaceList(ref list) if list.workspaces.iter().any(|item| item.workspace_id == workspace) && list.workspaces.iter().any(|item| item.workspace_id == other_workspace))
    {
        return Err(JsValue::from_str("user workspace list is incomplete"));
    }
    let registered = client_core::AsyncHostConnection::request(
        &switched,
        RequestBody::HostRegister(client_api::HostRegisterRequest { root: other_root }),
    )
    .await
    .map_err(|e| JsValue::from_str(&e))?;
    if !matches!(registered, ResponseBody::HostRegister(ref result) if result.workspace_id == other_workspace)
    {
        return Err(JsValue::from_str("workspace registration failed"));
    }
    let forbidden = client_core::AsyncHostConnection::request(
        &switched,
        RequestBody::HostWorktree(editor_core::TaskId::new("not-authorized")),
    )
    .await
    .map_err(|e| JsValue::from_str(&e))?;
    if !matches!(forbidden, ResponseBody::Error(ref error) if error.code.as_str() == client_api::FORBIDDEN)
    {
        return Err(JsValue::from_str("host management was not rejected"));
    }
    drop(switched);
    let auth = ClientMessage::Authenticate(AuthenticateRequest::new(
        hello,
        workspace.clone(),
        &credential_id,
        &secret,
    ));
    drop(secret);
    let (reconnected, _) = crate::connection::BrowserConnection::connect(&url, auth)
        .await
        .map_err(|e| JsValue::from_str(&e))?;
    let connected = client_core::AsyncHostConnection::request(
        &reconnected,
        RequestBody::WorkspaceConnect(WorkspaceConnectRequest {
            workspace_id: workspace.clone(),
        }),
    )
    .await
    .map_err(|e| JsValue::from_str(&e))?;
    if !matches!(connected, ResponseBody::WorkspaceConnect(_)) {
        return Err(JsValue::from_str("workspace reconnect failed"));
    }
    let snapshot = client_core::AsyncHostConnection::request(
        &reconnected,
        RequestBody::SnapshotResync(client_api::SnapshotRequest {
            workspace_id: workspace,
            reason: client_api::SnapshotReason::Reconnect,
        }),
    )
    .await
    .map_err(|e| JsValue::from_str(&e))?;
    let ResponseBody::SnapshotResync(snapshot) = snapshot else {
        return Err(JsValue::from_str("terminal snapshot failed"));
    };
    if !snapshot.snapshot.documents.iter().any(|document| {
        document.document_id == document_id
            && document.dirty
            && document.content == "共有された未保存入力・続き"
    }) {
        return Err(JsValue::from_str(
            "dirty document did not survive disconnect",
        ));
    }
    if !snapshot.snapshot.terminals.iter().any(|terminal| {
        terminal.terminal_id == terminal_id
            && terminal.status == client_api::TerminalStatus::Running
    }) {
        return Err(JsValue::from_str("terminal did not survive disconnect"));
    }
    let input = client_core::AsyncHostConnection::request(
        &reconnected,
        RequestBody::TerminalInput(client_api::TerminalInputRequest {
            terminal_id: terminal_id.clone(),
            input: client_api::TerminalInput::Text("\r".into()),
        }),
    )
    .await
    .map_err(|e| JsValue::from_str(&e))?;
    if !matches!(input, ResponseBody::TerminalInput(_)) {
        return Err(JsValue::from_str("terminal input after disconnect failed"));
    }
    let terminated = client_core::AsyncHostConnection::request(
        &reconnected,
        RequestBody::TerminalTerminate(client_api::TerminalTerminateRequest { terminal_id }),
    )
    .await
    .map_err(|e| JsValue::from_str(&e))?;
    if !matches!(terminated, ResponseBody::TerminalTerminate(_)) {
        return Err(JsValue::from_str("terminal cleanup failed"));
    }
    Ok("ok".to_owned())
}
