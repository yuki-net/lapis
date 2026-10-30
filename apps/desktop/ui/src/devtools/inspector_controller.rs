//! ApplicationはWindowの対応だけを管理し、検査・表示状態はTargetごとのsessionが所有する。
use super::elements::{
    inspection::Inspection,
    tree_state::{TreeRow, TreeState},
};
use super::tool::DevToolTab;
use crate::components::ScrollState;
use gpui::{
    AnyWindowHandle, App, Bounds, EntityId, Global, Inspector, InspectorElementId, Subscription,
    Window, WindowBounds, WindowId, px, size,
};
use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
};

struct InspectorSession {
    target: AnyWindowHandle,
    viewer: AnyWindowHandle,
    inspection: Inspection,
    tree: TreeState,
    tree_scroll: ScrollState,
    tree_list: gpui::UniformListScrollHandle,
    content_scroll: ScrollState,
    /// 現在選択中のDevToolタブ
    active_tab: DevToolTab,
}

#[derive(Default)]
pub(super) struct InspectorController {
    // WindowIdはGPUIの世代付きkey。EntityIdも照合し、古いUI操作を再開後へ渡さない。
    sessions: HashMap<WindowId, InspectorSession>,
    _window_closed_subscription: Option<Subscription>,
}
impl Global for InspectorController {}

pub(super) fn init(cx: &mut App) {
    cx.set_global(InspectorController::default());
    let subscription = cx.on_window_closed(|cx| {
        if cx.global::<InspectorController>().sessions.is_empty() {
            return;
        }
        let open: HashSet<_> = cx
            .windows()
            .iter()
            .map(|handle| handle.window_id())
            .collect();
        let ended = {
            let controller = cx.global_mut::<InspectorController>();
            let ended: Vec<_> = controller
                .sessions
                .iter()
                .filter_map(|(id, session)| {
                    (!open.contains(id) || !open.contains(&session.viewer.window_id()))
                        .then_some(*id)
                })
                .collect();
            ended
                .into_iter()
                .filter_map(|id| controller.sessions.remove(&id))
                .collect::<Vec<_>>()
        };
        for session in ended {
            // 購読を先に解放する。残った遅延通知は世代付きWindow handleの検証で捨てる。
            let InspectorSession {
                target,
                viewer,
                inspection,
                ..
            } = session;
            drop(inspection);
            if open.contains(&target.window_id()) {
                let _ = target.update(cx, |_, window, _| Inspection::disable(window));
            }
            if open.contains(&viewer.window_id()) {
                let _ = viewer.update(cx, |_, window, _| window.remove_window());
            }
        }
    });
    cx.global_mut::<InspectorController>()
        ._window_closed_subscription = Some(subscription);
}

impl InspectorController {
    fn session(&self, inspector: EntityId) -> Option<&InspectorSession> {
        self.sessions
            .values()
            .find(|session| session.inspection.entity().entity_id() == inspector)
    }
    fn session_mut(&mut self, inspector: EntityId) -> Option<&mut InspectorSession> {
        self.sessions
            .values_mut()
            .find(|session| session.inspection.entity().entity_id() == inspector)
    }

    pub(super) fn defer(
        cx: &mut gpui::Context<Inspector>,
        action: impl FnOnce(EntityId, &mut App) + 'static,
    ) {
        let inspector = cx.entity_id();
        cx.defer(move |cx| {
            if cx.global::<Self>().session(inspector).is_some() {
                action(inspector, cx);
            }
        });
    }

    pub(super) fn inspector_handle(&self, target: WindowId) -> Option<AnyWindowHandle> {
        self.sessions.get(&target).map(|session| session.viewer)
    }
    pub(super) fn inspection_entity(&self, target: WindowId) -> Option<gpui::Entity<Inspector>> {
        self.sessions
            .get(&target)
            .map(|session| session.inspection.entity())
    }
    pub(super) fn tree_rows(
        inspector: &Inspector,
        entity: EntityId,
        cx: &mut App,
    ) -> Rc<[TreeRow]> {
        // open_windowの初回描画はsession登録前。登録後にviewerを明示更新する。
        cx.global_mut::<Self>()
            .session_mut(entity)
            .map(|session| {
                let rows = session.tree.rows(
                    inspector.element_tree(),
                    inspector.tree_revision(),
                    inspector.active_element_id(),
                    inspector.is_picking(),
                );
                if let Some(index) = session.tree.take_scroll_target() {
                    session
                        .tree_list
                        .scroll_to_item(index, gpui::ScrollStrategy::Center);
                }
                rows
            })
            .unwrap_or_default()
    }
    pub(super) fn toggle_tree_node(id: InspectorElementId, entity: EntityId, cx: &mut App) {
        if let Some(session) = cx.global_mut::<Self>().session_mut(entity) {
            session.tree.toggle(id);
        }
    }
    pub(super) fn tree_scroll(entity: EntityId, cx: &App) -> ScrollState {
        cx.global::<Self>()
            .session(entity)
            .map(|session| session.tree_scroll.clone())
            .unwrap_or_default()
    }
    pub(super) fn tree_list(entity: EntityId, cx: &App) -> gpui::UniformListScrollHandle {
        cx.global::<Self>()
            .session(entity)
            .map(|session| session.tree_list.clone())
            .unwrap_or_default()
    }
    pub(super) fn tree_width_index(entity: EntityId, cx: &App) -> usize {
        cx.global::<Self>()
            .session(entity)
            .map_or(0, |session| session.tree.width_index())
    }
    pub(super) fn content_scroll(entity: EntityId, cx: &App) -> ScrollState {
        cx.global::<Self>()
            .session(entity)
            .map(|session| session.content_scroll.clone())
            .unwrap_or_default()
    }

