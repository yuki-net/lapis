use super::*;
use crate::extension_ui::PanelPosition;

impl Shell {
    pub(super) fn group_visibility_entries(&self) -> Vec<(super::super::TabGroupId, String, bool)> {
        self.shell
            .layout
            .leaf_group_ids()
            .into_iter()
            .filter_map(|id| {
                let group = self.shell.layout.group(&id)?;
                let zone = match group.position {
                    PanelPosition::Left => self.t("panel.left"),
                    PanelPosition::Bottom => self.t("panel.bottom"),
                    PanelPosition::Right => self.t("panel.right"),
                    PanelPosition::Main => return None,
                };
                let title = group
                    .active
                    .as_ref()
                    .or_else(|| group.tabs.first())
                    .map(|tab| self.tab_title(group.position, tab))
                    .unwrap_or_else(|| self.t("panel.empty"));
                let open = group.requested_open();
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
        self.open_panel(
            PanelPosition::Right,
            Some(ViewId::new(id::VIEW_PREVIEW)),
            cx,
        );
        self.shell.command_palette_open = false;
        self.refresh_feature_activation(cx);
        cx.notify();
    }

    pub(crate) fn toggle_bottom_panel(
        &mut self,
        _: &ToggleBottomPanel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_panel(PanelPosition::Bottom, cx);
        window.focus(&self.focus_handle);
        self.shell.command_palette_open = false;
        self.refresh_feature_activation(cx);
        cx.notify();
    }

    pub(crate) fn toggle_assistant(
        &mut self,
        _: &ToggleAssistant,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_panel(
            PanelPosition::Right,
            Some(ViewId::new(id::VIEW_ASSISTANT)),
            cx,
        );
        self.shell.command_palette_open = false;
        self.refresh_feature_activation(cx);
        cx.notify();
    }

    pub(crate) fn open_panel(
        &mut self,
        position: PanelPosition,
        view: Option<ViewId>,
        cx: &mut Context<Self>,
    ) {
        self.request_panel_open(position, true, cx);
        if let Some(view) = view {
            self.shell
                .panel_mut(position)
                .activate_tool_without_open(view);
        }
    }

    pub(crate) fn toggle_panel(&mut self, position: PanelPosition, cx: &mut Context<Self>) {
        if position == PanelPosition::Main {
            return;
        }
        let open = !self.shell.layout.zone_open(position);
        let transitions = self.shell.layout.request_zone_open(
            position,
            open,
            crate::shell::panel_transition::PANEL_ANIMATION_DURATION,
            std::time::Instant::now(),
        );
        for (group, generation, duration) in transitions {
            self.schedule_group_transition(group, generation, duration, cx);
        }
        if !open
            && self
                .shell
                .layout
                .group(&self.shell.focused_group)
                .is_some_and(|group| group.position == position)
        {
            self.shell.focused_group = self
                .shell
                .layout
                .primary_group_id(PanelPosition::Main)
                .clone();
        }
    }

    pub(super) fn toggle_group_visibility(
        &mut self,
        group: super::super::TabGroupId,
        cx: &mut Context<Self>,
    ) {
        let Some(panel) = self.shell.layout.group(&group) else {
            return;
        };
        let open = !panel.requested_open();
        if let Ok(Some((generation, duration))) = self.shell.layout.request_group_open(
            &group,
            open,
            crate::shell::panel_transition::PANEL_ANIMATION_DURATION,
            std::time::Instant::now(),
        ) {
            self.schedule_group_transition(group.clone(), generation, duration, cx);
        }
        if !open && self.shell.focused_group == group {
            self.shell.focused_group = self
                .shell
                .layout
                .primary_group_id(PanelPosition::Main)
                .clone();
        }
        self.refresh_feature_activation(cx);
        cx.notify();
    }

    fn schedule_group_transition(
        &mut self,
        group: super::super::TabGroupId,
        generation: u64,
        duration: Duration,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            Timer::after(duration).await;
            let _ = this.update(cx, |shell, cx| {
                let next = shell.shell.layout.group_mut(&group).and_then(|panel| {
                    panel.complete_transition(generation, std::time::Instant::now())
                });
                if let Some((generation, duration)) = next {
                    shell.schedule_group_transition(group, generation, duration, cx);
                }
                shell.refresh_feature_activation(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn toggle_header_panel(
        &mut self,
        position: PanelPosition,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_panel(position, cx);
        window.focus(&self.focus_handle);
        self.shell.command_palette_open = false;
        self.refresh_feature_activation(cx);
        cx.notify();
    }

    pub(crate) fn activate_panel_view(
        &mut self,
        position: PanelPosition,
        view: ViewId,
        cx: &mut Context<Self>,
    ) {
        self.request_panel_open(position, true, cx);
        self.shell
            .panel_mut(position)
            .activate_tool_without_open(view);
        self.refresh_feature_activation(cx);
        cx.notify();
    }

    pub(crate) fn close_panel(&mut self, position: PanelPosition, cx: &mut Context<Self>) {
        if position == PanelPosition::Main {
            return;
        }
        self.shell.panel_mut(position).active = None;
        self.request_panel_open(position, false, cx);
        self.refresh_feature_activation(cx);
        cx.notify();
    }

    pub(crate) fn close_empty_panel(
        &mut self,
        position: PanelPosition,
        cx: &mut Context<Self>,
    ) -> bool {
        if position == PanelPosition::Main || !self.shell.panel(position).tabs.is_empty() {
            return false;
        }
        self.close_panel(position, cx);
        true
    }

    pub(crate) fn request_panel_open_with_duration(
        &mut self,
        position: PanelPosition,
        open: bool,
        duration: Duration,
        cx: &mut Context<Self>,
    ) {
        let transition = self.shell.panel_mut(position).request_open_with_duration(
            open,
            duration,
            std::time::Instant::now(),
        );
        if let Some((generation, duration)) = transition {
            self.schedule_panel_transition(position, generation, duration, cx);
        }
    }

    fn request_panel_open(&mut self, position: PanelPosition, open: bool, cx: &mut Context<Self>) {
        self.request_panel_open_with_duration(
            position,
            open,
            crate::shell::panel_transition::PANEL_ANIMATION_DURATION,
            cx,
        );
    }

    fn schedule_panel_transition(
        &mut self,
        position: PanelPosition,
        generation: u64,
        duration: Duration,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            Timer::after(duration).await;
            let _ = this.update(cx, |editor, cx| {
                editor.complete_panel_transition(position, generation, cx);
            });
        })
        .detach();
    }

    fn complete_panel_transition(
        &mut self,
        position: PanelPosition,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        let next = self
            .shell
            .panel_mut(position)
            .complete_transition(generation, std::time::Instant::now());
        if let Some((next_generation, duration)) = next {
            self.schedule_panel_transition(position, next_generation, duration, cx);
        }
        self.refresh_feature_activation(cx);
        cx.notify();
    }

    pub(crate) fn open_tool_picker(
        &mut self,
        group: super::super::TabGroupId,
        anchor: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.shell.layout.group(&group).is_none() {
            return;
        }
        self.shell.focused_group = group.clone();
        self.shell.tool_picker = Some(group);
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
        self.open_panel(
            PanelPosition::Main,
            Some(ViewId::new(id::VIEW_SETTINGS)),
            cx,
        );
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
    pub(crate) fn select_panel_tab_in_group(
        &mut self,
        group: TabGroupId,
        position: PanelPosition,
        tab: PanelTab,
        cx: &mut Context<Self>,
    ) {
        if self.shell.layout.primary_group_id(position) == &group {
            self.request_panel_open(position, true, cx);
        }
        self.shell.open_group_tab(&group, tab);
        self.refresh_feature_activation(cx);
        cx.notify();
    }

    pub(crate) fn close_panel_tab_in_group(
        &mut self,
        group: TabGroupId,
        position: PanelPosition,
        tab: PanelTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let PanelTab::Document(document_id) = tab else {
            if let Some(group) = self.shell.layout.group_for_tab(&tab) {
                self.shell.layout.group_mut(&group).unwrap().remove(&tab);
                self.feature_registry
                    .close_view_instance(&tab.instance_id());
            }
            if self.shell.layout.primary_group_id(position) != &group
                || !self.close_empty_panel(position, cx)
            {
                self.refresh_feature_activation(cx);
                cx.notify();
            }
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
        group: super::super::TabGroupId,
        view: ViewId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.shell.layout.group(&group) else {
            self.close_tool_picker(window, cx);
            return;
        };
        let position = target.position;
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
        if !self.shell.open_group_tab(&group, tab) {
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
            if let Some(group) = self.shell.tool_picker.clone()
                && let Some((view, _)) = self
                    .picker_views(&group)
                    .get(self.shell.tool_picker_selected)
            {
                self.select_tool_from_picker(group, view.clone(), window, cx);
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
        group: &super::super::TabGroupId,
    ) -> Vec<(ViewId, crate::extension_ui::ToolInstancePolicy)> {
        self.picker_contributions(group)
            .into_iter()
            .filter_map(|contribution| {
                Some((contribution.view.clone()?, contribution.instance_policy))
            })
            .collect()
    }

    pub(super) fn picker_contributions(
        &self,
        group: &super::super::TabGroupId,
    ) -> Vec<&crate::extension_ui::UiContribution> {
        let Some(target) = self.shell.layout.group(group) else {
            return Vec::new();
        };
        self.feature_registry
            .tool_contributions(target.position)
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

    pub(crate) fn move_panel_tab_to_group(
        &mut self,
        source: TabGroupId,
        target: TabGroupId,
        tab: PanelTab,
        zone: DropZone,
        cx: &mut Context<Self>,
    ) {
        let target_size = self
            .group_bounds
            .borrow()
            .get(&target)
            .map(|bounds| (f32::from(bounds.size.width), f32::from(bounds.size.height)))
            .unwrap_or((0.0, 0.0));
        match self
            .shell
            .layout
            .drop_tab(&source, &target, &tab, zone, target_size)
        {
            Ok(()) => {
                if let Some(group) = self.shell.layout.group_for_tab(&tab) {
                    self.shell.focused_group = group;
                }
                self.refresh_feature_activation(cx);
                cx.notify();
            }
            Err(error) => {
                self.status = match error {
                    LayoutError::InsufficientSpace => "分割できる最小領域を確保できません".into(),
                    LayoutError::SideColumnLimitReached => "サイド領域は通常2列までです".into(),
                    LayoutError::TabNotFound => "移動元のタブが見つかりません".into(),
                    _ => "この位置にはタブを配置できません".into(),
                };
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
    fn committed_drop_focuses_the_group_that_contains_the_tab(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            for zone in [
                DropZone::Center,
                DropZone::Left,
                DropZone::Right,
                DropZone::Top,
                DropZone::Bottom,
            ] {
                let document = cx.new(|_| DocumentFixture);
                let shell = cx.new(|cx| Shell::new(document.into(), HashMap::new(), cx));
                shell.update(cx, |shell, cx| {
                    let source = shell
                        .shell
                        .layout
                        .primary_group_id(PanelPosition::Left)
                        .clone();
                    let target = shell
                        .shell
                        .layout
                        .primary_group_id(PanelPosition::Main)
                        .clone();
                    let tab = shell.shell.layout.group(&source).unwrap().tabs[0].clone();
                    shell.group_bounds.borrow_mut().insert(
                        target.clone(),
                        gpui::Bounds::new(
                            point(px(0.0), px(0.0)),
                            gpui::size(px(900.0), px(600.0)),
                        ),
                    );
                    shell.move_panel_tab_to_group(source, target.clone(), tab.clone(), zone, cx);
                    let actual = shell.shell.layout.group_for_tab(&tab).unwrap();
                    assert_eq!(shell.shell.focused_group, actual);
                    if zone != DropZone::Center {
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
                    assert!(shell.shell.layout.validate().is_ok());
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
    fn header_zone_and_view_menu_keep_individual_groups_recoverable(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let document = cx.new(|_| Empty);
            let shell = cx.new(|cx| Shell::new(document.into(), HashMap::new(), cx));
            shell.update(cx, |shell, cx| {
                let a = shell
                    .shell
                    .layout
                    .primary_group_id(PanelPosition::Left)
                    .clone();
                let b = shell
                    .shell
                    .layout
                    .split_group(&a, crate::shell::layout::SplitDirection::Horizontal, 0.5)
                    .unwrap();
                let tab = PanelTab::tool_instance("terminal", "visible-terminal");
                shell
                    .shell
                    .layout
                    .group_mut(&b)
                    .unwrap()
                    .activate(tab.clone());
                let placements = shell.shell.layout.placements();
                shell.shell.focused_group = b.clone();
                shell.toggle_group_visibility(b.clone(), cx);
                assert_ne!(shell.shell.focused_group, b);
                shell.toggle_panel(PanelPosition::Left, cx);
                assert!(!shell.shell.layout.zone_open(PanelPosition::Left));
                shell.toggle_panel(PanelPosition::Left, cx);
                assert!(shell.shell.layout.group(&a).unwrap().requested_open());
                assert!(!shell.shell.layout.group(&b).unwrap().requested_open());
                shell.toggle_group_visibility(a.clone(), cx);
                assert!(!shell.shell.layout.zone_open(PanelPosition::Left));
                let entries = shell.group_visibility_entries();
                assert!(entries.iter().any(|(id, _, open)| id == &a && !open));
                assert!(entries.iter().any(|(id, _, open)| id == &b && !open));
                shell.toggle_group_visibility(b.clone(), cx);
                assert!(shell.shell.layout.group(&b).unwrap().requested_open());
                assert!(!shell.shell.layout.group(&a).unwrap().requested_open());
                assert_eq!(shell.shell.layout.placements(), placements);
                assert!(shell.shell.layout.validate().is_ok());
            });
        });
    }
}
