use super::*;
use crate::extension_ui::ZonePosition;

impl Shell {
    pub(super) fn panel_visibility_entries(&self) -> Vec<(super::super::PanelId, String, bool)> {
        self.shell
            .layout
            .panel_ids()
            .into_iter()
            .filter_map(|id| {
                let panel = self.shell.layout.panel(&id)?;
                let position = self.shell.layout.zone_for_panel(&id)?;
                let zone = match position {
                    ZonePosition::Left => self.t("panel.left"),
                    ZonePosition::Bottom => self.t("panel.bottom"),
                    ZonePosition::Right => self.t("panel.right"),
                    ZonePosition::Main => return None,
                };
                let title = panel
                    .active
                    .as_ref()
                    .or_else(|| panel.tabs.first())
                    .map(|tab| self.tab_title(position, tab))
                    .unwrap_or_else(|| self.t("panel.empty"));
                let open = panel.requested_open();
                let action = self.t(if open { "panel.hide" } else { "panel.show" });
                Some((id, format!("{action} {zone} · {title}"), open))
            })
            .collect()
    }
    pub(crate) fn toggle_preview(
        &mut self,
        _: &TogglePreview,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_zone(ZonePosition::Right, Some(ViewId::new(id::VIEW_PREVIEW)), cx);
        self.shell.command_palette_open = false;
        self.refresh_feature_activation(cx);
        cx.notify();
    }

    pub(crate) fn toggle_bottom_zone(
        &mut self,
        _: &ToggleBottomZone,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command(id::COMMAND_TOGGLE_BOTTOM_ZONE.into(), window, cx);
    }

    pub(crate) fn toggle_left_zone(
        &mut self,
        _: &ToggleLeftZone,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command(id::COMMAND_TOGGLE_LEFT_ZONE.into(), window, cx);
    }

    pub(crate) fn toggle_right_zone(
        &mut self,
        _: &ToggleRightZone,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command(id::COMMAND_TOGGLE_RIGHT_ZONE.into(), window, cx);
    }

    pub(crate) fn toggle_assistant(
        &mut self,
        _: &ToggleAssistant,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_zone(
            ZonePosition::Right,
            Some(ViewId::new(id::VIEW_ASSISTANT)),
            cx,
        );
        self.shell.command_palette_open = false;
        self.refresh_feature_activation(cx);
        cx.notify();
    }

    pub(crate) fn open_zone(
        &mut self,
        position: ZonePosition,
        view: Option<ViewId>,
        cx: &mut Context<Self>,
    ) {
        if let Some(view) = view {
            let _ = self.shell.layout.move_tab_to_zone(
                position,
                PanelTab::tool(view),
                std::time::Instant::now(),
            );
        } else {
            self.request_zone_open(position, true, cx);
        }
    }

    pub(crate) fn toggle_zone(&mut self, position: ZonePosition, cx: &mut Context<Self>) {
        if position == ZonePosition::Main {
            return;
        }
        let open = !self.shell.layout.zone_open(position);
        self.shell.layout.set_zone_open(position, open);
        if !open
            && self
                .shell
                .focused_panel
                .as_ref()
                .and_then(|panel| self.shell.layout.zone_for_panel(panel))
                .is_some_and(|zone| zone == position)
        {
            self.shell.focused_panel = None;
        }
        cx.notify();
    }

    pub(super) fn toggle_panel_visibility(
        &mut self,
        panel_id: super::super::PanelId,
        cx: &mut Context<Self>,
    ) {
        let Some(panel) = self.shell.layout.panel(&panel_id) else {
            return;
        };
        let open = !panel.requested_open();
        self.shell.layout.set_panel_open(&panel_id, open);
        if !open && self.shell.focused_panel.as_ref() == Some(&panel_id) {
            self.shell.focused_panel = None;
        }
        self.refresh_feature_activation(cx);
        cx.notify();
    }

