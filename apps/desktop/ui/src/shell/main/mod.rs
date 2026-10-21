use super::ShellState;
pub(crate) use crate::tokens;
use crate::{
    app::*,
    commands::UiCommand,
    components::{
        Icon, IconName, ScrollAxis, ScrollState, SurfaceVariant, panel_empty_state,
        panel_empty_state_element, scroll_viewport, surface,
    },
    extension_ui::{ActivationEvent, FeatureRegistry, PanelScrollPolicy, ThemeId, UiSlot, ViewId},
    features::{self, id},
    keymap::KeymapRegistry,
    localization::Localizer,
    theme,
};
use crate::{
    extension_ui::PanelPosition,
    shell::{DraggedPanelTab, PanelHost, PanelSpanTransition, PanelTab, ResizeMode, ResizeTarget},
};
use gpui::{
    AnyView, App, ClickEvent, Context, CursorStyle, ElementId, FocusHandle, Focusable,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point, Render, SharedString,
    Timer, Window, WindowControlArea, div, point, prelude::*, px, relative,
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
mod tool_picker;

const fn panel_key(position: PanelPosition) -> u32 {
    match position {
        PanelPosition::Left => 1,
        PanelPosition::Main => 2,
        PanelPosition::Bottom => 3,
        PanelPosition::Right => 4,
    }
}

pub(crate) struct Shell {
    pub(crate) shell: ShellState,
    pub(crate) session: ShellPresentation,
    document_view: AnyView,
    views: HashMap<ViewId, AnyView>,
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
    panel_empty: [ScrollState; 4],
    panel_tabs: [ScrollState; 4],
    tool_picker: ScrollState,
}
impl Default for ShellScrollStates {
    fn default() -> Self {
        Self {
            panel_contents: RefCell::new(HashMap::new()),
            panel_empty: std::array::from_fn(|_| ScrollState::new()),
            panel_tabs: std::array::from_fn(|_| ScrollState::new()),
            tool_picker: ScrollState::new(),
        }
    }
}
fn panel_index(position: PanelPosition) -> usize {
    match position {
        PanelPosition::Left => 0,
        PanelPosition::Main => 1,
        PanelPosition::Bottom => 2,
        PanelPosition::Right => 3,
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
    fn panel_empty(&self, position: PanelPosition) -> &ScrollState {
        &self.panel_empty[panel_index(position)]
    }
    fn panel_tabs(&self, position: PanelPosition) -> &ScrollState {
        &self.panel_tabs[panel_index(position)]
    }
}
impl gpui::EventEmitter<UiCommand> for Shell {}
impl Focusable for Shell {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
impl Shell {
    pub(crate) fn new(
        document_view: AnyView,
        views: HashMap<ViewId, AnyView>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            shell: ShellState::default(),
            session: ShellPresentation::default(),
            document_view,
            views,
            focus_handle: cx.focus_handle(),
            feature_registry: features::bundled_registry(),
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
        self.refresh_feature_activation();
        cx.notify();
    }
    pub(crate) fn t(&self, key: &str) -> String {
        self.locale
            .resolve(&lapis_localization::MessageId::new(key))
    }
    fn refresh_feature_activation(&mut self) {
        self.shell
            .synchronize_activation(&mut self.feature_registry);
    }
    pub(crate) fn preview_visible(&self) -> bool {
        [
            &self.shell.left_panel,
            &self.shell.main_panel,
            &self.shell.bottom_panel,
            &self.shell.right_panel,
        ]
        .into_iter()
        .any(|panel| {
            panel.open
                && matches!(
                    panel.active.as_ref(),
                    Some(PanelTab::Tool(view)) if view.as_str() == id::VIEW_PREVIEW
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
        let navigation = PanelTab::Tool(id::VIEW_PROJECT_TASKS.into());
        if !self.shell.left_panel.tabs.contains(&navigation) {
            self.shell.left_panel.tabs.insert(0, navigation);
        }
        self.refresh_feature_activation();
        cx.notify();
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
