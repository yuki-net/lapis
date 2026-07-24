use super::*;
use crate::extension_ui::PanelPosition;

impl Editor {
    pub(super) fn toggle_preview(
        &mut self,
        _: &TogglePreview,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_view(PanelPosition::Right, ViewId::new(id::VIEW_PREVIEW));
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
        self.toggle_panel(PanelPosition::Bottom);
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
        self.toggle_view(PanelPosition::Right, ViewId::new(id::VIEW_ASSISTANT));
        self.shell.command_palette_open = false;
        self.refresh_feature_activation();
        cx.notify();
    }

    pub(super) fn select_tool(&mut self, view: ViewId, cx: &mut Context<Self>) {
        self.select_view(PanelPosition::Left, view, cx);
    }

    pub(super) fn select_view(
        &mut self,
        position: PanelPosition,
        view: ViewId,
        cx: &mut Context<Self>,
    ) {
        self.shell.activate_view(position, view);
        self.refresh_feature_activation();
        cx.notify();
    }

    pub(super) fn toggle_panel(&mut self, position: PanelPosition) {
        if let Some(panel) = self.shell.panel_mut(position) {
            if panel.open {
                panel.close();
            } else {
                panel.open = true;
            }
        }
    }

    pub(super) fn toggle_view(&mut self, position: PanelPosition, view: ViewId) {
        let Some(panel) = self.shell.panel_mut(position) else {
            return;
        };
        if panel.open && panel.active.as_ref() == Some(&view) {
            panel.close();
        } else {
            panel.activate(view);
        }
    }
}
