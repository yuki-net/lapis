//! ブラウザーでDesktop UIを起動する境界。

use std::{
    borrow::Cow,
    cell::{Cell, RefCell},
    path::PathBuf,
    sync::Arc,
};

use lapis_desktop_client::{
    ConversationRepository, ConversationSession, EditorSession, GitSession, LspSession,
    SettingsSession, TaskSession, TerminalSession, WorkspaceDialog, WorkspaceRepository,
    WorkspaceSearchSession, WorkspaceStateRepository,
};
use lapis_document::DocumentConnection;
use lapis_git::GitConnection;
use lapis_lsp::LanguageServerBackend;
use lapis_settings::GlobalSettingsRepository;
use lapis_task::TaskConnection;
use lapis_terminal::TerminalBackend;
use lapis_workspace::WorkspaceSearchBackend;
use wasm_bindgen::prelude::*;

use crate::{DesktopServices, InitialView};

thread_local! {
    static APPLICATION: RefCell<Option<gpui_preview::ApplicationHandle>> = const { RefCell::new(None) };
    static READY: Cell<bool> = const { Cell::new(false) };
}

#[wasm_bindgen::prelude::wasm_bindgen]
pub fn is_ready() -> bool {
    READY.with(Cell::get)
}

/// Hostの設定が渡された場合はHostへ接続し、無ければメモリー上のWorkspaceで起動する。
/// pairingで新しいcredentialが発行された場合だけ、保存用に返す。
#[wasm_bindgen::prelude::wasm_bindgen]
pub async fn start(
    backend_preference: &str,
    url: Option<String>,
    workspace: Option<String>,
    token: Option<String>,
    credential: Option<String>,
    client_id: String,
) -> Result<Option<String>, JsValue> {
    console_error_panic_hook::set_once();
    let (Some(url), Some(workspace)) = (url, workspace) else {
        start_prototype(backend_preference).map_err(|error| JsValue::from_str(&error))?;
        return Ok(None);
    };
    if url.trim().is_empty() || workspace.trim().is_empty() {
        start_prototype(backend_preference).map_err(|error| JsValue::from_str(&error))?;
        return Ok(None);
    }
    let credential =
        lapis_desktop_client::web_host::connect(lapis_desktop_client::web_host::HostConfig {
            url,
            workspace,
            client_id,
            token,
            credential,
        })
        .await
        .map_err(|error| JsValue::from_str(&error))?;
    let backend = Arc::new(lapis_desktop_client::web_host::WebBackend);
    launch(
        backend,
        lapis_desktop_client::web_host::ROOT,
        lapis_desktop_client::web_host::initial_path(),
        backend_preference,
    )
    .map_err(|error| JsValue::from_str(&error))?;
    Ok(credential)
}

fn start_prototype(backend_preference: &str) -> Result<(), String> {
    let backend = Arc::new(crate::web_prototype::PrototypeBackend::new());
    launch(
        backend,
        crate::web_prototype::ROOT,
        Some(PathBuf::from(crate::web_prototype::ROOT).join("README.md")),
        backend_preference,
    )
}

fn launch<T>(
    backend: Arc<T>,
    root: &str,
    initial: Option<PathBuf>,
    backend_preference: &str,
) -> Result<(), String>
where
    T: WorkspaceRepository
        + WorkspaceDialog
        + WorkspaceStateRepository
        + DocumentConnection
        + GitConnection
        + TaskConnection
        + TerminalBackend
        + LanguageServerBackend
        + WorkspaceSearchBackend
        + ConversationRepository
        + GlobalSettingsRepository
        + Send
        + Sync
        + 'static,
{
    let mut session = EditorSession::new_empty(
        backend.clone(),
        backend.clone(),
        backend.clone(),
        backend.clone(),
    );
    session
        .open_workspace(PathBuf::from(root))
        .map_err(|error| error.to_string())?;
    if let Some(path) = initial {
        let _ = session.open_path(path);
    }
    let conversation = ConversationSession::new(backend.clone(), session.snapshot());
    let settings = SettingsSession::load(backend.clone()).map_err(|error| error.to_string())?;
    let services = DesktopServices::new(
        TaskSession::new(backend.clone()),
        GitSession::new(backend.clone()),
        LspSession::new(backend.clone()),
        TerminalSession::new(backend.clone()),
        WorkspaceSearchSession::new(backend.clone()),
        conversation,
        settings,
    );
    let handle = crate::run_web(
        session,
        services,
        InitialView::default(),
        vec![
            Cow::<'static, [u8]>::Borrowed(include_bytes!("../assets/IBMPlexSans-Regular.ttf")),
            Cow::<'static, [u8]>::Borrowed(include_bytes!("../assets/IBMPlexSans-SemiBold.ttf")),
        ],
        backend_preference,
        || READY.with(|ready| ready.set(true)),
    );
    APPLICATION.with(|application| *application.borrow_mut() = Some(handle));
    Ok(())
}
