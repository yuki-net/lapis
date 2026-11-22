use super::ShellState;
use crate::ViewInstanceId;
use crate::timer::Timer;
pub use crate::tokens;
use crate::{
    DraggedPanelTab, PanelDrop, PanelEdge, PanelHost, PanelId, PanelSpanTransition, PanelTab,
    PickerTarget, ResizeMode, ResizeTarget,
};
use crate::{
    actions::*,
    commands::{InvokeCommand, UiCommand},
    components::{
        Icon, IconName, ScrollAxis, ScrollState, SurfaceVariant, panel_content, panel_empty_state,
        panel_empty_state_element, scroll_viewport,
    },
    extension_ui::{ActivationEvent, FeatureRegistry},
    ids as id,
    localization::Localizer,
    theme,
};
use extension_ui_contract::{PanelScrollPolicy, UiSlot, ViewId, ZonePosition};
use gpui::{
    AnyView, App, ClickEvent, Context, CursorStyle, ElementId, FocusHandle, Focusable,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point, Render, SharedString,
    Window, div, point, prelude::*, px, relative,
};
use std::{cell::RefCell, collections::HashMap, time::Duration};
use workspace::FileEntryKind;

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
mod tab_context;
mod tab_drag;
mod tool_picker;
mod view_instances;

pub struct Shell {
    pub branch_picker: Option<gpui::AnyView>,
    pub shell: ShellState,
    pub host: super::window_group::HostId,
    pub group: Option<gpui::EntityId>,
    pub detached: bool,
    tab_drag: super::tab_state::TabDragState,
    drag_preview: Option<gpui::AnyWindowHandle>,
    tab_context: Option<(PanelId, PanelTab, Point<Pixels>)>,
    tab_mutations_blocked: bool,
    tab_bounds: std::rc::Rc<RefCell<HashMap<PanelTab, gpui::Bounds<Pixels>>>>,
    _drag_keys: gpui::Subscription,
    _notifications: gpui::Subscription,
    pub zone_panel_bounds: RefCell<HashMap<ZonePosition, gpui::Bounds<Pixels>>>,
    pub panel_bounds: RefCell<HashMap<PanelId, gpui::Bounds<Pixels>>>,
    drop_target: Option<DropPreview>,
    pub zone_bounds: RefCell<HashMap<ZonePosition, gpui::Bounds<Pixels>>>,
    dock_geometry: RefCell<Option<crate::dock_geometry::DockGeometry>>,
    pub session: ShellPresentation,
    document_view: AnyView,
    focus_handle: FocusHandle,
    feature_registry: FeatureRegistry,
    extension_view_titles: HashMap<
        ViewId,
        (
            desktop_client::ExtensionContributionSnapshot,
            String,
            localization::MessageId,
        ),
    >,
    notification_center: gpui::Entity<crate::notification::Center>,
    locale: Localizer,
    languages: language::LanguageRegistry,
    scroll_states: ShellScrollStates,
    status: String,
    /// 通常の status とは独立した警告。共有状態が古い可能性の表示に使う。
    warning: String,
}

#[derive(Clone, Debug, PartialEq)]
struct DropPreview {
    target: PanelDrop,
    invalid: bool,
    zone: ZonePosition,
    rect: Option<(f32, f32, f32, f32)>,
}

