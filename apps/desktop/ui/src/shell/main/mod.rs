use super::ShellState;
pub(crate) use crate::tokens;
use crate::{
    app::*,
    commands::UiCommand,
    components::{
        Icon, IconName, ScrollAxis, ScrollState, SurfaceVariant, panel_empty_state,
        panel_empty_state_element, scroll_viewport,
    },
    extension_ui::{ActivationEvent, FeatureRegistry, PanelScrollPolicy, ThemeId, UiSlot, ViewId},
    features::{self, id},
    keymap::KeymapRegistry,
    localization::Localizer,
    theme,
};
use crate::{
    extension_ui::PanelPosition,
    shell::{
        DraggedPanelTab, DropTarget, DropZone, LayoutError, LayoutNode, PanelHost,
        PanelSpanTransition, PanelTab, ResizeMode, ResizeTarget, SplitDirection, TabGroupId,
    },
};
use gpui::{
    AnyView, App, ClickEvent, Context, CursorStyle, ElementId, FocusHandle, Focusable,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point, Render, SharedString,
    Timer, Window, div, point, prelude::*, px, relative,
};
use lapis_workspace::FileEntryKind;
use std::{cell::RefCell, collections::HashMap, time::Duration};

mod actions;
mod command_palette;
mod content;
mod footer;
mod header;
mod layout;
mod overlays;
mod panel_frame;
mod panel_tabs;
mod render;
mod resize;
mod tab_drag;
mod tool_picker;

pub(crate) struct Shell {
    pub(crate) shell: ShellState,
    pub(crate) host: super::window_group::HostId,
    pub(crate) group: Option<gpui::EntityId>,
    pub(crate) detached: bool,
    tab_drag: super::tab_state::TabDragState,
    drag_preview: Option<gpui::AnyWindowHandle>,
    _drag_keys: gpui::Subscription,
    pub(crate) panel_bounds: RefCell<HashMap<PanelPosition, gpui::Bounds<Pixels>>>,
    pub(crate) group_bounds: RefCell<HashMap<TabGroupId, gpui::Bounds<Pixels>>>,
    drop_target: Option<DropTarget>,
    layout_bounds: RefCell<Option<gpui::Bounds<Pixels>>>,
    pub(crate) session: ShellPresentation,
    document_view: AnyView,
    focus_handle: FocusHandle,
    feature_registry: FeatureRegistry,
    locale: Localizer,
    keymap: KeymapRegistry,
    languages: lapis_language::LanguageRegistry,
    scroll_states: ShellScrollStates,
    status: String,
}

#[derive(Default)]
pub(crate) struct ShellPresentation {
    pub(crate) documents: Vec<lapis_desktop_client::DocumentTab>,
    pub(crate) root: Option<std::path::PathBuf>,
    pub(crate) workspace_name: String,
    pub(crate) active: Option<lapis_editor_core::DocumentId>,
    pub(crate) cursor: (usize, usize),
    pub(crate) line_ending: &'static str,
    pub(crate) encoding: lapis_desktop_client::Encoding,
    pub(crate) recent_workspaces: Vec<std::path::PathBuf>,
}
impl ShellPresentation {
    fn tabs(&self) -> Vec<lapis_desktop_client::DocumentTab> {
        self.documents.clone()
    }
    fn workspace_root(&self) -> Option<&std::path::Path> {
        self.root.as_deref()
    }
    fn workspace_name(&self) -> &str {
        &self.workspace_name
    }
    fn active_document_id(&self) -> Option<&lapis_editor_core::DocumentId> {
        self.active.as_ref()
    }
    fn active_path(&self) -> Option<&std::path::Path> {
        self.documents
            .iter()
            .find(|tab| Some(&tab.id) == self.active.as_ref())
            .and_then(|tab| tab.path.as_deref())
    }
    fn display_name(&self) -> String {
        self.documents
            .iter()
            .find(|tab| Some(&tab.id) == self.active.as_ref())
            .map(|tab| tab.display_name.clone())
            .unwrap_or_else(|| "Untitled".into())
    }
    fn encoding(&self) -> lapis_desktop_client::Encoding {
        self.encoding
    }
}

