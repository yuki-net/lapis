//! Desktop（ネイティブ window）の起動 entry。

use desktop_client::EditorSession;
use desktop_ui::{DesktopServices, InitialView};

/// Desktop のイベントループを開始し、最後の Window が閉じた後に終了する。
pub fn run(session: EditorSession, services: DesktopServices, initial_view: InitialView) {
    desktop_ui::init_tracing();
    desktop_ui::desktop_application().run(move |cx| {
        desktop_ui::initialize(cx, &services);
        desktop_ui::open_desktop_window(session, services, initial_view, cx);
        desktop_ui::on_last_window_closed(cx);
    });
}
