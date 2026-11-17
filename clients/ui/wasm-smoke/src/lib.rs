//! 実 Host と接続した Web 拡張 Terminal の結合検証。
#![cfg(target_arch = "wasm32")]

use std::{path::Path, sync::Arc};

use client_api::HostExtensionSetCapabilityRequest;
use desktop_client::{
    TerminalSession,
    web_host::{self, ExtensionTerminalBackend, HostConfig, ROOT},
};
use gloo_timers::future::TimeoutFuture;
use terminal::TerminalId;
use wasm_bindgen::prelude::*;

fn failure(message: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&message.to_string())
}

async fn wait_for_text(session: &mut TerminalSession, expected: &str) -> Result<(), JsValue> {
    for _ in 0..200 {
        session.refresh().map_err(failure)?;
        if session
            .terminals()
            .iter()
            .any(|terminal| terminal.screen.plain_text().contains(expected))
        {
            return Ok(());
        }
        TimeoutFuture::new(100).await;
    }
    Err(JsValue::from_str("Terminal output timed out"))
}

/// 検証専用 feature でだけ export する。
#[wasm_bindgen]
pub async fn extension_terminal_smoke(
    url: String,
    token: String,
    workspace: String,
) -> Result<String, JsValue> {
    let credential = web_host::connect(HostConfig {
        url: url.clone(),
        workspace: workspace.clone(),
        client_id: "web-extension-smoke".into(),
        token: Some(token),
        credential: None,
        id_token: None,
    })
    .await
    .map_err(failure)?
    .ok_or_else(|| JsValue::from_str("pairing returned no credential"))?;
    let capabilities = web_host::extension_capabilities("lapis.terminal".into())
        .await
        .map_err(failure)?;
    web_host::set_extension_capability(HostExtensionSetCapabilityRequest {
        id: "lapis.terminal".into(),
        manifest_digest: capabilities.manifest_digest,
        capability: "process.pty".into(),
        version: "1.0.0".into(),
        approved: true,
    })
    .await
    .map_err(failure)?;
    let backend = Arc::new(ExtensionTerminalBackend);
    let mut first = TerminalSession::new(backend.clone());
    let id = first.start(Path::new(ROOT), 80, 24).map_err(failure)?;
    if !id.as_str().starts_with("client-") {
        return Err(JsValue::from_str("invalid session ID"));
    }
    first
        .input(&id, "[console]::WriteLine('web'+'proof')\r")
        .map_err(failure)?;
    wait_for_text(&mut first, "webproof").await?;

    web_host::connect(HostConfig {
        url,
        workspace,
        client_id: "web-extension-smoke".into(),
        token: None,
        credential: Some(credential),
        id_token: None,
    })
    .await
    .map_err(failure)?;
    let mut second = TerminalSession::new(backend);
    second
        .attach(Path::new(ROOT), &TerminalId::new(id.as_str()))
        .map_err(failure)?;
    wait_for_text(&mut second, "webproof").await?;
    let mut released = false;
    for _ in 0..100 {
        second.refresh().map_err(failure)?;
        if second.owner(&id).is_none() {
            released = true;
            break;
        }
        TimeoutFuture::new(100).await;
    }
    if !released {
        return Err(JsValue::from_str(
            "previous connection retained Terminal ownership",
        ));
    }
    second.claim(&id).map_err(failure)?;
    second
        .input(&id, "[console]::WriteLine('re'+'attached')\r")
        .map_err(failure)?;
    wait_for_text(&mut second, "reattached").await?;
    second.terminate_and_forget(&id).map_err(failure)?;
    for _ in 0..100 {
        if !web_host::ExtensionTerminal::list()
            .await
            .map_err(failure)?
            .iter()
            .any(|item| item.id == id.as_str())
        {
            return Ok("ok".into());
        }
        TimeoutFuture::new(100).await;
    }
    Err(JsValue::from_str("Terminal did not close"))
}
