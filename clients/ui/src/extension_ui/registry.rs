use gpui::{AnyView, App};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

use extension_ui_contract::{
    ActivationCondition, CommandId, FeatureDescriptor, FeatureId, FeatureKind, ToolInstancePolicy,
    UiContribution, UiSlot, ViewId, ViewInstanceId, ViewVisibility, WorkspaceCapabilityId,
    ZonePosition,
};

use super::ActivationEvent;

/// タブの表示状態と Resource を扱う機能からの要求。
///
/// 機能は Layout を所有しない。タブの Resource 差し替えや新しいタブの追加は
/// 要求として返し、Shell が Layout に適用する。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ViewRequest {
    /// タブの参照先 Resource を差し替える。個体 ID は変えない。
    BindResource {
        instance: ViewInstanceId,
        resource: Option<String>,
    },
    /// 同じ View のタブを1つ増やす。
    OpenTab(ViewId),
}

/// 宣言された View の個体を所有する機能が実装する内部契約。
///
/// [`FeatureRegistry`] は個体 ID と表示状態を解決し、Entity の生成・表示状態の
/// 反映・Resource の接続と終了はこの契約の実装（feature 自身）が行う。
/// Shell は View の種類で分岐せず、この契約だけを呼ぶ。
pub(crate) trait ViewLifecycle {
    /// 個体の実体を作る。作れない場合は `None`。
    fn create(&mut self, _instance: &ViewInstanceId, _cx: &mut App) -> Option<AnyView> {
        None
    }

    /// 外部の変更通知を購読し始める。`notify` は Shell へ再同期を促し、
    /// Shell が既に解放されていれば `false` を返す。
    fn start(&mut self, _notify: ViewNotifier, _cx: &mut App) {}

    /// タブの表示状態と参照先 Resource を個体へ伝える。
    fn activate(
        &mut self,
        _instance: &ViewInstanceId,
        _resource: Option<&str>,
        _visible: bool,
        _cx: &mut App,
    ) {
    }

    /// タブが Layout から消えた。所有 Resource は終了せず、個体への配線だけを外す。
    fn release(&mut self, _instance: &ViewInstanceId) {}

    /// タブを閉じた。この個体が所有する Resource を終了する。
    fn deactivate(&mut self, _instance: &ViewInstanceId, _cx: &mut App) {}

    /// 外部の変更通知を受けた。表示中の個体へ再取得を促す。
    fn changed(&mut self, _cx: &mut App) {}

    /// タブの表示状態と Resource を扱う機能からの要求を取り出す。
    fn take_requests(&mut self) -> Vec<ViewRequest> {
        Vec::new()
    }

    /// 機能が所有する Resource の永続化用サマリー。
    fn persisted_resources(&self, _cx: &App) -> Vec<desktop_client::RestoredTerminal> {
        Vec::new()
    }

    /// 機能が所有する Resource の所有者（Task）。
    fn resource_owner(&self, _cx: &App) -> Option<editor_core::ConversationId> {
        None
    }
}

pub(crate) type SharedLifecycle = Rc<RefCell<dyn ViewLifecycle>>;

/// 機能から Shell への再同期通知。Shell が既に解放されていれば `false`。
pub(crate) type ViewNotifier = Rc<dyn Fn(&mut App) -> bool>;

