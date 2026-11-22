use gpui::{AnyView, App};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::rc::Rc;

use extension_ui_contract::{
    ActivationCondition, CommandId, ContributedView, FeatureDescriptor, FeatureId, FeatureKind,
    ToolInstancePolicy, UiContribution, UiSlot, ViewId, ViewInstanceId, ViewVisibility,
    WorkspaceCapabilityId, ZonePosition,
};

use super::ActivationEvent;

/// タブの表示状態と Resource を扱う機能からの要求。
///
/// 機能は Layout を所有しない。タブの Resource 差し替えや新しいタブの追加は
/// 要求として返し、Shell が Layout に適用する。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewRequest {
    /// タブの参照先 Resource を差し替える。個体 ID は変えない。
    BindResource {
        instance: ViewInstanceId,
        resource: Option<String>,
    },
    /// 同じ View のタブを1つ増やす。
    OpenTab,
}

/// 宣言された View の個体を所有する機能が実装する内部契約。
///
/// [`FeatureRegistry`] は個体 ID と表示状態を解決し、Entity の生成・表示状態の
/// 反映・Resource の接続と終了はこの契約の実装（feature 自身）が行う。
/// Shell は View の種類で分岐せず、この契約だけを呼ぶ。
pub trait ViewLifecycle {
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

