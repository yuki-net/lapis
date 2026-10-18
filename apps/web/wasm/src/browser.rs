use super::backend::{PrototypeBackend, ROOT};
use lapis_app_services::{
    ConversationSession, EditorSession, GitSession, LspSession, SettingsSession, TaskSession,
    TerminalSession, WorkspaceSearchSession,
};
use lapis_desktop_ui::{DesktopServices, InitialView};
use std::{
    borrow::Cow,
    cell::{Cell, RefCell},
    path::Path,
    sync::Arc,
};

thread_local! {
    static APPLICATION: RefCell<Option<gpui::ApplicationHandle>> = const { RefCell::new(None) };
    static READY: Cell<bool> = const { Cell::new(false) };
}

#[wasm_bindgen::prelude::wasm_bindgen]
pub fn is_ready() -> bool {
    READY.with(Cell::get)
}

#[wasm_bindgen::prelude::wasm_bindgen]
pub fn start(backend_preference: &str) {
    let backend = Arc::new(PrototypeBackend::new());
    let mut session = EditorSession::new_empty(backend.clone(), backend.clone(), backend.clone());
    session
        .open_workspace(ROOT.into())
        .expect("Sample workspace");
    session
        .open_path(Path::new(ROOT).join("README.md"))
        .expect("Sample document");
    let conversation = ConversationSession::new(backend.clone(), session.snapshot());
    let settings = SettingsSession::load(backend.clone()).expect("Prototype settings");
    let services = DesktopServices::new(
        TaskSession::new(backend.clone()),
        GitSession::new(backend.clone()),
        LspSession::new(backend.clone()),
        TerminalSession::new(backend.clone()),
        WorkspaceSearchSession::new(backend),
        conversation,
        settings,
    );
    let handle = lapis_desktop_ui::run_web(
        session,
        services,
        InitialView::default(),
        vec![
            Cow::Borrowed(include_bytes!("../assets/IBMPlexSans-Regular.ttf")),
            Cow::Borrowed(include_bytes!("../assets/IBMPlexSans-SemiBold.ttf")),
        ],
        backend_preference,
        || READY.with(|ready| ready.set(true)),
    );
    APPLICATION.with(|application| *application.borrow_mut() = Some(handle));
}