    pub(super) fn execute_toggle_zone(
        &mut self,
        position: ZonePosition,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_zone(position, cx);
        window.focus(&self.focus_handle);
        self.shell.command_palette_open = false;
        self.refresh_feature_activation(cx);
        cx.notify();
    }

    pub(crate) fn activate_panel_view(
        &mut self,
        position: ZonePosition,
        view: ViewId,
        cx: &mut Context<Self>,
    ) {
        let _ = self.shell.layout.move_tab_to_zone(
            position,
            PanelTab::tool(view),
            std::time::Instant::now(),
        );
        self.refresh_feature_activation(cx);
        cx.notify();
    }

    pub(crate) fn close_zone(&mut self, position: ZonePosition, cx: &mut Context<Self>) {
        if position == ZonePosition::Main {
            return;
        }
        self.request_zone_open(position, false, cx);
        self.refresh_feature_activation(cx);
        cx.notify();
    }

    pub(crate) fn request_zone_open_with_duration(
        &mut self,
        position: ZonePosition,
        open: bool,
        _duration: Duration,
        cx: &mut Context<Self>,
    ) {
        self.shell.layout.set_zone_open(position, open);
        cx.notify();
    }

    fn request_zone_open(&mut self, position: ZonePosition, open: bool, cx: &mut Context<Self>) {
        self.request_zone_open_with_duration(
            position,
            open,
            crate::shell::panel_transition::PANEL_ANIMATION_DURATION,
            cx,
        );
    }

    pub(crate) fn open_tool_picker(
        &mut self,
        panel_id: super::super::PanelId,
        anchor: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.shell.layout.panel(&panel_id).is_none() {
            return;
        }
        self.shell.focused_panel = Some(panel_id.clone());
        self.shell.tool_picker = Some(PickerTarget::Panel(panel_id));
        self.shell.tool_picker_anchor = anchor;
        self.shell.set_tool_picker_query(String::new());
        window.focus(&self.focus_handle);
        cx.notify();
    }

    pub(super) fn open_empty_zone_picker(
        &mut self,
        position: ZonePosition,
        anchor: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.shell.layout.zone_open(position) || self.shell.layout.zone_has_panels(position) {
            return;
        }
        self.shell.tool_picker = Some(PickerTarget::Zone(position));
        self.shell.tool_picker_anchor = anchor;
        self.shell.set_tool_picker_query(String::new());
        window.focus(&self.focus_handle);
        cx.notify();
    }

    pub(crate) fn toggle_settings_menu(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.shell.settings_menu_open = !self.shell.settings_menu_open;
        self.shell.settings_menu_anchor = position;
        self.shell.tool_picker = None;
        if !self.shell.settings_menu_open {
            self.shell.theme_picker_open = false;
        }
        cx.notify();
    }

    pub(crate) fn close_settings_menu(&mut self, cx: &mut Context<Self>) {
        if self.shell.settings_menu_open {
            self.shell.settings_menu_open = false;
            self.shell.theme_picker_open = false;
            cx.notify();
        }
    }

    pub(crate) fn settings_menu_key_down(
        &mut self,
        event: &KeyDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.shell.settings_menu_open && event.keystroke.key == "escape" {
            self.close_settings_menu(cx);
        }
    }

    pub(crate) fn open_settings_view(&mut self, cx: &mut Context<Self>) {
        self.open_zone(ZonePosition::Main, Some(ViewId::new(id::VIEW_SETTINGS)), cx);
        self.shell.settings_menu_open = false;
        self.shell.theme_picker_open = false;
        self.refresh_feature_activation(cx);
        cx.notify();
    }

    pub(crate) fn toggle_theme_preference(&mut self, cx: &mut Context<Self>) {
        if self.shell.theme_save_in_flight {
            return;
        }
        self.shell.theme_picker_open = !self.shell.theme_picker_open;
        cx.notify();
    }

