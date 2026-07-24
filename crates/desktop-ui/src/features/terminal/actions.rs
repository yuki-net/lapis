use super::*;

impl Editor {
    pub(super) fn start_terminal(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.session.workspace_root().map(ToOwned::to_owned) else {
            self.status = "Open a workspace before starting a terminal".to_owned();
            cx.notify();
            return;
        };
        match self.terminal.session.start(&root, 120, 30) {
            Ok(_) => {
                self.shell.activate_view(
                    crate::extension_ui::PanelPosition::Bottom,
                    ViewId::new(id::VIEW_TERMINAL),
                );
                self.refresh_feature_activation();
                self.status = "Terminal started".to_owned();
            }
            Err(error) => self.status = format!("Terminal start failed: {error}"),
        }
        cx.notify();
    }

    pub(super) fn send_terminal_clipboard(&mut self, cx: &mut Context<Self>) {
        let Some(terminal) = self.terminal.session.terminals().last() else {
            self.start_terminal(cx);
            return;
        };
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            self.status = "Copy a command, then use Send clipboard".to_owned();
            cx.notify();
            return;
        };
        let mut input = text;
        input.push('\r');
        match self.terminal.session.input(&terminal.id, &input) {
            Ok(()) => self.status = "Sent clipboard to terminal".to_owned(),
            Err(error) => self.status = format!("Terminal input failed: {error}"),
        }
        cx.notify();
    }
}
