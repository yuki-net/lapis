use gpui::{AnyView, App};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

use lapis_language::LanguageId;

use super::{
    ActivationCondition, ActivationEvent, CommandId, FeatureId, UiContribution, UiSlot, ViewId,
    WorkspaceCapabilityId, ZonePosition,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeatureKind {
    Core,
    Bundled,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeatureDescriptor {
    pub id: FeatureId,
    pub kind: FeatureKind,
    pub activation: Vec<ActivationCondition>,
    pub contributions: Vec<UiContribution>,
    pub required_capabilities: Vec<WorkspaceCapabilityId>,
}

impl FeatureDescriptor {
    pub fn core(id: impl Into<FeatureId>) -> Self {
        Self {
            id: id.into(),
            kind: FeatureKind::Core,
            activation: Vec::new(),
            contributions: Vec::new(),
            required_capabilities: Vec::new(),
        }
    }

    pub fn bundled(
        id: impl Into<FeatureId>,
        activation: impl IntoIterator<Item = ActivationCondition>,
    ) -> Self {
        Self {
            id: id.into(),
            kind: FeatureKind::Bundled,
            activation: activation.into_iter().collect(),
            contributions: Vec::new(),
            required_capabilities: Vec::new(),
        }
    }

    pub fn contributes(mut self, contribution: UiContribution) -> Self {
        self.contributions.push(contribution);
        self
    }

    pub fn requires(mut self, capability: impl Into<WorkspaceCapabilityId>) -> Self {
        self.required_capabilities.push(capability.into());
        self
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FeatureState {
    pub installed: bool,
    pub loaded: bool,
    pub running: bool,
}

#[derive(Default)]
pub struct FeatureRegistry {
    descriptors: BTreeMap<FeatureId, FeatureDescriptor>,
    states: HashMap<FeatureId, FeatureState>,
    active_views: HashMap<UiSlot, ViewId>,
    visible_views: HashSet<ViewId>,
    shared_views: HashMap<ViewId, AnyView>,
    view_factories: HashMap<ViewId, ViewFactory>,
    instances: HashMap<super::ViewInstanceId, (ViewId, AnyView)>,
    active_language: Option<LanguageId>,
    workspace_open: bool,
    capabilities: HashSet<WorkspaceCapabilityId>,
    active_commands: HashSet<CommandId>,
}

type ViewFactory = Rc<dyn Fn(&mut App) -> AnyView>;

impl FeatureRegistry {
    pub(crate) fn register_shared_view(&mut self, view: ViewId, entity: AnyView) {
        self.shared_views.insert(view, entity);
    }

    /// MultipleはFactoryが提供する独立Entityだけで描画する。
    #[allow(dead_code)] // Plugin・Featureの登録入口。
    pub(crate) fn register_view_factory(&mut self, view: ViewId, factory: ViewFactory) {
        self.view_factories.insert(view, factory);
    }

    pub(crate) fn can_create_view(&self, view: &ViewId, policy: super::ToolInstancePolicy) -> bool {
        match policy {
            super::ToolInstancePolicy::Shared => self.shared_views.contains_key(view),
            super::ToolInstancePolicy::Multiple => self.view_factories.contains_key(view),
        }
    }

    pub(crate) fn ensure_view_instance(
        &mut self,
        view: &ViewId,
        instance: &super::ViewInstanceId,
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
            Some(super::ToolInstancePolicy::Multiple) => {
                self.view_factories.get(view).map(|factory| factory(cx))
            }
            Some(super::ToolInstancePolicy::Shared) => self.shared_views.get(view).cloned(),
            None => None,
        };
        let Some(entity) = entity else {
            return false;
        };
        self.instances
            .insert(instance.clone(), (view.clone(), entity));
        true
    }

    pub(crate) fn instance_view(&self, instance: &super::ViewInstanceId) -> Option<&AnyView> {
        self.instances.get(instance).map(|(_, entity)| entity)
    }

    pub(crate) fn close_view_instance(&mut self, instance: &super::ViewInstanceId) {
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
                    && contribution
                        .visibility
                        .is_visible(workspace_open, capabilities)
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
    #[test]
    fn hidden_contributions_remain_available_for_layout() {
        use super::*;
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
    use super::*;
    use crate::extension_ui::{IconId, MessageId};

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

    #[gpui::test]
    fn multiple_views_own_independent_entities_and_reuse_the_same_instance(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::AppContext;
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
        // 宣言だけではMultipleを有効化しない。
        assert!(!registry.can_create_view(&view, super::super::ToolInstancePolicy::Multiple));
        let entities = Rc::new(std::cell::RefCell::new(Vec::new()));
        let created = entities.clone();
        registry.register_view_factory(
            view.clone(),
            Rc::new(move |cx| {
                let entity = cx.new(|_| InstanceCounter(0));
                created.borrow_mut().push(entity.clone());
                entity.into()
            }),
        );
        let first = super::super::ViewInstanceId::new("one");
        let second = super::super::ViewInstanceId::new("two");
        cx.update(|cx| {
            assert!(registry.ensure_view_instance(&view, &first, cx));
            assert!(registry.ensure_view_instance(&view, &second, cx));
            assert!(registry.ensure_view_instance(&view, &first, cx));
            assert_eq!(entities.borrow().len(), 2);
            let first_entity = entities.borrow()[0].clone();
            let second_entity = entities.borrow()[1].clone();
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
            let instance = super::super::ViewInstanceId::shared_tool(&view);
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
