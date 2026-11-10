//! ブラウザーでDesktop UIを起動する境界。

#[cfg(target_family = "wasm")]
mod prototype;
#[cfg(target_family = "wasm")]
mod startup;

#[cfg(target_family = "wasm")]
pub use startup::run_web;

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

/// 接続中の Host/Workspace 識別子と切り替え候補を JSON で返す。
#[wasm_bindgen::prelude::wasm_bindgen(js_name = connectionInfo)]
pub fn connection_info() -> Option<String> {
    desktop_client::web_host::connection_info().and_then(|info| serde_json::to_string(&info).ok())
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
    let Some(url) = url else {
        return start_without_host(backend_preference);
    };
    if url.trim().is_empty() {
        return start_without_host(backend_preference);
    }
    let credential = desktop_client::web_host::connect(desktop_client::web_host::HostConfig {
        url,
        workspace: workspace.unwrap_or_default(),
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
            "Host URL を指定してください。Workspace ID を省略した場合は接続時に primary Workspace を取得します。デモは ?demo=1 で起動できます。",
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

/// URL の `#/t/<task-id>` で指定された Task。Web の初期選択にだけ使う。
fn requested_task() -> Option<editor_core::ConversationId> {
    let hash = web_sys::window()?.location().hash().ok()?;
    let id = hash.strip_prefix("#/t/")?;
    (!id.is_empty()).then(|| editor_core::ConversationId::new(id))
}

/// 選択中の Task を URL fragment へ反映するようページへ通知する。
pub(crate) fn notify_task_selected(task: &str) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let init = web_sys::CustomEventInit::new();
    init.set_detail(&JsValue::from_str(task));
    if let Ok(event) = web_sys::CustomEvent::new_with_event_init_dict("lapis:task-selected", &init)
    {
        let _ = window.dispatch_event(&event);
    }
}

fn start_prototype(backend_preference: &str) -> Result<(), String> {
    let backend = Arc::new(crate::web::prototype::PrototypeBackend::new());
    launch(
        backend,
        crate::web::prototype::ROOT,
        Some(PathBuf::from(crate::web::prototype::ROOT).join("README.md")),
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
    let mut services = DesktopServices::new(
        TaskSession::new(backend.clone()),
        GitSession::new(backend.clone()),
        LspSession::new(backend.clone()),
        TerminalSession::new(backend.clone()),
        WorkspaceSearchSession::new(backend.clone()),
        conversation,
        settings,
    );
    // 表示用の Workspace 名は既存の ConnectionInfo をそのまま使う。
    if let Some(info) = desktop_client::web_host::connection_info() {
        services = services.with_connection_identity(crate::ConnectionIdentity {
            host_name: Some(info.host_name),
            host_id: info.host_id,
            workspace_name: Some(info.workspace_name),
            workspace_id: Some(info.workspace_id),
        });
    }
    if let Some(task) = requested_task() {
        services = services.with_initial_task(task);
    }
    let handle = crate::run_web(
        session,
        services,
        InitialView::default(),
        vec![
            Cow::<'static, [u8]>::Borrowed(include_bytes!("../../assets/IBMPlexSans-Regular.ttf")),
            Cow::<'static, [u8]>::Borrowed(include_bytes!("../../assets/IBMPlexSans-SemiBold.ttf")),
        ],
        backend_preference,
        || READY.with(|ready| ready.set(true)),
    );
    APPLICATION.with(|application| *application.borrow_mut() = Some(handle));
    Ok(())
}
