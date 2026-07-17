use std::sync::Arc;

use lapis_app_services::EditorSession;
use lapis_platform::{LocalDocumentRepository, NativeMarkdownFileDialog};

fn main() {
    let session = EditorSession::new(
        Arc::new(LocalDocumentRepository),
        Arc::new(NativeMarkdownFileDialog),
    );
    lapis_desktop_ui::run(session);
}
