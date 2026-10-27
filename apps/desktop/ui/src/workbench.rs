use lapis_desktop_client::{
    ConversationViewState, ViewBindingState, ViewScopeState, WorkbenchLayoutState,
    WorkbenchModeState, WorkbenchViewState,
};
use lapis_editor_core::ConversationId;
use std::collections::BTreeMap;

/// Task選択が表示Layoutへ与えるポリシーです。Resourceの所有権は変更しません。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum WorkbenchMode {
    /// Taskを切り替えても、現在のEditor・Terminal・TabGroupを維持します。
    #[default]
    Ide,
    /// Taskごとに保存した表示Layoutを復元します。
    Agent,
}

/// 表示上のView所属です。Resourceの所有者や実行中Sessionを表しません。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ViewScope {
    SharedWorkbench,
    Task(ConversationId),
    FollowSelectedTask,
    Resource(String),
}

/// ViewInstanceに対する表示所属の変更を識別するキーです。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ViewInstanceKey(String);

impl ViewInstanceKey {
    pub(crate) fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        (!value.is_empty()).then_some(Self(value))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// Shellの配置とTaskの選択を接続するUI専用状態です。
///
/// `ConversationViewState`は既存の永続化形式として残し、ここでは同じ値を
/// Taskごとの復元候補として扱います。IDEモードでは候補を読み出すだけで
/// 現在のLayoutへ適用せず、Task選択でTabを複製・破棄しません。
#[derive(Clone, Debug, Default)]
pub(crate) struct WorkbenchState {
    mode: WorkbenchMode,
    selected_task: Option<ConversationId>,
    context_task: Option<ConversationId>,
    task_views: BTreeMap<ConversationId, ConversationViewState>,
    ide_view: Option<ConversationViewState>,
    ide_context: Option<ConversationId>,
    bindings: BTreeMap<ViewInstanceKey, ViewScope>,
}

impl WorkbenchState {
    pub(crate) fn mode(&self) -> WorkbenchMode {
        self.mode
    }

    pub(crate) fn selected_task(&self) -> Option<&ConversationId> {
        self.selected_task.as_ref()
    }

    pub(crate) fn context_task(&self) -> Option<&ConversationId> {
        self.context_task.as_ref()
    }

    /// IDEのTask選択は作業環境を変えず、明示された表示参照先だけを更新する。
    pub(crate) fn select_task(&mut self, task: ConversationId, view: ConversationViewState) {
        self.task_views
            .entry(task.clone())
            .or_insert_with(|| layout_only(view));
        self.selected_task = Some(task);
    }

    pub(crate) fn restore_persisted(&mut self, view: &WorkbenchViewState) {
        self.mode = match view.mode {
            WorkbenchModeState::Ide => WorkbenchMode::Ide,
            WorkbenchModeState::Agent => WorkbenchMode::Agent,
        };
        self.selected_task = view.selected_task.clone();
        self.ide_view = view.ide_layout.as_ref().map(restore_layout);
        self.ide_context = view.ide_context.clone();
        self.task_views = view
            .task_layouts
            .iter()
            .map(|(task, view)| (task.clone(), restore_layout(view)))
            .collect();
        self.bindings = view
            .bindings
            .iter()
            .filter_map(|binding| {
                Some((
                    ViewInstanceKey::new(binding.view.clone())?,
                    view_scope_from_state(&binding.scope),
                ))
            })
            .collect();
    }

    pub(crate) fn persisted_view(&self, view: &mut ConversationViewState) {
        let mut task_layouts = self
            .task_views
            .iter()
            .map(|(task, view)| (task.clone(), capture_layout(view)))
            .collect::<BTreeMap<_, _>>();
        let mut ide_layout = self.ide_view.as_ref().map(capture_layout);
        let mut ide_context = self.ide_context.clone();
        match self.mode {
            WorkbenchMode::Ide => {
                ide_layout = Some(capture_layout(view));
                ide_context = self.context_task.clone();
            }
            WorkbenchMode::Agent => {
                if let Some(task) = &self.context_task {
                    task_layouts.insert(task.clone(), capture_layout(view));
                }
            }
        }
        view.workbench = WorkbenchViewState {
            ide_layout,
            ide_context,
            task_layouts,
            selected_task: self.selected_task.clone(),
            mode: match self.mode {
                WorkbenchMode::Ide => WorkbenchModeState::Ide,
                WorkbenchMode::Agent => WorkbenchModeState::Agent,
            },
            bindings: self
                .bindings
                .iter()
                .map(|(view, scope)| ViewBindingState {
                    view: view.0.clone(),
                    scope: view_scope_state(scope),
                })
                .collect(),
        };
    }

