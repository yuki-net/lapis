use super::*;

impl Editor {
    pub(super) fn toggle_preview(
        &mut self,
        _: &TogglePreview,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shell.side_panel = if self
            .shell
            .side_panel
            .as_ref()
            .is_some_and(|view| view.as_str() == id::VIEW_PREVIEW)
        {
            None
        } else {
            Some(ViewId::new(id::VIEW_PREVIEW))
        };
        self.shell.command_palette_open = false;
        self.refresh_feature_activation();
        cx.notify();
    }

    pub(super) fn toggle_bottom_panel(
        &mut self,
        _: &ToggleBottomPanel,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shell.bottom_panel_open = !self.shell.bottom_panel_open;
        self.shell.command_palette_open = false;
        self.refresh_feature_activation();
        cx.notify();
    }

    pub(super) fn toggle_assistant(
        &mut self,
        _: &ToggleAssistant,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shell.side_panel = if self
            .shell
            .side_panel
            .as_ref()
            .is_some_and(|view| view.as_str() == id::VIEW_ASSISTANT)
        {
            None
        } else {
            Some(ViewId::new(id::VIEW_ASSISTANT))
        };
        self.shell.command_palette_open = false;
        self.refresh_feature_activation();
        cx.notify();
    }

    pub(super) fn select_tool(&mut self, panel: ViewId, cx: &mut Context<Self>) {
        self.shell.active_tool = panel;
        self.refresh_feature_activation();
        cx.notify();
    }
}