#[derive(Default)]
pub struct ShellPresentation {
    pub documents: Vec<desktop_client::DocumentTab>,
    pub root: Option<std::path::PathBuf>,
    pub workspace_name: String,
    pub workspace_id: Option<String>,
    pub connection: Option<crate::ConnectionIdentity>,
    pub active: Option<editor_core::DocumentId>,
    pub cursor: (usize, usize),
    pub line_ending: &'static str,
    pub encoding: desktop_client::Encoding,
    pub recent_workspaces: Vec<std::path::PathBuf>,
}
impl ShellPresentation {
    fn tabs(&self) -> Vec<desktop_client::DocumentTab> {
        self.documents.clone()
    }
    fn workspace_root(&self) -> Option<&std::path::Path> {
        self.root.as_deref()
    }
    fn workspace_name(&self) -> &str {
        &self.workspace_name
    }
    fn active_document_id(&self) -> Option<&editor_core::DocumentId> {
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
    fn encoding(&self) -> desktop_client::Encoding {
        self.encoding
    }
}

struct ShellScrollStates {
    panel_contents: RefCell<HashMap<PanelTab, ScrollState>>,
    panel_empty: RefCell<HashMap<super::PanelId, ScrollState>>,
    panel_tabs: RefCell<HashMap<super::PanelId, ScrollState>>,
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
    fn panel_empty(&self, panel_id: &super::PanelId) -> ScrollState {
        self.panel_empty
            .borrow_mut()
            .entry(panel_id.clone())
            .or_default()
            .clone()
    }
    fn panel_tabs(&self, panel_id: &super::PanelId) -> ScrollState {
        self.panel_tabs
            .borrow_mut()
            .entry(panel_id.clone())
            .or_default()
            .clone()
    }
}
impl gpui::EventEmitter<UiCommand> for Shell {}

#[cfg(all(test, debug_assertions))]
mod panel_scroll_tests {
    use super::*;
    #[test]
    fn panels_have_independent_scroll_state() {
        let states = ShellScrollStates::default();
        let a = super::super::PanelId::new(2);
        let b = super::super::PanelId::new(5);
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
    pub fn new(
        document_view: AnyView,
        views: HashMap<ViewId, AnyView>,
        mut feature_registry: FeatureRegistry,
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
        let shell = cx.entity().downgrade();
        let execute = std::rc::Rc::new(move |command, window: &mut Window, cx: &mut App| {
            let _ = shell.update(cx, |shell, cx| shell.execute_command(command, window, cx));
        });
        let center = cx.new(|cx| crate::notification::Center::new(execute, cx));
        feature_registry.register_shared_view(id::VIEW_NOTIFICATIONS.into(), center.clone().into());
        let notifications = cx.observe_global::<crate::notification::Store>(|_, cx| cx.notify());
        for (view, entity) in views {
            feature_registry.register_shared_view(view, entity);
        }
        Self {
            shell: ShellState::default(),
            branch_picker: None,
            host: 0,
            group: None,
            detached: false,
            tab_drag: Default::default(),
            drag_preview: None,
            tab_context: None,
            tab_mutations_blocked: false,
            tab_bounds: Default::default(),
            _drag_keys: drag_keys,
            _notifications: notifications,
            zone_panel_bounds: Default::default(),
            panel_bounds: Default::default(),
            zone_bounds: Default::default(),
            drop_target: None,
            dock_geometry: RefCell::new(None),
            session: ShellPresentation::default(),
            document_view,
            focus_handle: cx.focus_handle(),
            feature_registry,
            extension_view_titles: HashMap::new(),
            notification_center: center,
            locale: Localizer::bundled(),
            languages: language::LanguageRegistry::bundled(),
            scroll_states: ShellScrollStates::default(),
            status: String::new(),
            warning: String::new(),
        }
    }
    pub fn update_presentation(&mut self, presentation: ShellPresentation, cx: &mut Context<Self>) {
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
    pub fn t(&self, key: &str) -> String {
        self.locale.resolve(&localization::MessageId::new(key))
    }

    /// 個体を所有する機能へ、View 単位の操作を行う。
    ///
    /// Shell は Terminal などの型を知らないため、永続化やコンテキスト切替は
    /// この入口から View の所有者へ委ねる。
    pub fn with_lifecycle<R>(
        &self,
        view: &ViewId,
        f: impl FnOnce(&dyn crate::extension_ui::ViewLifecycle) -> R,
    ) -> Option<R> {
        let lifecycle = self.feature_registry.lifecycle(view)?;
        Some(f(&*lifecycle.borrow()))
    }

    /// タブの Resource を個体へ再適用する。復元したタブと実体を一致させる。
    pub fn bind_view_resources(&mut self, cx: &mut Context<Self>) {
        self.synchronize_view_instances(cx);
        self.refresh_feature_activation(cx);
    }

    /// 個体を所有する機能が持つ Resource の永続化用サマリー。
    pub fn persisted_view_resources(
        &self,
        view: &ViewId,
        cx: &gpui::App,
    ) -> Vec<desktop_client::RestoredTerminal> {
        self.with_lifecycle(view, |lifecycle| lifecycle.persisted_resources(cx))
            .unwrap_or_default()
    }

    /// 個体を所有する機能が持つ Resource の所有者。
    pub fn view_resource_owner(
        &self,
        view: &ViewId,
        cx: &gpui::App,
    ) -> Option<editor_core::ConversationId> {
        self.with_lifecycle(view, |lifecycle| lifecycle.resource_owner(cx))
            .flatten()
    }

    pub fn tool_close_requires_confirmation(&self, tab: &PanelTab, cx: &gpui::App) -> bool {
        // 復元や再接続が終わる前は Resource の稼働状態を判定できない。
        if tab.resource().is_some()
            && (self
                .feature_registry
                .instance_view(&tab.instance_id())
                .is_none()
                || tab
                    .view_id()
                    .is_none_or(|view| self.feature_registry.lifecycle(view).is_none()))
        {
            return true;
        }
        tab.view_id()
            .and_then(|view| {
                self.with_lifecycle(view, |lifecycle| {
                    lifecycle.requires_close_confirmation(&tab.instance_id(), cx)
                })
            })
            .unwrap_or(false)
    }

    pub fn running_work_requires_confirmation(&self, cx: &gpui::App) -> bool {
        self.shell
            .layout
            .placements()
            .iter()
            .any(|(tab, _)| self.tool_close_requires_confirmation(tab, cx))
    }
    fn refresh_feature_activation(&mut self, cx: &mut Context<Self>) {
        self.synchronize_view_instances(cx);
        self.shell
            .synchronize_activation(&mut self.feature_registry);
    }
    fn synchronize_view_instances(&mut self, cx: &mut Context<Self>) {
        for (tab, _) in self.shell.layout.placements() {
            if let PanelTab::Tool { view, instance, .. } = tab {
                // 個体を所有する機能は宣言から解決する。ここでは生成しない。
                if self
                    .feature_registry
                    .declaration(&view)
                    .is_some_and(|declaration| {
                        declaration.instance_policy
                            == extension_ui_contract::ToolInstancePolicy::Multiple
                    })
                {
                    continue;
                }
                self.feature_registry
                    .ensure_view_instance(&view, &instance, cx);
            }
        }
        self.synchronize_owned_instances(cx);
    }
    /// 個体を所有する機能から、表示中タブの代表個体を引く。
    ///
    /// Terminal のようにタブごとに個体が異なる View は、代表として表示中の
    /// タブの所属を使う。Resource 固定の参照先は各タブ自身が保持する。
    pub fn first_active_tab_of(&self, view: &str) -> Option<PanelTab> {
        self.shell
            .layout
            .placements()
            .into_iter()
            .find(|(tab, _)| {
                tab.view_id()
                    .is_some_and(|candidate| candidate.as_str() == view)
            })
            .map(|(tab, _)| tab)
    }

    pub fn preview_visible(&self) -> bool {
        self.shell.layout.visible_panels().any(|panel| {
            matches!(
                panel.active.as_ref(),
                Some(PanelTab::Tool { view, .. }) if view.as_str() == id::VIEW_PREVIEW
            )
        })
    }

    pub fn view_state(&self) -> desktop_client::ConversationViewState {
        crate::view_state::capture_view_state(&self.shell)
    }

    pub fn restore_view(
        &mut self,
        view: desktop_client::ConversationViewState,
        cx: &mut Context<Self>,
    ) {
        crate::view_state::apply_view_state(
            view,
            &mut self.shell,
            &self.session.documents,
            &self.feature_registry,
        );
        self.synchronize_view_instances(cx);
        self.refresh_feature_activation(cx);
        cx.notify();
    }
    pub fn apply_scope_visibility(
        &mut self,
        hidden: std::collections::HashSet<extension_ui_contract::ViewInstanceId>,
        cx: &mut Context<Self>,
    ) {
        if self.shell.hidden_views != hidden {
            self.shell.hidden_views = hidden;
            self.refresh_feature_activation(cx);
            cx.notify();
        }
    }

    pub fn restore_workbench_view(
        &mut self,
        view: desktop_client::ConversationViewState,
        retained: &std::collections::HashSet<extension_ui_contract::ViewInstanceId>,
        cx: &mut Context<Self>,
    ) {
        let tabs = self
            .shell
            .layout
            .placements()
            .into_iter()
            .filter_map(|(tab, panel_id)| {
                if !matches!(tab, PanelTab::Tool { .. }) || !retained.contains(&tab.instance_id()) {
                    return None;
                }
                let position = self.shell.layout.zone_for_panel(&panel_id)?;
                let open = self.shell.layout.panel(&panel_id)?.open;
                Some((tab, panel_id, position, open))
            })
            .collect::<Vec<_>>();
        self.restore_view(view, cx);
        for (tab, panel_id, position, open) in tabs {
            if self.shell.layout.panel_for_tab(&tab).is_some() {
                continue;
            }
            let panel_id = if self.shell.layout.zone_for_panel(&panel_id) == Some(position) {
                panel_id
            } else {
                match self.shell.layout.first_panel_in_zone(position) {
                    Some(panel_id) => panel_id,
                    None => {
                        let _ = self.shell.layout.open_tab(position, tab);
                        continue;
                    }
                }
            };
            if self.shell.layout.insert_tab_in_panel(&panel_id, tab, true) && open {
                self.shell.layout.set_panel_open(&panel_id, true);
            }
        }
        self.refresh_feature_activation(cx);
    }

    pub fn set_status(&mut self, status: String, cx: &mut Context<Self>) {
        self.status = status;
        cx.notify();
    }
    /// 通常の status と独立した警告を設定する。空文字列で解除する。
    pub fn set_warning(&mut self, warning: String, cx: &mut Context<Self>) {
        if self.warning == warning {
            return;
        }
        self.warning = warning;
        cx.notify();
    }
    pub fn set_locale(&mut self, locale: localization::LocaleId, cx: &mut Context<Self>) {
        let _ = self.locale.set_active(&locale);
        self.notification_center
            .update(cx, |center, cx| center.set_locale(&locale, cx));
        cx.notify();
    }
    /// 新規 View の生成だけを制御する。既存のタブは一時切断で終了しない。
    pub fn set_view_available(&mut self, view: ViewId, available: bool, cx: &mut Context<Self>) {
        self.feature_registry.set_view_available(view, available);
        self.synchronize_view_instances(cx);
        cx.notify();
    }

    /// View の登録名だけを差し替え、タブ個体と接続中の Resource を維持する。
    pub fn can_rekey_extension_view(&self, old: &ViewId, new: &ViewId) -> bool {
        self.feature_registry.can_rekey_view(old, new)
    }
    pub fn rekey_extension_view(
        &mut self,
        old: &ViewId,
        new: &ViewId,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.feature_registry.rekey_view(old, new) {
            return false;
        }
        let tabs = self
            .shell
            .layout
            .placements()
            .into_iter()
            .map(|(tab, _)| tab)
            .filter(|tab| tab.view_id() == Some(old))
            .collect::<Vec<_>>();
        for tab in tabs {
            self.shell
                .layout
                .rebind_tab(&tab, tab.with_view(new.clone()));
        }
        if let Some(value) = self.extension_view_titles.remove(old) {
            self.extension_view_titles.insert(new.clone(), value);
        }
        if self.shell.hidden_view_types.remove(old) {
            self.shell.hidden_view_types.insert(new.clone());
        }
        self.shell.unregistered_view_types.remove(old);
        self.shell.unregistered_view_types.remove(new);
        self.synchronize_view_instances(cx);
        self.refresh_feature_activation(cx);
        cx.notify();
        true
    }
    /// 拡張の表示だけを隠す。Layout の保存値と他の非表示条件は変えない。
    pub fn set_view_hidden(&mut self, view: ViewId, hidden: bool, cx: &mut Context<Self>) {
        let changed = if hidden {
            self.shell.hidden_view_types.insert(view)
        } else {
            self.shell.hidden_view_types.remove(&view)
        };
        if changed {
            self.synchronize_view_instances(cx);
            self.refresh_feature_activation(cx);
            cx.notify();
        }
    }
    /// View の表示名は提供元の翻訳を使い、選択言語の変更時にも解決し直す。
    pub fn set_extension_view_title(
        &mut self,
        view: ViewId,
        snapshot: desktop_client::ExtensionContributionSnapshot,
        owner: String,
        title: localization::MessageId,
        cx: &mut Context<Self>,
    ) {
        self.extension_view_titles
            .insert(view, (snapshot, owner, title));
        cx.notify();
    }

    pub fn set_extension_view_command(
        &mut self,
        view: ViewId,
        command: Option<extension_ui_contract::CommandId>,
        default_zone: ZonePosition,
        multiple_instances: bool,
        cx: &mut Context<Self>,
    ) {
        self.feature_registry
            .set_view_default_zone(&view, default_zone);
        self.feature_registry.set_view_instance_policy(
            &view,
            if multiple_instances {
                extension_ui_contract::ToolInstancePolicy::Multiple
            } else {
                extension_ui_contract::ToolInstancePolicy::Shared
            },
        );
        if !self.feature_registry.set_view_open_command(view, command) {
            tracing::warn!("拡張の View コマンドが重複しています");
        }
        cx.notify();
    }

    fn contribution_title(&self, contribution: &extension_ui_contract::UiContribution) -> String {
        contribution
            .view
            .as_ref()
            .and_then(|view| self.extension_view_titles.get(view))
            .map(|(snapshot, owner, title)| {
                snapshot.resolve_text(owner, self.locale.active(), title)
            })
            .unwrap_or_else(|| self.locale.resolve(&contribution.title))
    }

    pub fn command_search_provider(
        &self,
        shortcut: impl Fn(&extension_ui_contract::CommandId) -> String,
    ) -> crate::command_search::CommandSearchProvider {
        crate::command_search::CommandSearchProvider::from_registry(
            &self.feature_registry,
            &self.locale,
            shortcut,
            |contribution| self.contribution_title(contribution),
        )
    }
    pub fn set_locale_packs(
        &mut self,
        packs: &[localization::LocalePack],
        locale: &localization::LocaleId,
        cx: &mut Context<Self>,
    ) {
        let _ = self.locale.replace_external(packs);
        let _ = self.locale.set_active(locale);
        self.notification_center
            .update(cx, |center, cx| center.set_locale_packs(packs, locale, cx));
        cx.notify();
    }
    pub fn request_document_close(
        &mut self,
        id: editor_core::DocumentId,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.emit(UiCommand::CloseDocument(id));
    }
    pub fn open_recent_workspace(
        &mut self,
        root: std::path::PathBuf,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.emit(UiCommand::OpenRecent(root));
    }
    pub fn open_new_window(&mut self, cx: &mut Context<Self>) {
        cx.emit(UiCommand::NewWindow);
    }
    pub fn toggle_devtool_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        #[cfg(all(debug_assertions, not(target_family = "wasm")))]
        {
            let _ = crate::toggle_devtools(window, cx);
        }
        #[cfg(all(debug_assertions, target_family = "wasm"))]
        window.toggle_inspector(cx);
        let _ = (window, cx);
    }
    pub fn open_quick_search(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(UiCommand::QuickSearch);
    }
    pub fn dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        self.shell.command_palette_open = false;
        self.tab_context = None;
        self.shell.tool_picker = None;
        self.shell.header_menu_open = false;
        self.shell.workspace_menu_open = false;
        self.shell.settings_menu_open = false;
        cx.notify();
    }
}
impl Shell {
    pub fn open_project(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(UiCommand::OpenWorkspace);
    }
}
impl Shell {
    pub fn open_file(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(UiCommand::OpenFile);
    }
}
impl Shell {
    pub fn save_file(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(UiCommand::Save);
    }
}
impl Shell {
    pub fn close_project(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(UiCommand::CloseWorkspace);
    }
}
impl Shell {
    pub fn create_new_folder(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(UiCommand::NewFolder);
    }
}
impl Shell {
    pub fn clone_from_git(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(UiCommand::CloneRepository);
    }
}
impl Shell {
    pub fn new_document(&mut self, _: &New, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(UiCommand::NewDocument);
    }
}
impl Shell {
    pub fn open(&mut self, _: &Open, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(UiCommand::OpenFile);
    }
}
impl Shell {
    pub fn save(&mut self, _: &Save, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(UiCommand::Save);
    }
    pub fn save_as(&mut self, _: &SaveAs, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(UiCommand::SaveAs);
    }
}
impl Shell {
    pub fn show_commands(&mut self, _: &ShowCommands, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(UiCommand::QuickSearch);
    }
}
impl Shell {
    pub fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(UiCommand::Cut);
    }
}
impl Shell {
    pub fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(UiCommand::Copy);
    }
}
impl Shell {
    pub fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(UiCommand::Paste);
    }
}

fn command_item(
    index: usize,
    label: String,
    shortcut: String,
    cx: &gpui::App,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(("command-palette-item", index))
        .h(px(31.0))
        .px_2()
        .rounded(px(6.0))
        .flex()
        .items_center()
        .text_size(px(12.0))
        .text_color(theme::colors(cx).text_primary)
        .hover(|style| style.bg(theme::colors(cx).button_background_selected))
        .child(label)
        .child(div().flex_1())
        .child(
            div()
                .text_size(px(10.0))
                .text_color(theme::colors(cx).text_tertiary)
                .child(shortcut),
        )
}
