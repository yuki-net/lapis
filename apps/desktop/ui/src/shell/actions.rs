use super::*;
use crate::extension_ui::PanelPosition;

impl Editor {
    pub(super) fn toggle_preview(
        &mut self,
        _: &TogglePreview,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_panel(PanelPosition::Right, Some(ViewId::new(id::VIEW_PREVIEW)));
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
        self.open_panel(PanelPosition::Right, Some(ViewId::new(id::VIEW_ASSISTANT)));
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

    pub(super) fn open_panel(&mut self, position: PanelPosition, view: Option<ViewId>) {
        let panel = self.shell.panel_mut(position);
        panel.open = true;
        if let Some(view) = view {
            panel.activate_tool(view);
        }
    }

    pub(super) fn toggle_panel(&mut self, position: PanelPosition) {
        let panel = self.shell.panel_mut(position);
        panel.open = !panel.open;
    }

    pub(super) fn toggle_header_panel(&mut self, position: PanelPosition, cx: &mut Context<Self>) {
        self.toggle_panel(position);
        self.shell.command_palette_open = false;
        self.refresh_feature_activation();
        cx.notify();
    }

    pub(super) fn open_tool_picker(&mut self, position: PanelPosition, cx: &mut Context<Self>) {
        self.shell.tool_picker = Some(position);
        self.shell.set_tool_picker_query(String::new());
        cx.notify();
    }

    pub(super) fn select_panel_tab(
        &mut self,
        position: PanelPosition,
        tab: PanelTab,
        cx: &mut Context<Self>,
    ) {
        self.shell.panel_mut(position).activate(tab.clone());
        if let PanelTab::Tool(view) = tab {
            self.shell.activate_view(position, view);
        }
        self.refresh_feature_activation();
        cx.notify();
    }

    pub(super) fn close_tool_picker(&mut self, cx: &mut Context<Self>) {
        if self.shell.tool_picker.take().is_some() {
            self.shell.set_tool_picker_query(String::new());
            cx.notify();
        }
    }

    pub(super) fn select_tool_from_picker(
        &mut self,
        position: PanelPosition,
        view: ViewId,
        cx: &mut Context<Self>,
    ) {
        self.shell.activate_view(position, view);
        self.shell.tool_picker = None;
        self.shell.set_tool_picker_query(String::new());
        self.refresh_feature_activation();
        cx.notify();
    }

    pub(super) fn tool_picker_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.shell.tool_picker.is_none() {
            return;
        }

        let key = event.keystroke.key.as_str();
        if key == "escape" {
            self.close_tool_picker(cx);
            return;
        }
        if key == "backspace" {
            self.shell.tool_picker_query.pop();
            cx.notify();
            return;
        }
        if key == "enter" {
            if let Some(position) = self.shell.tool_picker
                && let Some(view) = self
                    .feature_registry
                    .panel_contributions(position)
                    .into_iter()
                    .find(|contribution| self.tool_matches(contribution))
                    .and_then(|contribution| contribution.view.clone())
            {
                self.select_tool_from_picker(position, view, cx);
            }
            return;
        }
        if event.keystroke.modifiers.control
            || event.keystroke.modifiers.alt
            || event.keystroke.modifiers.platform
        {
            return;
        }
        if let Some(character) = event
            .keystroke
            .key
            .chars()
            .next()
            .filter(|_| key.len() == 1)
        {
            self.shell.tool_picker_query.push(character);
            window.prevent_default();
            cx.notify();
        }
    }

    fn tool_matches(&self, contribution: &crate::extension_ui::UiContribution) -> bool {
        let query = self.shell.tool_picker_query.trim().to_lowercase();
        if query.is_empty() {
            return true;
        }
        let title = self.locale.resolve(&contribution.title).to_lowercase();
        let view = contribution
            .view
            .as_ref()
            .map(|view| view.as_str())
            .unwrap_or_default()
            .to_lowercase();
        title.contains(&query) || view.contains(&query)
    }

    pub(super) fn move_panel_tab(
        &mut self,
        source: PanelPosition,
        target: PanelPosition,
        tab: PanelTab,
        cx: &mut Context<Self>,
    ) {
        if !self.shell.panel(target).open && target != PanelPosition::Main {
            return;
        }
        self.shell.move_tab(source, target, tab);
        self.refresh_feature_activation();
        cx.notify();
    }
}