    /// Agentモードで現在のTaskの表示状態を退避します。
    pub(crate) fn capture_current(
        &mut self,
        task: Option<&ConversationId>,
        view: &ConversationViewState,
    ) {
        match self.mode {
            WorkbenchMode::Agent => {
                if let Some(task) = task {
                    self.task_views
                        .insert(task.clone(), layout_only(view.clone()));
                }
            }
            WorkbenchMode::Ide => {
                self.ide_view = Some(layout_only(view.clone()));
                self.ide_context = task.cloned();
            }
        }
    }

    /// Task切替直前の表示状態を保存します。IDEモードでは共有Workbenchを変更しません。
    pub(crate) fn prepare_task_switch(
        &mut self,
        current_task: Option<&ConversationId>,
        current_view: &ConversationViewState,
    ) {
        self.capture_current(current_task, current_view);
    }

    /// 初回は保存Layoutを復元し、以後のTask切替ではAgentモード時だけ返します。
    pub(crate) fn apply_task_snapshot(
        &mut self,
        task: ConversationId,
        snapshot_view: ConversationViewState,
        switched: bool,
    ) -> Option<ConversationViewState> {
        let initial = self.context_task.is_none();
        if initial {
            self.restore_persisted(&snapshot_view.workbench);
        }
        if self.selected_task.is_none()
            || (!initial && switched && self.mode == WorkbenchMode::Agent)
        {
            self.selected_task = Some(task.clone());
        }
        self.context_task = Some(task.clone());
        self.task_views
            .entry(task.clone())
            .or_insert_with(|| layout_only(snapshot_view.clone()));
        if initial && self.mode == WorkbenchMode::Ide {
            return Some(self.ide_view.clone().unwrap_or(snapshot_view));
        }
        (initial || (self.mode == WorkbenchMode::Agent && switched))
            .then(|| self.task_views.get(&task).cloned())
            .flatten()
    }

    /// Mode変更時に必要なら現在Taskの保存Layoutを返します。
    pub(crate) fn set_mode(
        &mut self,
        mode: WorkbenchMode,
        current_task: Option<&ConversationId>,
        current_view: &ConversationViewState,
    ) -> Option<ConversationViewState> {
        if self.mode == mode {
            return None;
        }
        self.capture_current(current_task, current_view);
        self.mode = mode;
        match mode {
            WorkbenchMode::Ide => self.ide_view.clone().or_else(|| Some(current_view.clone())),
            WorkbenchMode::Agent => self
                .selected_task
                .as_ref()
                .and_then(|task| self.task_views.get(task).cloned())
                .or_else(|| Some(current_view.clone())),
        }
    }

    pub(crate) fn mode_context(&self) -> Option<&ConversationId> {
        match self.mode {
            WorkbenchMode::Ide => self.ide_context.as_ref().or(self.context_task.as_ref()),
            WorkbenchMode::Agent => self.selected_task.as_ref(),
        }
    }

    /// Worker待ちの間に受理した表示変更は、古い候補Snapshotで上書きしない。
    pub(crate) fn commit_mode_transition(
        &mut self,
        mut candidate: Self,
        current_view: &ConversationViewState,
    ) {
        let task = self.context_task.clone();
        self.capture_current(task.as_ref(), current_view);
        candidate.bindings = self.bindings.clone();
        candidate.ide_view = self.ide_view.clone();
        candidate.ide_context = self.ide_context.clone();
        candidate.task_views = self.task_views.clone();
        *self = candidate;
    }

    /// Viewの表示上の所属だけを変更します。HostのResourceには触れません。
    pub(crate) fn bind_view(&mut self, view: ViewInstanceKey, scope: ViewScope) {
        self.bindings.insert(view, scope);
    }