struct ShellScrollStates {
    panel_contents: RefCell<HashMap<PanelTab, ScrollState>>,
    panel_empty: RefCell<HashMap<super::TabGroupId, ScrollState>>,
    panel_tabs: RefCell<HashMap<super::TabGroupId, ScrollState>>,
    tool_picker: ScrollState,
}
impl Default for ShellScrollStates {
    fn default() -> Self {
        Self {
            panel_contents: RefCell::new(HashMap::new()),
            panel_empty: RefCell::new(HashMap::new()),
            panel_tabs: RefCell::new(HashMap::new()),
            tool_picker: ScrollState::new(),
        }
    }
}
impl ShellScrollStates {
    fn panel_content(&self, tab: &PanelTab) -> ScrollState {
        self.panel_contents
            .borrow_mut()
            .entry(tab.clone())
            .or_default()
            .clone()
    }
    fn panel_empty(&self, group: &super::TabGroupId) -> ScrollState {
        self.panel_empty
            .borrow_mut()
            .entry(group.clone())
            .or_default()
            .clone()
    }
    fn panel_tabs(&self, group: &super::TabGroupId) -> ScrollState {
        self.panel_tabs
            .borrow_mut()
            .entry(group.clone())
            .or_default()
            .clone()
    }
}
impl gpui::EventEmitter<UiCommand> for Shell {}

#[cfg(all(test, debug_assertions))]
mod group_scroll_tests {
    use super::*;
    #[test]
    fn tab_groups_have_independent_scroll_state() {
        let states = ShellScrollStates::default();
        let a = super::super::TabGroupId::new(2);
        let b = super::super::TabGroupId::new(5);
        let first = states.panel_tabs(&a);
        let second = states.panel_tabs(&b);
        first.handle().set_offset(point(px(-80.0), px(0.0)));
        assert_eq!(second.handle().offset(), point(px(0.0), px(0.0)));
        assert!(first.shares_state_with(&states.panel_tabs(&a)));
        assert!(!first.shares_state_with(&second));
    }
}
impl Focusable for Shell {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
impl Shell {
    /// Creates a shell around supplied feature views and intercepts Escape to cancel tab drags.
    pub(crate) fn new(
        document_view: AnyView,
        views: HashMap<ViewId, AnyView>,
        cx: &mut Context<Self>,
    ) -> Self {
        let shell = cx.entity().downgrade();
        // Escape は通常の key binding / Tool の action より先に取り消す。
        let drag_keys = cx.intercept_keystrokes(move |event, window, cx| {
            if event.keystroke.key == "escape" {
                let _ = shell.update(cx, |shell, cx| {
                    if shell.tab_drag.cancel() {
                        shell.drop_target = None;
                        shell.close_drag_preview(cx);
                        cx.stop_active_drag(window);
                        cx.stop_propagation();
                    }
                });
            }
        });
        let mut feature_registry = features::bundled_registry();
        for (view, entity) in views {
            feature_registry.register_shared_view(view, entity);
        }
        Self {
            shell: ShellState::default(),
            host: 0,
            group: None,
            detached: false,
            tab_drag: Default::default(),
            drag_preview: None,
            _drag_keys: drag_keys,
            panel_bounds: Default::default(),
            group_bounds: Default::default(),
            drop_target: None,
            layout_bounds: RefCell::new(None),
            session: ShellPresentation::default(),
            document_view,
            focus_handle: cx.focus_handle(),
            feature_registry,
            locale: Localizer::bundled(),
            keymap: KeymapRegistry::bundled(),
            languages: lapis_language::LanguageRegistry::bundled(),
            scroll_states: ShellScrollStates::default(),
            status: String::new(),
        }
    }
    pub(crate) fn update_presentation(
        &mut self,
        presentation: ShellPresentation,
        cx: &mut Context<Self>,
    ) {
        self.session = presentation;
        self.shell.synchronize_documents(&self.session.documents);
        self.synchronize_view_instances(cx);
        if self.session.root.is_some() {
            self.feature_registry
                .activate(ActivationEvent::WorkspaceOpened);
        }
        let language = self
            .session
            .active_path()
            .and_then(|path| self.languages.detect_path(path));
        self.feature_registry
            .activate(ActivationEvent::LanguageChanged(language));
        self.refresh_feature_activation(cx);
        cx.notify();
    }
    pub(crate) fn t(&self, key: &str) -> String {
        self.locale
            .resolve(&lapis_localization::MessageId::new(key))
    }
    fn refresh_feature_activation(&mut self, cx: &mut Context<Self>) {
        self.synchronize_view_instances(cx);
        self.shell
            .synchronize_activation(&mut self.feature_registry);
    }
    fn synchronize_view_instances(&mut self, cx: &mut Context<Self>) {
        for (tab, _) in self.shell.layout.placements() {
            if let PanelTab::Tool { view, instance } = tab {
                self.feature_registry
                    .ensure_view_instance(&view, &instance, cx);
            }
        }
    }
    pub(crate) fn preview_visible(&self) -> bool {
        [
            self.shell.panel(PanelPosition::Left),
            self.shell.panel(PanelPosition::Main),
            self.shell.panel(PanelPosition::Bottom),
            self.shell.panel(PanelPosition::Right),
        ]
        .into_iter()
        .any(|panel| {
            panel.open
                && matches!(
                    panel.active.as_ref(),
                    Some(PanelTab::Tool { view, .. }) if view.as_str() == id::VIEW_PREVIEW
                )
        })
    }

