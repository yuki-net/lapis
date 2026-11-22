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
    // CI のコールド runner では PTY とシェルの起動に時間がかかる。20 秒では
    // 足りずに落ちたため 60 秒まで待つ（ローカルは数秒で返る）。
    for _ in 0..600 {
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
    let mut found_profiles = false;
    for _ in 0..100 {
        if let Ok(profiles) = first.profiles(Path::new(ROOT))
            && profiles
                .default
                .as_ref()
                .is_some_and(|id| id.as_str() == "powershell")
            && profiles
                .profiles
                .iter()
                .any(|profile| profile.id.as_str() == "cmd")
        {
            found_profiles = true;
            break;
        }
        TimeoutFuture::new(100).await;
    }
    if !found_profiles {
        return Err(JsValue::from_str("Terminal profiles unavailable"));
    }
    let id = first
        .start_profile(
            Path::new(ROOT),
            80,
            24,
            Some(terminal::TerminalProfileId::new("powershell")),
        )
        .map_err(failure)?;
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
    let mut closed = false;
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
            closed = true;
            break;
        }
        TimeoutFuture::new(100).await;
    }
    if !closed {
        return Err(JsValue::from_str("Terminal did not close"));
    }
    let cmd = second
        .start_profile(
            Path::new(ROOT),
            80,
            24,
            Some(terminal::TerminalProfileId::new("cmd")),
        )
        .map_err(failure)?;
    second
        .input(&cmd, "echo web-cmd-proof\r")
        .map_err(failure)?;
    wait_for_text(&mut second, "web-cmd-proof").await?;
    second.terminate_and_forget(&cmd).map_err(failure)?;
    Ok("ok".into())
}