    pub(crate) fn ensure_view_binding(&mut self, view: ViewInstanceKey, scope: ViewScope) {
        self.bindings.entry(view).or_insert(scope);
    }

    pub(crate) fn view_scope(&self, view: &ViewInstanceKey) -> Option<&ViewScope> {
        self.bindings.get(view)
    }

    pub(crate) fn hidden_views(
        &self,
    ) -> std::collections::HashSet<crate::extension_ui::ViewInstanceId> {
        self.bindings
            .iter()
            .filter_map(|(view, scope)| match scope {
                ViewScope::Task(task)
                    if self.mode == WorkbenchMode::Agent
                        && self.selected_task.as_ref() != Some(task) =>
                {
                    Some(crate::extension_ui::ViewInstanceId::new(view.0.clone()))
                }
                _ => None,
            })
            .collect()
    }

    pub(crate) fn retained_views(
        &self,
    ) -> std::collections::HashSet<crate::extension_ui::ViewInstanceId> {
        self.bindings
            .iter()
            .filter(|(_, scope)| !matches!(scope, ViewScope::Task(_)))
            .map(|(view, _)| crate::extension_ui::ViewInstanceId::new(view.0.clone()))
            .collect()
    }

    pub(crate) fn task_reference(&self, view: &ViewInstanceKey) -> Option<&ConversationId> {
        match self.bindings.get(view) {
            Some(ViewScope::Task(task)) => Some(task),
            _ => self.selected_task(),
        }
    }

