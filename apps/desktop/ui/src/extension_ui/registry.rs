use std::collections::{BTreeMap, HashMap, HashSet};

use lapis_language::LanguageId;

use super::{
    ActivationCondition, ActivationEvent, CommandId, FeatureId, PanelPosition, UiContribution,
    UiSlot, ViewId, WorkspaceCapabilityId,
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
    active_language: Option<LanguageId>,
    workspace_open: bool,
    capabilities: HashSet<WorkspaceCapabilityId>,
    active_commands: HashSet<CommandId>,
}

impl FeatureRegistry {
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

    pub fn panel_contributions(&self, position: PanelPosition) -> Vec<&UiContribution> {
        let mut contributions = self
            .descriptors
            .values()
            .flat_map(|descriptor| descriptor.contributions.iter())
            .filter(|contribution| {
                contribution.view.is_some() && contribution.allowed_panels.contains(&position)
            })
            .collect::<Vec<_>>();
        contributions.sort_by_key(|contribution| contribution.order);
        contributions
    }

    pub fn tool_contributions(&self, position: PanelPosition) -> Vec<&UiContribution> {
        self.panel_contributions(position)
            .into_iter()
            .filter(|contribution| contribution.slot != UiSlot::SettingsPage)
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

    pub fn set_panel_active_view(&mut self, panel: PanelPosition, view: Option<ViewId>) {
        let slot = match panel {
            PanelPosition::Left => UiSlot::ToolDock,
            PanelPosition::Main => UiSlot::EditorDecoration,
            PanelPosition::Bottom => UiSlot::BottomDock,
            PanelPosition::Right => UiSlot::SideDock,
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
                        self.active_views.values().any(|active| active == view)
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
    use crate::extension_ui::{IconId, MessageId};

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
}