    pub(crate) fn view_state(&self) -> lapis_desktop_client::ConversationViewState {
        crate::features::conversation::capture_view_state(&self.shell)
    }
    pub(crate) fn restore_view(
        &mut self,
        view: lapis_desktop_client::ConversationViewState,
        cx: &mut Context<Self>,
    ) {
        crate::features::conversation::apply_view_state(
            view,
            &mut self.shell,
            &self.session.documents,
            &self.feature_registry,
        );
        self.synchronize_view_instances(cx);
        self.refresh_feature_activation(cx);
        cx.notify();
    }
    pub(crate) fn apply_scope_visibility(
        &mut self,
        hidden: std::collections::HashSet<crate::extension_ui::ViewInstanceId>,
        cx: &mut Context<Self>,
    ) {
        if self.shell.hidden_views != hidden {
            self.shell.hidden_views = hidden;
            self.refresh_feature_activation(cx);
            cx.notify();
        }
    }

    pub(crate) fn restore_workbench_view(
        &mut self,
        view: lapis_desktop_client::ConversationViewState,
        retained: &std::collections::HashSet<crate::extension_ui::ViewInstanceId>,
        cx: &mut Context<Self>,
    ) {
        let tabs = self
            .shell
            .layout
            .placements()
            .into_iter()
            .filter_map(|(tab, group)| {
                if !matches!(tab, PanelTab::Tool { .. }) || !retained.contains(&tab.instance_id()) {
                    return None;
                }
                self.shell
                    .layout
                    .group(&group)
                    .map(|panel| (tab, group.clone(), panel.position, panel.open))
            })
            .collect::<Vec<_>>();
        self.restore_view(view, cx);
        for (tab, group, position, open) in tabs {
            if self.shell.layout.group_for_tab(&tab).is_some() {
                continue;
            }
            let group = if self
                .shell
                .layout
                .group(&group)
                .is_some_and(|panel| panel.position == position)
            {
                group
            } else {
                self.shell.layout.primary_group_id(position).clone()
            };
            if let Some(panel) = self.shell.layout.group_mut(&group) {
                let active = panel.active.clone();
                panel.activate_without_open(tab);
                if active.is_some() {
                    panel.active = active;
                }
                if open {
                    panel.set_open_immediate(true);
                }
            }
        }
        self.refresh_feature_activation(cx);
    }

