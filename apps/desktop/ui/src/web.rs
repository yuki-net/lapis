//! ブラウザーでDesktop UIを起動する境界。

use std::{
    borrow::Cow,
    cell::{Cell, RefCell},
    path::Path,
    sync::Arc,
};

use crate::{
    DesktopServices, InitialView,
    web_prototype::{PrototypeBackend, ROOT},
};
use lapis_desktop_client::{
    ConversationSession, EditorSession, GitSession, LspSession, SettingsSession, TaskSession,
    TerminalSession, WorkspaceSearchSession,
};

thread_local! {
    static APPLICATION: RefCell<Option<gpui_preview::ApplicationHandle>> = const { RefCell::new(None) };
    static READY: Cell<bool> = const { Cell::new(false) };
}

#[wasm_bindgen::prelude::wasm_bindgen]
pub fn is_ready() -> bool {
    READY.with(Cell::get)
}

#[wasm_bindgen::prelude::wasm_bindgen]
pub fn start(backend_preference: &str) {
    console_error_panic_hook::set_once();
    let backend = Arc::new(PrototypeBackend::new());
    let mut session = EditorSession::new_empty(
        backend.clone(),
        backend.clone(),
        backend.clone(),
        backend.clone(),
    );
    session
        .open_workspace(ROOT.into())
        .expect("sample workspace");
    session
        .open_path(Path::new(ROOT).join("README.md"))
        .expect("sample document");
    let conversation = ConversationSession::new(backend.clone(), session.snapshot());
    let settings = SettingsSession::load(backend.clone()).expect("prototype settings");
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
}
