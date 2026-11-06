//! ブラウザーでDesktop UIを起動する境界。

use std::{
    borrow::Cow,
    cell::{Cell, RefCell},
    path::PathBuf,
    sync::Arc,
};

use desktop_client::{
    ConversationRepository, ConversationSession, EditorSession, GitSession, LspSession,
    SettingsSession, TaskSession, TerminalSession, WorkspaceDialog, WorkspaceRepository,
    WorkspaceSearchSession, WorkspaceStateRepository,
};
use document::DocumentConnection;
use git::GitConnection;
use lsp::LanguageServerBackend;
use settings::GlobalSettingsRepository;
use task::TaskConnection;
use terminal::TerminalBackend;
use wasm_bindgen::prelude::*;
use workspace::WorkspaceSearchBackend;

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
    id_token: Option<String>,
    client_id: String,
) -> Result<Option<String>, JsValue> {
    console_error_panic_hook::set_once();
    let (Some(url), Some(workspace)) = (url, workspace) else {
        return start_without_host(backend_preference);
    };
    if url.trim().is_empty() || workspace.trim().is_empty() {
        return start_without_host(backend_preference);
    }
    let credential = desktop_client::web_host::connect(desktop_client::web_host::HostConfig {
        url,
        workspace,
        client_id,
        token,
        credential,
        id_token,
    })
    .await
    .map_err(|error| JsValue::from_str(&error))?;
    let backend = Arc::new(desktop_client::web_host::WebBackend);
    launch(
        backend,
        desktop_client::web_host::ROOT,
        desktop_client::web_host::initial_path(),
        backend_preference,
    )
    .map_err(|error| JsValue::from_str(&error))?;
    Ok(credential)
}

fn start_without_host(backend_preference: &str) -> Result<Option<String>, JsValue> {
    if !demo_requested() {
        return Err(JsValue::from_str(
            "Host URL と Workspace ID を指定してください。デモは ?demo=1 で起動できます。",
        ));
    }
    start_prototype(backend_preference).map_err(|error| JsValue::from_str(&error))?;
    Ok(None)
}

fn demo_requested() -> bool {
    web_sys::window()
        .and_then(|window| window.location().search().ok())
        .is_some_and(|search| {
            search
                .trim_start_matches('?')
                .split('&')
                .any(|param| param == "demo=1")
        })
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
    let conversation = ConversationSession::new(backend.clone(), session.snapshot());
    let restored = desktop_client::web_host::restore_snapshot(PathBuf::from(root));
    if let Some(snapshot) = restored {
        session
            .restore_snapshot(snapshot)
            .map_err(|error| error.to_string())?;
    } else if let Some(path) = initial {
        let _ = session.open_path(path);
    }
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