    #[cfg(test)]
    fn task_view(&self, task: &ConversationId) -> Option<&ConversationViewState> {
        self.task_views.get(task)
    }
}

fn capture_layout(view: &ConversationViewState) -> WorkbenchLayoutState {
    WorkbenchLayoutState {
        layout: view.layout.clone(),
        show_panel_controls: view.show_panel_controls,
    }
}

fn layout_only(mut view: ConversationViewState) -> ConversationViewState {
    view.workbench = Default::default();
    view
}

fn restore_layout(view: &WorkbenchLayoutState) -> ConversationViewState {
    ConversationViewState {
        layout: view.layout.clone(),
        show_panel_controls: view.show_panel_controls,
        ..Default::default()
    }
}

fn view_scope_state(scope: &ViewScope) -> ViewScopeState {
    match scope {
        ViewScope::SharedWorkbench => ViewScopeState::SharedWorkbench,
        ViewScope::Task(task) => ViewScopeState::Task(task.clone()),
        ViewScope::FollowSelectedTask => ViewScopeState::FollowSelectedTask,
        ViewScope::Resource(resource) => ViewScopeState::Resource(resource.clone()),
    }
}

fn view_scope_from_state(scope: &ViewScopeState) -> ViewScope {
    match scope {
        ViewScopeState::SharedWorkbench => ViewScope::SharedWorkbench,
        ViewScopeState::Task(task) => ViewScope::Task(task.clone()),
        ViewScopeState::FollowSelectedTask => ViewScope::FollowSelectedTask,
        ViewScopeState::Resource(resource) => ViewScope::Resource(resource.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_split_layouts_keep_group_and_view_identity_across_tasks_modes_and_restart() {
        use crate::features::conversation::{apply_view_state, capture_view_state};
        use crate::{
            extension_ui::PanelPosition,
            shell::{PanelDrop, PanelEdge, PanelTab, ShellState},
        };
        use lapis_desktop_client::DocumentTab;

        fn fixture(name: &str, direction: PanelEdge) -> (ConversationViewState, DocumentTab) {
            let document = DocumentTab {
                id: lapis_editor_core::DocumentId::new(name),
                display_name: name.into(),
                path: None,
                dirty: true,
                external_change: lapis_document::ExternalChange::Unchanged,
                encoding: Default::default(),
                active: true,
            };
            let mut shell = ShellState::default();
            let main = shell
                .layout
                .open_tab(PanelPosition::Main, PanelTab::Document(document.id.clone()))
                .unwrap();
            let terminal = PanelTab::tool(crate::features::id::VIEW_TERMINAL);
            let source = shell
                .layout
                .open_tab(PanelPosition::Main, terminal.clone())
                .unwrap();
            let extra = shell
                .layout
                .drop_tab(
                    &source,
                    &terminal,
                    &PanelDrop::Edge {
                        panel: main,
                        edge: direction,
                    },
                    (800.0, 600.0),
                    std::time::Instant::now(),
                )
                .unwrap();
            let preview = PanelTab::tool(crate::features::id::VIEW_PREVIEW);
            let source = shell
                .layout
                .open_tab(PanelPosition::Main, preview.clone())
                .unwrap();
            shell
                .layout
                .drop_tab(
                    &source,
                    &preview,
                    &PanelDrop::Edge {
                        panel: extra,
                        edge: if direction == PanelEdge::Right {
                            PanelEdge::Bottom
                        } else {
                            PanelEdge::Right
                        },
                    },
                    (800.0, 600.0),
                    std::time::Instant::now(),
                )
                .unwrap();
            shell
                .layout
                .set_zone_open(PanelPosition::Left, direction == PanelEdge::Right);
            (capture_view_state(&shell), document)
        }

        let a = ConversationId::new("split-a");
        let b = ConversationId::new("split-b");
        let (ide, document_a) = fixture("document-a", PanelEdge::Right);
        let (agent_b, document_b) = fixture("document-b", PanelEdge::Bottom);
        let mut state = WorkbenchState::default();
        state.apply_task_snapshot(a.clone(), ide.clone(), true);
        let terminal = ViewInstanceKey::new(
            PanelTab::tool(crate::features::id::VIEW_TERMINAL).stable_view_key(),
        )
        .unwrap();
        state.bind_view(terminal.clone(), ViewScope::SharedWorkbench);
        for selected in [&b, &a, &b] {
            state.select_task(
                selected.clone(),
                if selected == &a {
                    ide.clone()
                } else {
                    agent_b.clone()
                },
            );
            state.prepare_task_switch(Some(&a), &ide);
            assert!(
                state
                    .apply_task_snapshot(a.clone(), ide.clone(), false)
                    .is_none()
            );
            assert_eq!(state.context_task(), Some(&a));
        }
        assert_eq!(
            state.set_mode(WorkbenchMode::Agent, Some(&a), &ide),
            Some(agent_b.clone())
        );
        assert_eq!(
            state.apply_task_snapshot(b.clone(), agent_b.clone(), true),
            Some(agent_b.clone())
        );
        state.prepare_task_switch(Some(&b), &agent_b);
        assert_eq!(
            state.apply_task_snapshot(a.clone(), ide.clone(), true),
            Some(ide.clone())
        );
        let mut saved = ide.clone();
        state.persisted_view(&mut saved);
        let decoded: ConversationViewState =
            serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        let mut restored = WorkbenchState::default();
        assert_eq!(
            restored
                .apply_task_snapshot(a.clone(), decoded, true)
                .unwrap()
                .layout,
            ide.layout
        );
        assert_eq!(
            restored.view_scope(&terminal),
            Some(&ViewScope::SharedWorkbench)
        );

        let registry = crate::features::bundled_registry();
        let mut shell = ShellState::default();
        for (task, expected, document) in [(&b, &agent_b, &document_b), (&a, &ide, &document_a)] {
            let view = restored
                .apply_task_snapshot(task.clone(), expected.clone(), true)
                .unwrap();
            for _ in 0..2 {
                apply_view_state(
                    view.clone(),
                    &mut shell,
                    std::slice::from_ref(document),
                    &registry,
                );
                assert_eq!(shell.layout.capture(), *expected.layout.as_ref().unwrap());
                let placements = shell.layout.placements();
                let unique = placements
                    .iter()
                    .map(|(tab, _)| tab.instance_id())
                    .collect::<std::collections::HashSet<_>>();
                assert_eq!(unique.len(), placements.len());
                assert_eq!(
                    placements
                        .iter()
                        .filter(|(tab, _)| *tab == PanelTab::Document(document.id.clone()))
                        .count(),
                    1
                );
                assert_eq!(
                    placements
                        .iter()
                        .filter(
                            |(tab, _)| *tab == PanelTab::tool(crate::features::id::VIEW_TERMINAL)
                        )
                        .count(),
                    1
                );
                assert_eq!(shell.layout.leaf_group_ids().len(), 4);
            }
        }
        assert_eq!(
            capture_layout(
                &restored
                    .set_mode(WorkbenchMode::Ide, Some(&a), &ide)
                    .unwrap()
            ),
            capture_layout(&ide)
        );
        assert_eq!(
            capture_layout(restored.task_view(&b).unwrap()),
            capture_layout(&agent_b)
        );
    }

    fn view(tool: &str) -> ConversationViewState {
        let mut layout = crate::shell::DockLayout::default();
        layout
            .open_tab(
                crate::extension_ui::PanelPosition::Main,
                crate::shell::PanelTab::tool_instance("fixture", format!("instance-{tool}")),
            )
            .unwrap();
        ConversationViewState {
            layout: Some(layout.capture()),
            ..Default::default()
        }
    }

    #[test]
    fn agent_mode_restores_each_task_without_creating_a_session() {
        let first = ConversationId::new("task-a");
        let second = ConversationId::new("task-b");
        let mut state = WorkbenchState::default();

        assert_eq!(state.mode(), WorkbenchMode::Ide);
        assert!(
            state
                .apply_task_snapshot(first.clone(), view("files"), true)
                .is_some()
        );
        let first_view = view("terminal-a");
        assert_eq!(
            state.set_mode(WorkbenchMode::Agent, Some(&first), &first_view),
            Some(view("files"))
        );
        state.prepare_task_switch(Some(&first), &first_view);
        assert_eq!(
            state.apply_task_snapshot(second.clone(), view("git"), true),
            Some(view("git"))
        );
        let second_view = view("terminal-b");
        state.prepare_task_switch(Some(&second), &second_view);
        assert_eq!(
            state.apply_task_snapshot(first.clone(), view("stale"), true),
            Some(first_view.clone())
        );
        assert_eq!(state.task_view(&first), Some(&first_view));
        assert_eq!(state.task_view(&second), Some(&second_view));
    }

    #[test]
    fn ide_mode_keeps_the_shared_layout_when_task_changes() {
        let first = ConversationId::new("task-a");
        let second = ConversationId::new("task-b");
        let mut state = WorkbenchState::default();
        state.apply_task_snapshot(first.clone(), view("files"), true);
        state.select_task(second.clone(), view("git"));

        state.prepare_task_switch(Some(&ConversationId::new("task-a")), &view("terminal"));
        assert!(
            state
                .apply_task_snapshot(first.clone(), view("files"), false)
                .is_none()
        );
        assert_eq!(state.mode(), WorkbenchMode::Ide);
        assert_eq!(state.context_task(), Some(&first));
        assert_eq!(
            state.selected_task().map(ConversationId::as_str),
            Some("task-b")
        );
    }

    #[test]
    fn view_binding_moves_display_scope_without_changing_resource_identity() {
        let mut state = WorkbenchState::default();
        let key = ViewInstanceKey::new("terminal:terminal-1").unwrap();
        let scope = ViewScope::Task(ConversationId::new("task-a"));
        state.bind_view(key.clone(), scope.clone());
        assert_eq!(state.view_scope(&key), Some(&scope));
        assert_eq!(key.as_str(), "terminal:terminal-1");
    }

    #[test]
    fn persisted_workbench_state_round_trips_mode_and_bindings() {
        let mut state = WorkbenchState::default();
        let key = ViewInstanceKey::new("terminal:terminal-1").unwrap();
        state.bind_view(key, ViewScope::Task(ConversationId::new("task-a")));
        let mut view = view("files");
        state.set_mode(WorkbenchMode::Agent, None, &view);
        state.persisted_view(&mut view);

        let mut restored = WorkbenchState::default();
        restored.restore_persisted(&view.workbench);
        assert_eq!(restored.mode(), WorkbenchMode::Agent);
        assert_eq!(
            restored.view_scope(&ViewInstanceKey::new("terminal:terminal-1").unwrap()),
            Some(&ViewScope::Task(ConversationId::new("task-a")))
        );
    }

    #[test]
    fn a_task_with_no_agent_sessions_is_still_a_valid_selection() {
        let task = ConversationId::new("task-without-agent");
        let mut state = WorkbenchState::default();
        assert!(
            state
                .apply_task_snapshot(task.clone(), view("files"), true)
                .is_some()
        );
        assert_eq!(state.selected_task(), Some(&task));
        assert_eq!(state.task_view(&task), Some(&view("files")));
    }

    #[test]
    fn ide_selection_survives_resource_snapshots_without_changing_context() {
        let a = ConversationId::new("task-a");
        let b = ConversationId::new("task-b");
        let mut state = WorkbenchState::default();
        state.apply_task_snapshot(a.clone(), view("files"), true);
        for _ in 0..3 {
            state.select_task(b.clone(), view("b"));
            assert!(
                state
                    .apply_task_snapshot(a.clone(), view("stale"), false)
                    .is_none()
            );
            assert_eq!(state.selected_task(), Some(&b));
            assert_eq!(state.context_task(), Some(&a));
            state.select_task(a.clone(), view("a"));
            assert!(
                state
                    .apply_task_snapshot(a.clone(), view("stale"), false)
                    .is_none()
            );
        }
        assert_eq!(state.task_views.len(), 2);
    }

    #[test]
    fn mode_round_trip_keeps_latest_layout_and_bindings() {
        let a = ConversationId::new("task-a");
        let mut state = WorkbenchState::default();
        state.apply_task_snapshot(a.clone(), view("initial"), true);
        let key = ViewInstanceKey::new("document:dirty").unwrap();
        state.bind_view(key.clone(), ViewScope::Task(a.clone()));
        assert_eq!(
            state.set_mode(WorkbenchMode::Agent, Some(&a), &view("ide-edited")),
            Some(view("initial"))
        );
        assert_eq!(
            state.set_mode(WorkbenchMode::Ide, Some(&a), &view("agent-edited")),
            Some(view("ide-edited"))
        );
        assert_eq!(state.task_view(&a), Some(&view("agent-edited")));
        assert_eq!(
            state.set_mode(WorkbenchMode::Agent, Some(&a), &view("latest-ide")),
            Some(view("agent-edited"))
        );
        assert_eq!(
            state.set_mode(WorkbenchMode::Ide, Some(&a), &view("latest-agent")),
            Some(view("latest-ide"))
        );
        assert_eq!(state.view_scope(&key), Some(&ViewScope::Task(a)));
    }

    #[test]
    fn completing_mode_switch_preserves_binding_and_layout_accepted_while_worker_pending() {
        let a = ConversationId::new("task-a");
        let b = ConversationId::new("task-b");
        let mut state = WorkbenchState::default();
        state.apply_task_snapshot(a.clone(), view("initial-a"), true);
        state.select_task(b.clone(), view("initial-b"));
        let mut pending = state.clone();
        assert_eq!(
            pending.set_mode(WorkbenchMode::Agent, Some(&a), &view("ide-before")),
            Some(view("initial-b"))
        );
        let key = ViewInstanceKey::new("terminal:independent").unwrap();
        state.bind_view(key.clone(), ViewScope::Task(b.clone()));
        state.commit_mode_transition(pending, &view("ide-during-worker"));
        assert_eq!(state.mode(), WorkbenchMode::Agent);
        assert_eq!(state.view_scope(&key), Some(&ViewScope::Task(b.clone())));
        assert_eq!(state.task_view(&b), Some(&view("initial-b")));
        state.apply_task_snapshot(b.clone(), view("stale-b"), true);
        assert_eq!(
            state.set_mode(WorkbenchMode::Ide, Some(&b), &view("agent-edited")),
            Some(view("ide-during-worker"))
        );
        assert_eq!(state.view_scope(&key), Some(&ViewScope::Task(b)));
    }

    #[test]
    fn persisted_layout_signature_changes_for_mode_binding_and_selection() {
        let a = ConversationId::new("task-a");
        let mut state = WorkbenchState::default();
        state.apply_task_snapshot(a.clone(), view("initial"), true);
        let mut before = view("same-layout");
        state.persisted_view(&mut before);
        state.set_mode(WorkbenchMode::Agent, Some(&a), &before);
        let mut after = view("same-layout");
        state.persisted_view(&mut after);
        assert_ne!(before, after);
        before = after.clone();
        state.bind_view(
            ViewInstanceKey::new("terminal:1").unwrap(),
            ViewScope::SharedWorkbench,
        );
        state.persisted_view(&mut after);
        assert_ne!(before, after);
        before = after.clone();
        state.select_task(ConversationId::new("task-b"), view("b"));
        state.persisted_view(&mut after);
        assert_ne!(before, after);
        let mut restored = WorkbenchState::default();
        restored.apply_task_snapshot(a.clone(), after, true);
        assert_eq!(
            restored.selected_task(),
            Some(&ConversationId::new("task-b"))
        );
        assert_eq!(restored.context_task(), Some(&a));
    }

    #[test]
    fn task_scope_filters_presentation_while_shared_follow_and_resource_keep_identity() {
        use crate::extension_ui::PanelPosition;
        use crate::shell::{PanelTab, ShellState};
        let a = ConversationId::new("task-a");
        let b = ConversationId::new("task-b");
        let mut state = WorkbenchState::default();
        state.apply_task_snapshot(a.clone(), view("a"), true);
        let mut shell = ShellState::default();
        let owned = PanelTab::tool_instance("tool", "owned");
        let shared = PanelTab::tool_instance("tool", "shared");
        let follow = PanelTab::tool_instance("tool", "follow");
        let fixed = PanelTab::Document(lapis_editor_core::DocumentId::new("resource-a"));
        for tab in [&owned, &shared, &follow, &fixed] {
            shell
                .layout
                .open_tab(PanelPosition::Main, tab.clone())
                .unwrap();
        }
        state.bind_view(
            ViewInstanceKey::new(owned.stable_view_key()).unwrap(),
            ViewScope::Task(a.clone()),
        );
        state.bind_view(
            ViewInstanceKey::new(shared.stable_view_key()).unwrap(),
            ViewScope::SharedWorkbench,
        );
        state.bind_view(
            ViewInstanceKey::new(follow.stable_view_key()).unwrap(),
            ViewScope::FollowSelectedTask,
        );
        state.bind_view(
            ViewInstanceKey::new(fixed.stable_view_key()).unwrap(),
            ViewScope::Resource("resource-a".into()),
        );
        let before = shell.layout.capture();
        state.set_mode(WorkbenchMode::Agent, Some(&a), &view("ide"));
        state.apply_task_snapshot(b.clone(), view("b"), true);
        shell.hidden_views = state.hidden_views();
        assert!(!shell.view_visible(&owned));
        for tab in [&shared, &follow, &fixed] {
            assert!(shell.view_visible(tab));
        }
        assert_eq!(
            state.task_reference(&ViewInstanceKey::new(follow.stable_view_key()).unwrap()),
            Some(&b)
        );
        assert_eq!(
            state.task_reference(&ViewInstanceKey::new(owned.stable_view_key()).unwrap()),
            Some(&a)
        );
        assert_eq!(shell.layout.capture(), before);
        state.set_mode(WorkbenchMode::Ide, Some(&b), &view("agent-b"));
        shell.hidden_views = state.hidden_views();
        assert!(shell.view_visible(&owned));
        assert_eq!(shell.layout.capture(), before);
    }

    #[test]
    fn restart_preserves_independent_mode_layouts_without_nested_workbench_copies() {
        let a = ConversationId::new("task-a");
        let mut state = WorkbenchState::default();
        state.apply_task_snapshot(a.clone(), view("agent-initial"), true);
        state.set_mode(WorkbenchMode::Agent, Some(&a), &view("ide-initial"));
        state.set_mode(WorkbenchMode::Ide, Some(&a), &view("agent-edited"));
        let mut saved = view("ide-edited");
        state.persisted_view(&mut saved);
        let encoded = serde_json::to_string(&saved).unwrap();
        let decoded: ConversationViewState = serde_json::from_str(&encoded).unwrap();
        let mut restored = WorkbenchState::default();
        assert_eq!(
            restored
                .apply_task_snapshot(a.clone(), decoded, true)
                .unwrap()
                .layout,
            view("ide-edited").layout
        );
        assert_eq!(
            restored
                .set_mode(WorkbenchMode::Agent, Some(&a), &view("ide-edited"))
                .unwrap()
                .layout,
            view("agent-edited").layout
        );
        assert!(
            restored
                .task_views
                .values()
                .all(|view| view.workbench == WorkbenchViewState::default())
        );
    }
}