    pub(super) fn open(target: &mut Window, cx: &mut App) -> Result<AnyWindowHandle, String> {
        let handle = target.window_handle();
        if let Some(viewer) = cx.global::<Self>().inspector_handle(handle.window_id()) {
            // 同じTargetへのOpenは既存Windowへ戻る。重複した検査Entityを作らない。
            viewer
                .update(cx, |_, window, _| window.activate_window())
                .map_err(|error| error.to_string())?;
            return Ok(viewer);
        }
        if cx
            .global::<Self>()
            .sessions
            .values()
            .any(|session| session.viewer == handle)
        {
            return Err("Inspector自身を検査対象にはできません".into());
        }
        let inspector = Inspection::enable(target, cx);
        let observed = inspector.clone();
        let mut options = crate::app::window_options(cx);
        options.window_bounds = Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(900.), px(800.)),
            cx,
        )));
        options.window_min_size = Some(size(px(640.), px(360.)));
        if let Some(titlebar) = options.titlebar.as_mut() {
            titlebar.title = Some("Lapis DevTool".into());
        }
        let viewer = match cx.open_window(options, move |_, _| inspector) {
            Ok(viewer) => AnyWindowHandle::from(viewer),
            Err(error) => {
                Inspection::disable(target);
                return Err(format!("Inspectorウィンドウを開けませんでした: {error}"));
            }
        };
        let inspection = Inspection::attach(observed, viewer, cx);
        let tree_list = gpui::UniformListScrollHandle::new();
        let tree_scroll = ScrollState::with_handle(tree_list.0.borrow().base_handle.clone());
        cx.global_mut::<Self>().sessions.insert(
            handle.window_id(),
            InspectorSession {
                target: handle,
                viewer,
                inspection,
                tree: TreeState::default(),
                tree_scroll,
                tree_list,
                content_scroll: ScrollState::default(),
                active_tab: DevToolTab::default(),
            },
        );
        let _ = viewer.update(cx, |_, window, _| window.refresh());
        Ok(viewer)
    }

    pub(super) fn toggle(target: &mut Window, cx: &mut App) -> Result<bool, String> {
        if let Some(session) = cx
            .global_mut::<Self>()
            .sessions
            .remove(&target.window_handle().window_id())
        {
            let viewer = session.viewer;
            drop(session);
            Inspection::disable(target);
            let _ = viewer.update(cx, |_, window, _| window.remove_window());
            Ok(false)
        } else {
            Self::open(target, cx).map(|_| true)
        }
    }

    fn target(entity: EntityId, cx: &App) -> Option<AnyWindowHandle> {
        cx.global::<Self>()
            .session(entity)
            .map(|session| session.target)
    }
    pub(super) fn refresh_target(entity: EntityId, cx: &mut App) {
        if let Some(session) = cx.global::<Self>().session(entity) {
            let target = session.target;
            let inspector = session.inspection.entity();
            let _ = target.update(cx, |_, window, cx| {
                Inspection::refresh(&inspector, window, cx)
            });
        }
    }
    pub(super) fn repaint_target(entity: EntityId, cx: &mut App) {
        if let Some(target) = Self::target(entity, cx) {
            let _ = target.update(cx, |_, window, _| window.refresh());
        }
    }
    pub(super) fn select_element(id: InspectorElementId, entity: EntityId, cx: &mut App) {
        if let Some(target) = Self::target(entity, cx) {
            let _ = target.update(cx, |_, window, cx| Inspection::select(id, window, cx));
        }
    }
    pub(super) fn hover_element(id: Option<InspectorElementId>, entity: EntityId, cx: &mut App) {
        if let Some(target) = Self::target(entity, cx) {
            let _ = target.update(cx, |_, window, cx| Inspection::hover(id, window, cx));
        }
    }

    /// 現在選択中のDevToolタブを取得する
    pub(super) fn active_tab(entity: EntityId, cx: &App) -> DevToolTab {
        cx.global::<Self>()
            .session(entity)
            .map(|session| session.active_tab)
            .unwrap_or_default()
    }

    /// DevToolタブを切り替える
    pub(super) fn set_active_tab(tab: DevToolTab, entity: EntityId, cx: &mut App) {
        if let Some(session) = cx.global_mut::<Self>().session_mut(entity) {
            session.active_tab = tab;
        }
    }
}