/// 単一実体を共有する View の個体 ID。個体の命名は Client 側の配置規則とする。
pub(crate) fn shared_tool_instance(view: &ViewId) -> ViewInstanceId {
    ViewInstanceId::new(format!("shared:{}", view.as_str()))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FeatureState {
    pub installed: bool,
    pub loaded: bool,
    pub running: bool,
}

/// 宣言（`FeatureDescriptor`）を受けて、表示状態と GPUI の実体を解決する内部レジストリ。
#[derive(Default)]
pub struct FeatureRegistry {
    descriptors: BTreeMap<FeatureId, FeatureDescriptor>,
    states: HashMap<FeatureId, FeatureState>,
    active_views: HashMap<UiSlot, ViewId>,
    visible_views: HashSet<ViewId>,
    shared_views: HashMap<ViewId, AnyView>,
    lifecycles: HashMap<ViewId, SharedLifecycle>,
    started: HashSet<ViewId>,
    instances: HashMap<ViewInstanceId, (ViewId, AnyView)>,
    workspace_open: bool,
    capabilities: HashSet<WorkspaceCapabilityId>,
    active_commands: HashSet<CommandId>,
    active_language: Option<language::LanguageId>,
}

/// 宣言の表示条件を、Client が持つ現在の状態で評価する。
fn is_visible(
    visibility: &ViewVisibility,
    workspace_open: bool,
    capabilities: &HashSet<WorkspaceCapabilityId>,
) -> bool {
    match visibility {
        ViewVisibility::Always => true,
        ViewVisibility::WorkspaceOpen => workspace_open,
        ViewVisibility::WorkspaceCapability(capability) => capabilities.contains(capability),
    }
}

impl FeatureRegistry {
    pub(crate) fn register_shared_view(&mut self, view: ViewId, entity: AnyView) {
        self.shared_views.insert(view, entity);
    }

    /// 個体を所有する機能の実体を登録する。Multiple はこの実体が作る Entity だけで描画する。
    pub(crate) fn register_view_lifecycle(
        &mut self,
        view: ViewId,
        lifecycle: Rc<RefCell<dyn ViewLifecycle>>,
    ) {
        self.lifecycles.insert(view, lifecycle);
    }

    pub(crate) fn can_create_view(&self, view: &ViewId, policy: ToolInstancePolicy) -> bool {
        match policy {
            ToolInstancePolicy::Shared => self.shared_views.contains_key(view),
            ToolInstancePolicy::Multiple => self.lifecycles.contains_key(view),
        }
    }

    /// View の宣言。タブの追加方針や Resource 参照の宣言を Shell が読む。
    pub(crate) fn declaration(&self, view: &ViewId) -> Option<&UiContribution> {
        self.descriptors
            .values()
            .flat_map(|feature| &feature.contributions)
            .find(|contribution| contribution.view.as_ref() == Some(view))
    }

    /// 個体を所有する機能が登録された View。
    pub(crate) fn lifecycle_views(&self) -> Vec<ViewId> {
        self.lifecycles.keys().cloned().collect()
    }

    /// 個体を所有する機能を引く。
    pub(crate) fn lifecycle(&self, view: &ViewId) -> Option<&SharedLifecycle> {
        self.lifecycles.get(view)
    }

    /// 外部 Resource を参照するタブを宣言した View。タブの復元に使う。
    pub(crate) fn external_resource_view(&self) -> Option<ViewId> {
        self.descriptors
            .values()
            .flat_map(|feature| &feature.contributions)
            .find(|contribution| {
                contribution.tab_reference == extension_ui_contract::TabReference::ExternalResource
            })
            .and_then(|contribution| contribution.view.clone())
    }

    /// 機能が現在持っている個体。Layout から消えた個体の解放に使う。
    pub(crate) fn view_instances(&self, view: &ViewId) -> Vec<ViewInstanceId> {
        self.instances
            .iter()
            .filter(|(_, (owner, _))| owner == view)
            .map(|(instance, _)| instance.clone())
            .collect()
    }

    /// 機能の購読を1回だけ始める。backend の通知は receiver を1度しか返さない。
    pub(crate) fn ensure_started(&mut self, view: &ViewId, notify: ViewNotifier, cx: &mut App) {
        if !self.started.insert(view.clone()) {
            return;
        }
        if let Some(lifecycle) = self.lifecycles.get_mut(view) {
            lifecycle.borrow_mut().start(notify, cx);
        }
    }

    pub(crate) fn ensure_view_instance(
        &mut self,
        view: &ViewId,
        instance: &ViewInstanceId,
        cx: &mut App,
    ) -> bool {
        if let Some((definition, _)) = self.instances.get(instance) {
            return definition == view;
        }
        let policy = self
            .descriptors
            .values()
            .flat_map(|feature| &feature.contributions)
            .find(|contribution| contribution.view.as_ref() == Some(view))
            .map(|contribution| contribution.instance_policy);
        let entity = match policy {
            Some(ToolInstancePolicy::Multiple) => self
                .lifecycles
                .get(view)
                .and_then(|lifecycle| lifecycle.borrow_mut().create(instance, cx)),
            Some(ToolInstancePolicy::Shared) => self.shared_views.get(view).cloned(),
            None => None,
        };
        let Some(entity) = entity else {
            return false;
        };
        self.instances
            .insert(instance.clone(), (view.clone(), entity));
        true
    }

    /// タブの表示状態と参照先 Resource を個体へ伝える。
    pub(crate) fn activate_instance(
        &mut self,
        view: &ViewId,
        instance: &ViewInstanceId,
        resource: Option<&str>,
        visible: bool,
        cx: &mut App,
    ) {
        if let Some(lifecycle) = self.lifecycles.get_mut(view) {
            lifecycle
                .borrow_mut()
                .activate(instance, resource, visible, cx);
        }
    }

    /// タブが Layout から消えた。個体を所有する機能から配線を外す。
    ///
    /// Resource の終了は行わない。Task 切替や Layout の置き換えでタブが消えただけの
    /// 場合に Session を止めないため、終了は明示的な close 経路だけが行う。
    pub(crate) fn release_instance(
        &mut self,
        view: &ViewId,
        instance: &ViewInstanceId,
        cx: &mut App,
    ) {
        if let Some(lifecycle) = self.lifecycles.get_mut(view) {
            lifecycle.borrow_mut().release(instance);
        }
        self.instances.remove(instance);
        let _ = cx;
    }

    /// タブを閉じた。個体を所有する機能へ終了を伝える。
    pub(crate) fn deactivate_instance(
        &mut self,
        view: &ViewId,
        instance: &ViewInstanceId,
        cx: &mut App,
    ) {
        if let Some(lifecycle) = self.lifecycles.get_mut(view) {
            lifecycle.borrow_mut().deactivate(instance, cx);
        }
        self.instances.remove(instance);
    }

    /// 個体を所有する機能から Layout への要求を取り出す。
    pub(crate) fn take_view_requests(&mut self) -> Vec<(ViewId, ViewRequest)> {
        let mut requests = Vec::new();
        for (view, lifecycle) in &self.lifecycles {
            for request in lifecycle.borrow_mut().take_requests() {
                requests.push((view.clone(), request));
            }
        }
        requests
    }

    /// 外部の変更通知を受けた。個体を所有する機能へ再取得を促す。
    pub(crate) fn changed(&mut self, cx: &mut App) {
        for lifecycle in self.lifecycles.values() {
            lifecycle.borrow_mut().changed(cx);
        }
    }

    pub(crate) fn instance_view(&self, instance: &ViewInstanceId) -> Option<&AnyView> {
        self.instances.get(instance).map(|(_, entity)| entity)
    }

    pub(crate) fn close_view_instance(&mut self, instance: &ViewInstanceId) {
        // 表示Entityへの参照だけを解放する。Resourceの終了APIは呼ばない。
        self.instances.remove(instance);
    }

    pub(crate) fn set_visible_views(&mut self, views: impl IntoIterator<Item = ViewId>) {
        self.visible_views = views.into_iter().collect();
        self.recompute();
    }

    pub fn register(&mut self, descriptor: FeatureDescriptor) -> Result<(), FeatureId> {
        if self.descriptors.contains_key(&descriptor.id) {
            return Err(descriptor.id);
        }
        let is_core = descriptor.kind == FeatureKind::Core;
        self.states.insert(
            descriptor.id.clone(),
            FeatureState {
                installed: true,
                loaded: is_core,
                running: is_core,
            },
        );
        self.descriptors.insert(descriptor.id.clone(), descriptor);
        Ok(())
    }

    pub fn state(&self, id: &str) -> FeatureState {
        self.states
            .iter()
            .find_map(|(feature_id, state)| (feature_id.as_str() == id).then_some(*state))
            .unwrap_or_default()
    }

    pub fn descriptor(&self, id: &str) -> Option<&FeatureDescriptor> {
        self.descriptors
            .iter()
            .find_map(|(feature_id, descriptor)| (feature_id.as_str() == id).then_some(descriptor))
    }

    pub fn is_running(&self, id: &str) -> bool {
        self.state(id).running
    }

    pub fn contributions(&self, slot: UiSlot) -> Vec<&UiContribution> {
        let mut contributions = self
            .descriptors
            .values()
            .flat_map(|descriptor| descriptor.contributions.iter())
            .filter(|contribution| contribution.slot == slot)
            .collect::<Vec<_>>();
        contributions.sort_by_key(|contribution| contribution.order);
        contributions
    }

    pub fn zone_contributions(&self, position: ZonePosition) -> Vec<&UiContribution> {
        let workspace_open = self.workspace_open;
        let capabilities = &self.capabilities;
        let mut contributions = self
            .descriptors
            .values()
            .flat_map(|descriptor| descriptor.contributions.iter())
            .filter(|contribution| {
                contribution.view.is_some()
                    && contribution.allowed_zones.contains(&position)
                    && is_visible(&contribution.visibility, workspace_open, capabilities)
            })
            .collect::<Vec<_>>();
        contributions.sort_by_key(|contribution| contribution.order);
        contributions
    }

    pub fn tool_contributions(&self, position: ZonePosition) -> Vec<&UiContribution> {
        self.zone_contributions(position)
            .into_iter()
            .filter(|contribution| {
                contribution.slot != UiSlot::SettingsPage && contribution.show_in_picker
            })
            .collect()
    }

    pub fn set_active_view(&mut self, slot: UiSlot, view: Option<ViewId>) {
        match view {
            Some(view) => {
                self.active_views.insert(slot, view);
            }
            None => {
                self.active_views.remove(&slot);
            }
        }
        self.recompute();
    }

    pub fn set_zone_active_view(&mut self, zone: ZonePosition, view: Option<ViewId>) {
        let slot = match zone {
            ZonePosition::Left => UiSlot::ToolDock,
            ZonePosition::Main => UiSlot::EditorDecoration,
            ZonePosition::Bottom => UiSlot::BottomDock,
            ZonePosition::Right => UiSlot::SideDock,
        };
        self.set_active_view(slot, view);
    }

    pub fn activate(&mut self, event: ActivationEvent) {
        match event {
            ActivationEvent::WorkspaceOpened => self.workspace_open = true,
            ActivationEvent::LanguageChanged(language) => self.active_language = language,
            ActivationEvent::WorkspaceCapabilityChanged(capability, enabled) => {
                if enabled {
                    self.capabilities.insert(capability);
                } else {
                    self.capabilities.remove(&capability);
                }
            }
            ActivationEvent::CommandInvoked(command) => {
                self.active_commands.insert(command);
            }
        }
        self.recompute();
    }

    pub fn deactivate_command(&mut self, command: &CommandId) {
        self.active_commands.remove(command);
        self.recompute();
    }

    pub fn set_command_active(&mut self, command: CommandId, active: bool) {
        if active {
            self.active_commands.insert(command);
        } else {
            self.active_commands.remove(&command);
        }
        self.recompute();
    }

    fn recompute(&mut self) {
        for descriptor in self.descriptors.values() {
            let Some(state) = self.states.get_mut(&descriptor.id) else {
                continue;
            };
            if descriptor.kind == FeatureKind::Core {
                state.loaded = true;
                state.running = true;
                continue;
            }
            let should_run = descriptor
                .activation
                .iter()
                .any(|condition| match condition {
                    ActivationCondition::OnWorkspaceOpen => self.workspace_open,
                    ActivationCondition::OnLanguage(language) => {
                        self.active_language.as_ref() == Some(language)
                    }
                    ActivationCondition::OnCommand(command) => {
                        self.active_commands.contains(command)
                    }
                    ActivationCondition::OnView(view) => {
                        self.visible_views.contains(view)
                            || self.active_views.values().any(|active| active == view)
                    }
                    ActivationCondition::OnWorkspaceCapability(capability) => {
                        self.capabilities.contains(capability)
                    }
                });
            state.loaded |= should_run;
            state.running = should_run;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_contributions_remain_available_for_layout() {
        let mut registry = FeatureRegistry::default();
        registry
            .register(
                FeatureDescriptor::core("core.test").contributes(
                    UiContribution::view("view.hidden", UiSlot::SideDock, "title", "icon", 0)
                        .hidden_in_picker(),
                ),
            )
            .unwrap();
        assert_eq!(registry.zone_contributions(ZonePosition::Right).len(), 1);
        assert!(registry.tool_contributions(ZonePosition::Right).is_empty());
    }

    use extension_ui_contract::{IconId, MessageId};

    struct InstanceCounter(usize);
    impl gpui::Render for InstanceCounter {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            gpui::div()
        }
    }

    /// 個体を作る機能の最小実装。作った Entity を記録する。
    #[derive(Default)]
    struct CounterLifecycle {
        created: Vec<gpui::Entity<InstanceCounter>>,
    }

    impl ViewLifecycle for CounterLifecycle {
        fn create(&mut self, _: &ViewInstanceId, cx: &mut App) -> Option<AnyView> {
            use gpui::AppContext;
            let entity = cx.new(|_| InstanceCounter(0));
            self.created.push(entity.clone());
            Some(entity.into())
        }
    }

    #[gpui::test]
    fn multiple_views_own_independent_entities_and_reuse_the_same_instance(
        cx: &mut gpui::TestAppContext,
    ) {
        let mut registry = FeatureRegistry::default();
        let view = ViewId::new("counter");
        registry
            .register(
                FeatureDescriptor::core("counter").contributes(
                    UiContribution::view(view.clone(), UiSlot::ToolDock, "counter", "counter", 0)
                        .multiple_instances(),
                ),
            )
            .unwrap();
        // 宣言だけではMultipleを有効化しない。個体を所有する機能の登録が要る。
        assert!(!registry.can_create_view(&view, ToolInstancePolicy::Multiple));
        let lifecycle = Rc::new(RefCell::new(CounterLifecycle::default()));
        registry.register_view_lifecycle(view.clone(), lifecycle.clone());
        assert!(registry.can_create_view(&view, ToolInstancePolicy::Multiple));
        let first = ViewInstanceId::new("one");
        let second = ViewInstanceId::new("two");
        cx.update(|cx| {
            assert!(registry.ensure_view_instance(&view, &first, cx));
            assert!(registry.ensure_view_instance(&view, &second, cx));
            assert!(registry.ensure_view_instance(&view, &first, cx));
            let created = &lifecycle.borrow().created;
            assert_eq!(created.len(), 2);
            let first_entity = created[0].clone();
            let second_entity = created[1].clone();
            first_entity.update(cx, |counter, _| counter.0 = 7);
            assert_eq!(first_entity.read(cx).0, 7);
            assert_eq!(second_entity.read(cx).0, 0);
            assert_ne!(
                registry.instance_view(&first).unwrap().entity_id(),
                registry.instance_view(&second).unwrap().entity_id()
            );
        });
    }

    #[gpui::test]
    fn shared_views_reuse_registered_entity(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext;
        cx.update(|cx| {
            let mut registry = FeatureRegistry::default();
            let view = ViewId::new("shared");
            registry
                .register(
                    FeatureDescriptor::core("shared").contributes(UiContribution::view(
                        view.clone(),
                        UiSlot::ToolDock,
                        "shared",
                        "shared",
                        0,
                    )),
                )
                .unwrap();
            let entity = cx.new(|_| InstanceCounter(0));
            registry.register_shared_view(view.clone(), entity.clone().into());
            let instance = shared_tool_instance(&view);
            assert!(registry.ensure_view_instance(&view, &instance, cx));
            assert!(registry.ensure_view_instance(&view, &instance, cx));
            assert_eq!(
                registry.instance_view(&instance).unwrap().entity_id(),
                entity.entity_id()
            );
        });
    }

    #[test]
    fn bundled_feature_loads_only_when_its_view_is_active() {
        let mut registry = FeatureRegistry::default();
        registry
            .register(
                FeatureDescriptor::bundled(
                    "feature.git",
                    [ActivationCondition::OnView(ViewId::new("view.git"))],
                )
                .contributes(UiContribution::view(
                    "view.git",
                    UiSlot::ToolDock,
                    MessageId::new("view.git.title"),
                    IconId::new("git-branch"),
                    20,
                )),
            )
            .unwrap();

        assert_eq!(
            registry.state("feature.git"),
            FeatureState {
                installed: true,
                loaded: false,
                running: false,
            }
        );
        registry.set_active_view(UiSlot::ToolDock, Some(ViewId::new("view.git")));
        assert!(registry.state("feature.git").running);
        registry.set_active_view(UiSlot::ToolDock, Some(ViewId::new("view.files")));
        assert!(registry.state("feature.git").loaded);
        assert!(!registry.state("feature.git").running);
    }

    #[test]
    fn contributions_are_ordered_independently_from_registration() {
        let mut registry = FeatureRegistry::default();
        for (id, order) in [("view.second", 20), ("view.first", 10)] {
            registry
                .register(
                    FeatureDescriptor::bundled(
                        format!("feature.{id}"),
                        [ActivationCondition::OnView(ViewId::new(id))],
                    )
                    .contributes(UiContribution::view(
                        id,
                        UiSlot::ToolDock,
                        format!("{id}.title"),
                        "view",
                        order,
                    )),
                )
                .unwrap();
        }
        let views = registry
            .contributions(UiSlot::ToolDock)
            .into_iter()
            .filter_map(|item| item.view.as_ref().map(ViewId::as_str))
            .collect::<Vec<_>>();
        assert_eq!(views, ["view.first", "view.second"]);
    }

    #[test]
    fn command_activation_runs_until_the_command_finishes() {
        let mut registry = FeatureRegistry::default();
        let command = CommandId::new("command.search");
        registry
            .register(FeatureDescriptor::bundled(
                "feature.search",
                [ActivationCondition::OnCommand(command.clone())],
            ))
            .unwrap();

        registry.activate(ActivationEvent::CommandInvoked(command.clone()));
        registry.set_active_view(UiSlot::ToolDock, Some(ViewId::new("view.search")));
        assert!(registry.state("feature.search").running);

        registry.deactivate_command(&command);
        assert!(registry.state("feature.search").loaded);
        assert!(!registry.state("feature.search").running);
    }

    #[test]
    fn picker_contributions_respect_visibility_conditions() {
        let view = ViewId::new("view.workspace-only");
        let mut registry = FeatureRegistry::default();
        registry
            .register(
                FeatureDescriptor::bundled(
                    "feature.workspace-only",
                    [ActivationCondition::OnWorkspaceOpen],
                )
                .contributes(
                    UiContribution::view(
                        view.clone(),
                        UiSlot::ToolDock,
                        "view.workspace-only.title",
                        "workspace",
                        1,
                    )
                    .visible_when_workspace_open(),
                ),
            )
            .unwrap();

        assert!(registry.zone_contributions(ZonePosition::Left).is_empty());
        registry.activate(ActivationEvent::WorkspaceOpened);
        assert_eq!(
            registry
                .zone_contributions(ZonePosition::Left)
                .first()
                .and_then(|contribution| contribution.view.as_ref()),
            Some(&view)
        );
    }
}