    pub(crate) fn select_theme(&mut self, theme: ThemeId, cx: &mut Context<Self>) {
        self.shell.theme_picker_open = false;
        self.shell.settings_menu_open = false;
        cx.emit(UiCommand::Theme(theme));
        cx.notify();
    }
    pub(crate) fn select_tab_in_panel(
        &mut self,
        panel_id: PanelId,
        position: ZonePosition,
        tab: PanelTab,
        cx: &mut Context<Self>,
    ) {
        self.shell.layout.set_panel_open(&panel_id, true);
        self.request_zone_open(position, true, cx);
        self.shell.open_panel_tab(&panel_id, tab);
        self.refresh_feature_activation(cx);
        cx.notify();
    }

    pub(crate) fn close_tab_in_panel(
        &mut self,
        _panel: PanelId,
        _position: ZonePosition,
        tab: PanelTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let PanelTab::Document(document_id) = tab else {
            if let Some(panel_id) = self.shell.layout.panel_for_tab(&tab) {
                self.shell
                    .layout
                    .remove_tab(&panel_id, &tab, std::time::Instant::now());
                self.schedule_empty_close(panel_id, cx);
                self.feature_registry
                    .close_view_instance(&tab.instance_id());
            }
            self.refresh_feature_activation(cx);
            cx.notify();
            return;
        };

        self.request_document_close(document_id, window, cx);
    }

    pub(crate) fn toggle_panel_controls(&mut self, cx: &mut Context<Self>) {
        self.shell.show_panel_controls = !self.shell.show_panel_controls;
        cx.notify();
    }