    /// 明示的に閉じると、実行中または状態未確定の Resource が失われるか。
    fn requires_close_confirmation(&self, _instance: &ViewInstanceId, _cx: &App) -> bool {
        false
    }

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

pub type SharedLifecycle = Rc<RefCell<dyn ViewLifecycle>>;

/// 機能から Shell への再同期通知。Shell が既に解放されていれば `false`。
pub type ViewNotifier = Rc<dyn Fn(&mut App) -> bool>;

/// 単一実体を共有する View の個体 ID。個体の命名は Client 側の配置規則とする。
pub fn shared_tool_instance(view: &ViewId) -> ViewInstanceId {
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
    unavailable_views: HashSet<ViewId>,
    view_open_commands: HashMap<ViewId, CommandId>,
    view_aliases: HashMap<ViewId, ViewId>,
    /// 提供元 ID ごとの宣言済み View。差し替えと OFF に使う。
    provider_views: HashMap<String, Vec<ViewId>>,
    /// View とクライアント描画器の種別。提供元とは独立する。
    content_types: HashMap<ViewId, String>,
    /// クライアント native 描画器の登録。提供元ごとではなく共有する。
    content_renderers: HashMap<String, SharedLifecycle>,
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
    /// 保存済みの旧 View ID を現在登録されている ID に解決する。
    pub fn resolve_view(&self, view: &ViewId) -> ViewId {
        self.view_aliases
            .get(view)
            .cloned()
            .unwrap_or_else(|| view.clone())
    }

    /// 描画器の登録名を宣言 ID に移し、既存の Entity と個体 ID を維持する。
    pub fn can_rekey_view(&self, old: &ViewId, new: &ViewId) -> bool {
        self.declaration(old).is_some()
            && (old == new
                || (self.declaration(new).is_none()
                    && !self.lifecycles.contains_key(new)
                    && !self.shared_views.contains_key(new)
                    && self.view_aliases.get(new).is_none_or(|owner| owner == old)))
    }
    pub fn rekey_view(&mut self, old: &ViewId, new: &ViewId) -> bool {
        if !self.can_rekey_view(old, new) {
            return false;
        }
        if old == new {
            return true;
        }
        for descriptor in self.descriptors.values_mut() {
            for contribution in &mut descriptor.contributions {
                if contribution.view.as_ref() == Some(old) {
                    contribution.view = Some(new.clone());
                }
            }
            for activation in &mut descriptor.activation {
                if let ActivationCondition::OnView(view) = activation
                    && view == old
                {
                    *view = new.clone();
                }
            }
        }
        if let Some(value) = self.lifecycles.remove(old) {
            self.lifecycles.insert(new.clone(), value);
        }
        if let Some(value) = self.shared_views.remove(old) {
            self.shared_views.insert(new.clone(), value);
        }
        if let Some(value) = self.view_open_commands.remove(old) {
            self.view_open_commands.insert(new.clone(), value);
        }
        if self.started.remove(old) {
            self.started.insert(new.clone());
        }
        if self.unavailable_views.remove(old) {
            self.unavailable_views.insert(new.clone());
        }
        if self.visible_views.remove(old) {
            self.visible_views.insert(new.clone());
        }
        for value in self.active_views.values_mut() {
            if value == old {
                *value = new.clone();
            }
        }
        for (owner, _) in self.instances.values_mut() {
            if owner == old {
                *owner = new.clone();
            }
        }
        for value in self.view_aliases.values_mut() {
            if value == old {
                *value = new.clone();
            }
        }
        self.view_aliases.remove(new);
        self.view_aliases.insert(old.clone(), new.clone());
        self.recompute();
        true
    }
    pub fn set_view_default_zone(&mut self, view: &ViewId, zone: ZonePosition) {
        if let Some(contribution) = self
            .descriptors
            .values_mut()
            .flat_map(|feature| &mut feature.contributions)
            .find(|contribution| contribution.view.as_ref() == Some(view))
        {
            contribution.default_zone = Some(zone);
        }
    }

    pub fn set_view_instance_policy(&mut self, view: &ViewId, policy: ToolInstancePolicy) {
        if let Some(contribution) = self
            .descriptors
            .values_mut()
            .flat_map(|feature| &mut feature.contributions)
            .find(|contribution| contribution.view.as_ref() == Some(view))
        {
            contribution.instance_policy = policy;
        }
    }

    /// View を開く公開コマンドを、その描画器が登録された View に結び付ける。
    pub fn set_view_open_command(&mut self, view: ViewId, command: Option<CommandId>) -> bool {
        if let Some(command) = command.as_ref() {
            let conflicts = self
                .descriptors
                .values()
                .flat_map(|feature| &feature.contributions)
                .any(|contribution| contribution.command.as_ref() == Some(command))
                || self
                    .view_open_commands
                    .iter()
                    .any(|(other, existing)| *other != view && existing == command);
            if conflicts {
                self.view_open_commands.remove(&view);
                return false;
            }
        }
        if let Some(command) = command {
            self.view_open_commands.insert(view, command);
        } else {
            self.view_open_commands.remove(&view);
        }
        true
    }

    pub fn view_open_commands(&self) -> Vec<(&ViewId, &CommandId)> {
        let mut commands = self
            .view_open_commands
            .iter()
            .filter(|(view, _)| {
                self.declaration(view).is_some_and(|declaration| {
                    self.can_create_view(view, declaration.instance_policy)
                })
            })
            .collect::<Vec<_>>();
        commands.sort_by(|a, b| a.1.as_str().cmp(b.1.as_str()));
        commands
    }

    pub fn view_for_open_command(&self, command: &CommandId) -> Option<ViewId> {
        self.view_open_commands()
            .into_iter()
            .find_map(|(view, candidate)| (candidate == command).then(|| view.clone()))
    }
    pub fn register_shared_view(&mut self, view: ViewId, entity: AnyView) {
        self.shared_views.insert(view, entity);
    }

    /// 個体を所有する機能の実体を登録する。Multiple はこの実体が作る Entity だけで描画する。
    pub fn register_view_lifecycle(
        &mut self,
        view: ViewId,
        lifecycle: Rc<RefCell<dyn ViewLifecycle>>,
    ) {
        self.lifecycles.insert(view, lifecycle);
    }

    /// 署名済み宣言を提供元単位で登録する。同じ提供元の旧宣言を置換する。
    pub fn register_provider(
        &mut self,
        provider: &str,
        views: impl IntoIterator<Item = (ContributedView, UiContribution)>,
    ) -> Result<(), FeatureId> {
        self.remove_provider(provider);
        let mut views = views.into_iter();
        let first = views.next();
        if first.is_none() {
            self.provider_views.insert(provider.to_owned(), Vec::new());
            return Ok(());
        }
        let mut descriptor = FeatureDescriptor::core(format!("extension.{provider}"));
        let mut view_ids = Vec::new();
        for (view, contribution) in first.into_iter().chain(views) {
            let id = ViewId::new(view.id);
            self.content_types.insert(id.clone(), view.content_type);
            view_ids.push(id);
            descriptor = descriptor.contributes(contribution);
        }
        self.register(descriptor)?;
        self.provider_views.insert(provider.to_owned(), view_ids);
        Ok(())
    }

    /// 提供元の宣言を除去する。タブと個体は残し、表示可否だけを再評価する。
    pub fn remove_provider(&mut self, provider: &str) -> Vec<ViewId> {
        let Some(views) = self.provider_views.remove(provider) else {
            return Vec::new();
        };
        for view in &views {
            self.view_open_commands.remove(view);
            self.unavailable_views.remove(view);
            self.lifecycles.remove(view);
            self.started.remove(view);
            self.content_types.remove(view);
        }
        self.descriptors
            .remove(&FeatureId::new(format!("extension.{provider}")));
        self.states
            .remove(&FeatureId::new(format!("extension.{provider}")));
        self.recompute();
        views
    }

    /// 現在の目録にある提供元を、現在稼働中の公開状態として反映する。
    ///
    /// 目録に存在するが稼働していない提供元は、セッションと描画器を保持した
    /// まま公開面から隠す。目録から消えた提供元だけを宣言ごと除去する。
    pub fn sync_provider_availability(
        &mut self,
        live: impl IntoIterator<Item = String>,
        catalog: impl IntoIterator<Item = String>,
    ) -> Vec<ViewId> {
        let live = live.into_iter().collect::<BTreeSet<_>>();
        let catalog = catalog.into_iter().collect::<BTreeSet<_>>();
        let unavailable = self
            .provider_views
            .iter()
            .filter(|(provider, _)| catalog.contains(*provider) && !live.contains(*provider))
            .flat_map(|(_, views)| views.iter().cloned())
            .collect::<Vec<_>>();
        for view in &unavailable {
            self.set_view_available(view.clone(), false);
        }
        self.provider_views
            .keys()
            .filter(|provider| !catalog.contains(*provider))
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .flat_map(|provider| self.remove_provider(&provider))
            .collect()
    }

    /// 提供元の宣言を現在の有効状態へ反映する。
    ///
    /// 既に登録済みの View は再利用し、描画器の登録を失わない。無効化された
    /// 提供元だけを除去し、同じ `content_type` を持つ別提供元は残す。
    pub fn sync_providers(
        &mut self,
        declarations: impl IntoIterator<Item = (String, Vec<(ContributedView, UiContribution)>)>,
    ) -> Result<Vec<ViewId>, FeatureId> {
        let declarations = declarations.into_iter().collect::<Vec<_>>();
        let active = declarations
            .iter()
            .map(|(provider, _)| provider.as_str())
            .collect::<BTreeSet<_>>();
        let stale = self
            .provider_views
            .keys()
            .filter(|provider| !active.contains(provider.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        let mut removed = Vec::new();
        for provider in stale {
            removed.extend(self.remove_provider(&provider));
        }
        for (provider, views) in declarations {
            let next_views = views
                .iter()
                .map(|(view, _)| ViewId::new(view.id.clone()))
                .collect::<Vec<_>>();
            if self.provider_views.get(&provider) == Some(&next_views) {
                continue;
            }
            self.register_provider(&provider, views)?;
        }
        self.reconnect_registered_renderers();
        Ok(removed)
    }

    /// コンテンツ種別に対応する描画器を、宣言済みの全 View へ配線する。
    pub fn connect_renderer(
        &mut self,
        content_type: &str,
        renderer: SharedLifecycle,
    ) -> Vec<ViewId> {
        let views = self
            .provider_views
            .values()
            .flatten()
            .filter(|view| {
                self.declaration(view)
                    .is_some_and(|_| self.content_type(view) == Some(content_type))
            })
            .cloned()
            .collect::<Vec<_>>();
        for view in &views {
            self.lifecycles.insert(view.clone(), renderer.clone());
        }
        views
    }

    pub fn register_content_renderer(
        &mut self,
        content_type: impl Into<String>,
        renderer: SharedLifecycle,
    ) {
        self.content_renderers.insert(content_type.into(), renderer);
    }

    /// 追加された提供元の View へ、既に登録済みの描画器を接続する。
    pub fn reconnect_registered_renderers(&mut self) {
        let renderers = self
            .content_renderers
            .iter()
            .map(|(kind, renderer)| (kind.clone(), renderer.clone()))
            .collect::<Vec<_>>();
        for (kind, renderer) in renderers {
            self.connect_renderer(&kind, renderer);
        }
    }

    pub fn content_type(&self, view: &ViewId) -> Option<&str> {
        self.content_types.get(view).map(String::as_str)
    }

    /// 署名済み宣言から `UiContribution` の公開属性を更新する。
    pub fn set_view_presentation(
        &mut self,
        view: &ViewId,
        default_zone: ZonePosition,
        multiple_instances: bool,
        open_command: Option<CommandId>,
    ) {
        self.set_view_default_zone(view, default_zone);
        self.set_view_instance_policy(
            view,
            if multiple_instances {
                ToolInstancePolicy::Multiple
            } else {
                ToolInstancePolicy::Shared
            },
        );
        let _ = self.set_view_open_command(view.clone(), open_command);
    }

    pub fn provider_view_ids(&self) -> impl Iterator<Item = &ViewId> {
        self.provider_views.values().flatten()
    }

    pub fn can_create_view(&self, view: &ViewId, policy: ToolInstancePolicy) -> bool {
        if self.unavailable_views.contains(view)
            || self
                .content_types
                .get(view)
                .is_some_and(|content_type| !self.content_renderers.contains_key(content_type))
        {
            return false;
        }
        match policy {
            ToolInstancePolicy::Shared => {
                self.shared_views.contains_key(view) || self.lifecycles.contains_key(view)
            }
            ToolInstancePolicy::Multiple => self.lifecycles.contains_key(view),
        }
    }

    /// 提供元の現在の稼働状態を反映する。既存の個体は切断時に終了しない。
    pub fn set_view_available(&mut self, view: ViewId, available: bool) {
        if available {
            self.unavailable_views.remove(&view);
        } else {
            self.unavailable_views.insert(view);
        }
    }

    /// View の宣言。タブの追加方針や Resource 参照の宣言を Shell が読む。
    pub fn declaration(&self, view: &ViewId) -> Option<&UiContribution> {
        self.descriptors
            .values()
            .flat_map(|feature| &feature.contributions)
            .find(|contribution| contribution.view.as_ref() == Some(view))
    }

    /// 個体を所有する機能が登録された View。
    pub fn lifecycle_views(&self) -> Vec<ViewId> {
        self.lifecycles.keys().cloned().collect()
    }

    /// 個体を所有する機能を引く。
    pub fn lifecycle(&self, view: &ViewId) -> Option<&SharedLifecycle> {
        self.lifecycles.get(view)
    }

    /// 機能が現在持っている個体。Layout から消えた個体の解放に使う。
    pub fn view_instances(&self, view: &ViewId) -> Vec<ViewInstanceId> {
        self.instances
            .iter()
            .filter(|(_, (owner, _))| owner == view)
            .map(|(instance, _)| instance.clone())
            .collect()
    }

    /// 機能の購読を1回だけ始める。backend の通知は receiver を1度しか返さない。
    pub fn ensure_started(&mut self, view: &ViewId, notify: ViewNotifier, cx: &mut App) {
        if !self.started.insert(view.clone()) {
            return;
        }
        if let Some(lifecycle) = self.lifecycles.get_mut(view) {
            lifecycle.borrow_mut().start(notify, cx);
        }
    }

    pub fn ensure_view_instance(
        &mut self,
        view: &ViewId,
        instance: &ViewInstanceId,
        cx: &mut App,
    ) -> bool {
        if let Some((definition, _)) = self.instances.get(instance) {
            return definition == view;
        }
        if self.unavailable_views.contains(view)
            || self
                .content_types
                .get(view)
                .is_some_and(|content_type| !self.content_renderers.contains_key(content_type))
        {
            return false;
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
            Some(ToolInstancePolicy::Shared) => {
                self.shared_views.get(view).cloned().or_else(|| {
                    self.lifecycles
                        .get(view)
                        .and_then(|lifecycle| lifecycle.borrow_mut().create(instance, cx))
                })
            }
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
    pub fn activate_instance(
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
    pub fn release_instance(&mut self, view: &ViewId, instance: &ViewInstanceId, cx: &mut App) {
        if let Some(lifecycle) = self.lifecycles.get_mut(view) {
            lifecycle.borrow_mut().release(instance);
        }
        self.instances.remove(instance);
        let _ = cx;
    }

    /// タブを閉じた。個体を所有する機能へ終了を伝える。
    pub fn deactivate_instance(&mut self, view: &ViewId, instance: &ViewInstanceId, cx: &mut App) {
        if let Some(lifecycle) = self.lifecycles.get_mut(view) {
            lifecycle.borrow_mut().deactivate(instance, cx);
        }
        self.instances.remove(instance);
    }

    /// 個体を所有する機能から Layout への要求を取り出す。
    pub fn take_view_requests(&mut self) -> Vec<(ViewId, ViewRequest)> {
        let mut requests = Vec::new();
        for (view, lifecycle) in &self.lifecycles {
            for request in lifecycle.borrow_mut().take_requests() {
                requests.push((view.clone(), request));
            }
        }
        requests
    }

    /// 外部の変更通知を受けた。個体を所有する機能へ再取得を促す。
    pub fn changed(&mut self, cx: &mut App) {
        for lifecycle in self.lifecycles.values() {
            lifecycle.borrow_mut().changed(cx);
        }
    }

    pub fn instance_view(&self, instance: &ViewInstanceId) -> Option<&AnyView> {
        self.instances.get(instance).map(|(_, entity)| entity)
    }

    pub fn close_view_instance(&mut self, instance: &ViewInstanceId) {
        // 表示Entityへの参照だけを解放する。Resourceの終了APIは呼ばない。
        self.instances.remove(instance);
    }

    pub fn set_visible_views(&mut self, views: impl IntoIterator<Item = ViewId>) {
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
            .filter(|contribution| {
                contribution.slot == slot
                    && contribution
                        .view
                        .as_ref()
                        .is_none_or(|view| !self.unavailable_views.contains(view))
            })
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

    #[test]
    fn rekey_preserves_aliases_and_prevents_reusing_another_views_old_id() {
        let mut registry = FeatureRegistry::default();
        for (feature, view) in [("one", "example.one"), ("two", "example.two")] {
            registry
                .register(
                    FeatureDescriptor::core(feature).contributes(UiContribution::view(
                        view,
                        UiSlot::BottomDock,
                        "title",
                        "icon",
                        0,
                    )),
                )
                .unwrap();
        }
        let one = ViewId::new("example.one");
        let renamed = ViewId::new("example.renamed");
        let two = ViewId::new("example.two");
        assert!(registry.rekey_view(&one, &renamed));
        assert!(!registry.rekey_view(&two, &one));
        assert_eq!(registry.resolve_view(&one), renamed);
        assert!(registry.rekey_view(&renamed, &one));
        assert_eq!(registry.resolve_view(&renamed), one);
    }
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

    #[test]
    fn declared_open_command_tracks_availability_and_rejects_collisions() {
        let mut registry = FeatureRegistry::default();
        let view = ViewId::new("example.view");
        registry
            .register(
                FeatureDescriptor::core("example").contributes(
                    UiContribution::view(view.clone(), UiSlot::BottomDock, "title", "icon", 0)
                        .multiple_instances(),
                ),
            )
            .unwrap();
        registry.register_view_lifecycle(
            view.clone(),
            Rc::new(RefCell::new(CounterLifecycle::default())),
        );
        let command = CommandId::new("example.open");
        assert!(registry.set_view_open_command(view.clone(), Some(command.clone())));
        assert_eq!(registry.view_for_open_command(&command), Some(view.clone()));
        registry.set_view_available(view.clone(), false);
        assert_eq!(registry.view_for_open_command(&command), None);
        registry.set_view_available(view.clone(), true);
        assert_eq!(registry.view_for_open_command(&command), Some(view.clone()));
        let other = ViewId::new("other.view");
        assert!(!registry.set_view_open_command(other, Some(command.clone())));
        assert_eq!(registry.view_for_open_command(&command), Some(view.clone()));
        assert!(registry.set_view_open_command(view, None));
        assert_eq!(registry.view_for_open_command(&command), None);
    }

    #[gpui::test]
    fn unavailable_view_keeps_existing_instances_and_blocks_new_ones(
        cx: &mut gpui::TestAppContext,
    ) {
        let mut registry = FeatureRegistry::default();
        let view = ViewId::new("example.view");
        registry
            .register(
                FeatureDescriptor::core("example").contributes(
                    UiContribution::view(view.clone(), UiSlot::BottomDock, "title", "icon", 0)
                        .multiple_instances(),
                ),
            )
            .unwrap();
        registry.register_view_lifecycle(
            view.clone(),
            Rc::new(RefCell::new(CounterLifecycle::default())),
        );
        let first = ViewInstanceId::new("one");
        let second = ViewInstanceId::new("two");
        cx.update(|cx| {
            assert!(registry.ensure_view_instance(&view, &first, cx));
            registry.set_view_available(view.clone(), false);
            assert!(!registry.can_create_view(&view, ToolInstancePolicy::Multiple));
            assert!(registry.ensure_view_instance(&view, &first, cx));
            assert!(!registry.ensure_view_instance(&view, &second, cx));
            assert!(registry.instance_view(&first).is_some());
            registry.set_view_available(view.clone(), true);
            assert!(registry.ensure_view_instance(&view, &second, cx));
        });
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

    #[test]
    fn providers_share_a_renderer_by_content_type_and_off_removes_public_views() {
        let content_type = "example.grid.v1";
        let renderer = Rc::new(RefCell::new(CounterLifecycle::default()));
        let mut registry = FeatureRegistry::default();
        registry.register_content_renderer(content_type, renderer.clone());
        let declaration = |owner: &str, id: &str| {
            let view = ContributedView {
                id: id.into(),
                content_type: content_type.into(),
                title: "title".into(),
                default_zone: extension_ui_contract::ContributedZone::Bottom,
                multiple_instances: true,
                open_command: Some(format!("{owner}.open")),
            };
            (
                view.clone(),
                UiContribution::view(view.id, UiSlot::BottomDock, view.title, "terminal", 10)
                    .multiple_instances()
                    .references_external_resource(),
            )
        };
        registry
            .sync_providers(vec![
                (
                    "example.one".into(),
                    vec![declaration("example.one", "example.one.view")],
                ),
                (
                    "example.two".into(),
                    vec![declaration("example.two", "example.two.view")],
                ),
            ])
            .unwrap();
        let first = ViewId::new("example.one.view");
        let second = ViewId::new("example.two.view");
        assert_eq!(registry.content_type(&first), Some(content_type));
        assert_eq!(registry.content_type(&second), Some(content_type));
        assert!(registry.lifecycle(&first).is_some());
        assert!(registry.lifecycle(&second).is_some());
        assert_eq!(registry.provider_view_ids().count(), 2);

        registry
            .sync_providers(vec![(
                "example.two".into(),
                vec![declaration("example.two", "example.two.view")],
            )])
            .unwrap();
        assert!(registry.declaration(&first).is_none());
        assert!(registry.declaration(&second).is_some());
        assert!(registry.content_type(&first).is_none());
        assert!(registry.content_type(&second).is_some());
    }
}