    pub(crate) fn set_status(&mut self, status: String, cx: &mut Context<Self>) {
        self.status = status;
        cx.notify();
    }
    pub(crate) fn set_locale(
        &mut self,
        locale: lapis_localization::LocaleId,
        cx: &mut Context<Self>,
    ) {
        let _ = self.locale.set_active(&locale);
        cx.notify();
    }
    pub(crate) fn request_document_close(
        &mut self,
        id: lapis_editor_core::DocumentId,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.emit(UiCommand::CloseDocument(id));
    }
    pub(crate) fn open_recent_workspace(
        &mut self,
        root: std::path::PathBuf,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.emit(UiCommand::OpenRecent(root));
    }
    pub(crate) fn open_new_window(&mut self, cx: &mut Context<Self>) {
        cx.emit(UiCommand::NewWindow);
    }
    pub(crate) fn toggle_inspector_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        #[cfg(debug_assertions)]
        {
            let _ = crate::devtools::toggle_inspector(window, cx);
        }
        let _ = (window, cx);
    }
    pub(crate) fn open_quick_search(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(UiCommand::QuickSearch);
    }
    pub(crate) fn dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        self.shell.command_palette_open = false;
        self.shell.tool_picker = None;
        self.shell.header_menu_open = false;
        self.shell.settings_menu_open = false;
        cx.notify();
    }
}
impl Shell {
    pub(crate) fn open_project(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(UiCommand::OpenWorkspace);
    }
}
impl Shell {
    pub(crate) fn open_file(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(UiCommand::OpenFile);
    }
}
impl Shell {
    pub(crate) fn save_file(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(UiCommand::Save);
    }
}
impl Shell {
    pub(crate) fn close_project(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(UiCommand::CloseWorkspace);
    }
}
impl Shell {
    pub(crate) fn create_new_folder(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(UiCommand::NewFolder);
    }
}
impl Shell {
    pub(crate) fn clone_from_git(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(UiCommand::CloneRepository);
    }
}
impl Shell {
    pub(crate) fn new_document(&mut self, _: &New, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(UiCommand::NewDocument);
    }
}
impl Shell {
    pub(crate) fn open(&mut self, _: &Open, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(UiCommand::OpenFile);
    }
}
impl Shell {
    pub(crate) fn save(&mut self, _: &Save, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(UiCommand::Save);
    }
    pub(crate) fn save_as(&mut self, _: &SaveAs, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(UiCommand::SaveAs);
    }
}
impl Shell {
    pub(crate) fn show_commands(
        &mut self,
        _: &ShowCommands,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.emit(UiCommand::QuickSearch);
    }
}
impl Shell {
    pub(crate) fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(UiCommand::Undo);
    }
}
impl Shell {
    pub(crate) fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(UiCommand::Redo);
    }
}
impl Shell {
    pub(crate) fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(UiCommand::Cut);
    }
}
impl Shell {
    pub(crate) fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(UiCommand::Copy);
    }
}
impl Shell {
    pub(crate) fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(UiCommand::Paste);
    }
}

fn command_item(index: usize, label: String, shortcut: String) -> gpui::Stateful<gpui::Div> {
    div()
        .id(("command-palette-item", index))
        .h(px(31.0))
        .px_2()
        .rounded(px(6.0))
        .flex()
        .items_center()
        .text_size(px(12.0))
        .text_color(theme::colors().text_primary)
        .hover(|style| style.bg(theme::colors().button_background_selected))
        .child(label)
        .child(div().flex_1())
        .child(
            div()
                .text_size(px(10.0))
                .text_color(theme::colors().text_tertiary)
                .child(shortcut),
        )
}