    pub(crate) fn close_tool_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.shell.tool_picker.take().is_some() {
            self.shell.set_tool_picker_query(String::new());
            window.focus(&self.focus_handle);
            cx.notify();
        }
    }

    pub(crate) fn select_tool_from_picker(
        &mut self,
        target: PickerTarget,
        view: ViewId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(position) = self.picker_position(&target) else {
            self.close_tool_picker(window, cx);
            return;
        };
        let Some(instance_policy) = self
            .feature_registry
            .tool_contributions(position)
            .into_iter()
            .find(|contribution| contribution.view.as_ref() == Some(&view))
            .map(|contribution| contribution.instance_policy)
        else {
            return;
        };
        let tab = match instance_policy {
            crate::extension_ui::ToolInstancePolicy::Shared => PanelTab::tool(view.clone()),
            crate::extension_ui::ToolInstancePolicy::Multiple => {
                let Ok(instance) = self.shell.next_tool_instance(&view) else {
                    return;
                };
                PanelTab::tool_instance(view.clone(), instance)
            }
        };
        if !self
            .feature_registry
            .can_create_view(&view, instance_policy)
        {
            return;
        }
        let opened = match target {
            PickerTarget::Panel(panel_id) => self.shell.open_panel_tab(&panel_id, tab),
            PickerTarget::Zone(position) => self
                .shell
                .layout
                .move_tab_to_zone(position, tab, std::time::Instant::now())
                .map(|id| {
                    self.shell.focused_panel = Some(id);
                })
                .is_ok(),
        };
        if !opened {
            return;
        }
        self.synchronize_view_instances(cx);
        self.shell.tool_picker = None;
        self.shell.set_tool_picker_query(String::new());
        window.focus(&self.focus_handle);
        self.refresh_feature_activation(cx);
        cx.notify();
    }

    pub(crate) fn tool_picker_key_down(
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
            self.close_tool_picker(window, cx);
            return;
        }
        if key == "backspace" {
            let mut query = self.shell.tool_picker_query.clone();
            query.pop();
            self.shell.set_tool_picker_query(query);
            cx.notify();
            return;
        }
        if key == "enter" {
            if let Some(panel_id) = self.shell.tool_picker.clone()
                && let Some((view, _)) = self
                    .picker_views(&panel_id)
                    .get(self.shell.tool_picker_selected)
            {
                self.select_tool_from_picker(panel_id, view.clone(), window, cx);
            }
            return;
        }
        if matches!(key, "down" | "arrowdown" | "up" | "arrowup") {
            let count = self
                .picker_views(self.shell.tool_picker.as_ref().unwrap())
                .len();
            if count > 0 {
                self.shell.tool_picker_selected = if matches!(key, "down" | "arrowdown") {
                    (self.shell.tool_picker_selected + 1) % count
                } else {
                    self.shell
                        .tool_picker_selected
                        .checked_sub(1)
                        .unwrap_or(count - 1)
                };
                window.prevent_default();
                cx.notify();
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
            let mut query = self.shell.tool_picker_query.clone();
            query.push(character);
            self.shell.set_tool_picker_query(query);
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

    fn picker_views(
        &self,
        target: &PickerTarget,
    ) -> Vec<(ViewId, crate::extension_ui::ToolInstancePolicy)> {
        self.picker_contributions(target)
            .into_iter()
            .filter_map(|contribution| {
                Some((contribution.view.clone()?, contribution.instance_policy))
            })
            .collect()
    }

    pub(super) fn picker_contributions(
        &self,
        target: &PickerTarget,
    ) -> Vec<&crate::extension_ui::UiContribution> {
        let Some(position) = self.picker_position(target) else {
            return Vec::new();
        };
        self.feature_registry
            .tool_contributions(position)
            .into_iter()
            .filter(|contribution| self.tool_matches(contribution))
            .filter(|contribution| {
                contribution.view.as_ref().is_some_and(|view| {
                    self.feature_registry
                        .can_create_view(view, contribution.instance_policy)
                })
            })
            .collect()
    }

    fn picker_position(&self, target: &PickerTarget) -> Option<ZonePosition> {
        match target {
            PickerTarget::Panel(panel_id) => self.shell.layout.zone_for_panel(panel_id),
            PickerTarget::Zone(position) => Some(*position),
        }
    }

    pub(crate) fn schedule_empty_close(&mut self, panel_id: PanelId, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            Timer::after(crate::shell::panel_transition::PANEL_ANIMATION_DURATION).await;
            let _ = this.update(cx, |shell, cx| {
                shell.shell.layout.finish_empty_close(&panel_id);
                shell.refresh_feature_activation(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn drop_panel_tab(
        &mut self,
        source: PanelId,
        tab: PanelTab,
        target: PanelDrop,
        cx: &mut Context<Self>,
    ) {
        let size = self.target_zone_size(&target);
        match self
            .shell
            .layout
            .drop_tab(&source, &tab, &target, size, std::time::Instant::now())
        {
            Ok(destination) => {
                self.shell.focused_panel = Some(destination);
                if self
                    .shell
                    .layout
                    .panel(&source)
                    .is_some_and(|panel| panel.tabs.is_empty())
                {
                    self.schedule_empty_close(source, cx);
                }
                self.refresh_feature_activation(cx);
                cx.notify();
            }
            Err(()) => {
                self.status = "この位置にはタブを配置できません".into();
                cx.notify();
            }
        }
    }
}

#[cfg(test)]
mod focus_tests {
    use super::*;
    struct DocumentFixture;
    impl Render for DocumentFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    #[gpui::test]
    fn committed_drop_focuses_the_panel_that_contains_the_tab(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            for (position, edge) in [
                (ZonePosition::Main, None),
                (ZonePosition::Left, Some(PanelEdge::Right)),
                (ZonePosition::Right, Some(PanelEdge::Left)),
                (ZonePosition::Bottom, Some(PanelEdge::Top)),
            ] {
                let document = cx.new(|_| DocumentFixture);
                let shell = cx.new(|cx| Shell::new(document.into(), HashMap::new(), cx));
                shell.update(cx, |shell, cx| {
                    let source = shell
                        .shell
                        .layout
                        .open_tab(ZonePosition::Main, PanelTab::tool("source"))
                        .unwrap();
                    let target = shell
                        .shell
                        .layout
                        .open_tab(position, PanelTab::tool("target"))
                        .unwrap();
                    let tab = PanelTab::tool("source");
                    shell.zone_bounds.borrow_mut().insert(
                        position,
                        gpui::Bounds::new(
                            point(px(0.0), px(0.0)),
                            gpui::size(px(900.0), px(600.0)),
                        ),
                    );
                    let drop = edge.map_or_else(
                        || PanelDrop::Into(target.clone()),
                        |edge| PanelDrop::Edge {
                            panel: target.clone(),
                            edge,
                        },
                    );
                    shell.drop_panel_tab(source, tab.clone(), drop, cx);
                    let actual = shell.shell.layout.panel_for_tab(&tab).unwrap();
                    assert_eq!(shell.shell.focused_panel, Some(actual.clone()));
                    if edge.is_some() {
                        assert_ne!(actual, target);
                    }
                    assert_eq!(
                        shell
                            .shell
                            .layout
                            .placements()
                            .iter()
                            .filter(|(placed, _)| placed == &tab)
                            .count(),
                        1
                    );
                    assert!(shell.shell.layout.valid());
                });
            }
        });
    }
}

#[cfg(test)]
mod visibility_tests {
    use super::*;
    struct Empty;
    impl Render for Empty {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    #[gpui::test]
    fn empty_side_zones_reopen_by_command_without_opening_picker(cx: &mut gpui::TestAppContext) {
        let window = cx.add_window(|_, cx| {
            let document = cx.new(|_| Empty);
            Shell::new(document.into(), HashMap::new(), cx)
        });
        window
            .update(cx, |shell, window, cx| {
                for position in [
                    ZonePosition::Left,
                    ZonePosition::Right,
                    ZonePosition::Bottom,
                ] {
                    let tab = PanelTab::tool(format!("fixture-{position:?}"));
                    let id = shell.shell.layout.open_tab(position, tab.clone()).unwrap();
                    let tabs = shell.shell.layout.panel(&id).unwrap().tabs.clone();
                    let now = std::time::Instant::now();
                    for tab in tabs {
                        assert!(shell.shell.layout.remove_tab(&id, &tab, now));
                    }
                    shell.shell.layout.finish_due_closes(
                        now + crate::shell::panel_transition::PANEL_ANIMATION_DURATION,
                    );
                    assert!(!shell.shell.layout.zone_has_panels(position));
                    assert!(!shell.shell.layout.zone_open(position));
                    shell.execute_toggle_zone(position, window, cx);
                    assert!(shell.shell.layout.zone_open(position));
                    assert!(shell.shell.tool_picker.is_none());
                    let first = shell
                        .shell
                        .layout
                        .open_tab(position, PanelTab::tool("one"))
                        .unwrap();
                    let edge = match position {
                        ZonePosition::Left => PanelEdge::Right,
                        ZonePosition::Right => PanelEdge::Left,
                        ZonePosition::Bottom => PanelEdge::Top,
                        ZonePosition::Main => unreachable!(),
                    };
                    for count in 1..=3 {
                        let geometry = crate::shell::dock_geometry::DockGeometry::single_zone(
                            &shell.shell.layout,
                            position,
                            (900.0, 600.0),
                        );
                        let growth = shell.shell.layout.growth_panel(position).unwrap();
                        let rect = geometry.panels[&growth];
                        let point = match edge {
                            PanelEdge::Right => {
                                (rect.x + rect.width - 1.0, rect.y + rect.height / 2.0)
                            }
                            PanelEdge::Left => (rect.x + 1.0, rect.y + rect.height / 2.0),
                            PanelEdge::Top => (rect.x + rect.width / 2.0, rect.y + 1.0),
                        };
                        let target = crate::shell::dock_geometry::DockGeometry::drop_target(
                            &shell.shell.layout,
                            geometry.panels.iter().map(|(id, rect)| (id.clone(), *rect)),
                            geometry.zones.iter().map(|(zone, rect)| (*zone, *rect)),
                            point,
                        )
                        .unwrap();
                        if count == 3 {
                            assert_eq!(target, PanelDrop::Into(growth));
                        } else {
                            assert_eq!(
                                target,
                                PanelDrop::Edge {
                                    panel: growth,
                                    edge
                                }
                            );
                            let tab = PanelTab::tool(format!("extra-{count}"));
                            let source =
                                shell.shell.layout.open_tab(position, tab.clone()).unwrap();
                            shell
                                .shell
                                .layout
                                .drop_tab(&source, &tab, &target, (900.0, 600.0), now)
                                .unwrap();
                        }
                        assert_eq!(
                            shell
                                .shell
                                .layout
                                .panel_ids()
                                .iter()
                                .filter(
                                    |id| shell.shell.layout.zone_for_panel(id) == Some(position)
                                )
                                .count(),
                            count + usize::from(count < 3)
                        );
                    }
                    assert!(shell.shell.layout.panel(&first).is_some());
                    for (tab, id) in shell.shell.layout.placements() {
                        if shell.shell.layout.zone_for_panel(&id) == Some(position) {
                            assert!(shell.shell.layout.remove_tab(&id, &tab, now));
                        }
                    }
                    shell.shell.layout.finish_due_closes(
                        now + crate::shell::panel_transition::PANEL_ANIMATION_DURATION,
                    );
                    assert!(!shell.shell.layout.zone_has_panels(position));
                    shell.execute_toggle_zone(position, window, cx);
                    assert!(shell.shell.layout.zone_open(position));
                    assert!(shell.shell.tool_picker.is_none());
                    shell.execute_toggle_zone(position, window, cx);
                }
                assert!(shell.shell.layout.zone_open(ZonePosition::Main));
                assert_eq!(
                    shell
                        .shell
                        .layout
                        .panel_ids()
                        .iter()
                        .filter(
                            |id| shell.shell.layout.zone_for_panel(id) == Some(ZonePosition::Main)
                        )
                        .count(),
                    1
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn header_zone_and_view_menu_keep_individual_panels_recoverable(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let document = cx.new(|_| Empty);
            let shell = cx.new(|cx| Shell::new(document.into(), HashMap::new(), cx));
            shell.update(cx, |shell, cx| {
                let a = shell
                    .shell
                    .layout
                    .first_panel_in_zone(ZonePosition::Left)
                    .unwrap();
                let tab = PanelTab::tool_instance("terminal", "visible-terminal");
                shell
                    .shell
                    .layout
                    .open_tab(ZonePosition::Left, tab.clone())
                    .unwrap();
                let b = shell
                    .shell
                    .layout
                    .drop_tab(
                        &a,
                        &tab,
                        &PanelDrop::Edge {
                            panel: a.clone(),
                            edge: PanelEdge::Right,
                        },
                        (600.0, 400.0),
                        std::time::Instant::now(),
                    )
                    .unwrap();
                let placements = shell.shell.layout.placements();
                shell.shell.focused_panel = Some(b.clone());
                shell.toggle_panel_visibility(b.clone(), cx);
                assert_ne!(shell.shell.focused_panel, Some(b.clone()));
                shell.toggle_zone(ZonePosition::Left, cx);
                assert!(!shell.shell.layout.zone_open(ZonePosition::Left));
                shell.toggle_zone(ZonePosition::Left, cx);
                assert!(shell.shell.layout.panel(&a).unwrap().requested_open());
                assert!(!shell.shell.layout.panel(&b).unwrap().requested_open());
                shell.toggle_panel_visibility(a.clone(), cx);
                assert!(!shell.shell.layout.zone_open(ZonePosition::Left));
                let entries = shell.panel_visibility_entries();
                assert!(entries.iter().any(|(id, _, open)| id == &a && !open));
                assert!(entries.iter().any(|(id, _, open)| id == &b && !open));
                shell.toggle_panel_visibility(b.clone(), cx);
                assert!(shell.shell.layout.panel(&b).unwrap().requested_open());
                assert!(!shell.shell.layout.panel(&a).unwrap().requested_open());
                assert_eq!(shell.shell.layout.placements(), placements);
                assert!(shell.shell.layout.valid());
            });
        });
    }
}
