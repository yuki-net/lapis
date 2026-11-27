use super::*;
use crate::features::id;
use crate::workbench::{ViewInstanceKey, ViewScope, WorkbenchMode, WorkbenchState};
use desktop_client::ConversationViewState;
use editor_core::{ConversationId, ExecutionId};
use extension_ui_contract::ZonePosition;

#[test]
fn extension_view_default_zone_is_preserved() {
    use extension_ui_contract::ContributedZone;
    assert_eq!(contributed_zone(ContributedZone::Main), ZonePosition::Main);
    assert_eq!(contributed_zone(ContributedZone::Left), ZonePosition::Left);
    assert_eq!(
        contributed_zone(ContributedZone::Bottom),
        ZonePosition::Bottom
    );
    assert_eq!(
        contributed_zone(ContributedZone::Right),
        ZonePosition::Right
    );
}

#[test]
fn view_visibility_waits_for_a_definitive_host_state() {
    struct Memory;
    impl settings::SettingsRepository for Memory {
        fn load(&self) -> Result<settings::SettingsDocument, settings::SettingsError> {
            Ok(settings::SettingsDocument::empty(
                settings::SettingsScope::User,
            ))
        }
        fn save(&self, _: &settings::SettingsDocument) -> Result<(), settings::SettingsError> {
            Ok(())
        }
    }
    let settings = desktop_client::SettingsSession::load(std::sync::Arc::new(Memory)).unwrap();
    let session = settings.extension_contributions();
    let response = |active: Option<bool>| {
        serde_json::from_value(serde_json::json!({
            "extensions":[{"id":"lapis.terminal","version":"1.0.0","installed":true,"enabled":false,"active":active}],
            "contributions":[{"extension_id":"lapis.terminal",
                "declaration":{"schema":1,"translations":[{"target":"lapis.terminal","locale":"en-US","messages":{"terminal.title":"Terminal"}}]},
                "views":{"schema":1,"views":[{"id":"lapis.terminal.view","content_type":"example.grid.v1","title":"terminal.title","default_zone":"bottom"}]}}]
        })).unwrap()
    };
    let generation = session.begin_reload();
    let hidden = |snapshot: Option<&desktop_client::ExtensionContributionSnapshot>| {
        extension_view_hidden(snapshot, "lapis.terminal", "example.grid.v1")
    };
    assert_eq!(hidden(None), None);
    session.apply(generation, response(None)).unwrap();
    assert_eq!(hidden(session.snapshot().as_ref()), None);
    session.apply(generation, response(Some(false))).unwrap();
    assert_eq!(hidden(session.snapshot().as_ref()), Some(true));
    session.apply(generation, response(Some(true))).unwrap();
    assert_eq!(hidden(session.snapshot().as_ref()), Some(false));
    session
        .apply(
            generation,
            serde_json::from_value(serde_json::json!({
                "extensions": [], "contributions": []
            }))
            .unwrap(),
        )
        .unwrap();
    let snapshot = session.snapshot();
    assert_eq!(hidden(snapshot.as_ref()), Some(true));
    assert!(!extension_view_available(
        snapshot.as_ref(),
        "lapis.terminal",
        "example.grid.v1"
    ));
}

#[test]
fn workbench_publication_ignores_client_private_workspace_details() {
    let private = ConversationViewState {
        workspace: Some(desktop_client::SharedWorkspaceView {
            workspace_id: editor_core::WorkspaceId::new("workspace-1"),
            open_documents: Vec::new(),
            active_path: None,
            active_document_id: None,
        }),
        ..Default::default()
    };
    let changed_layout = ConversationViewState {
        show_panel_controls: false,
        ..private.clone()
    };

    assert_eq!(
        workbench_publish_view(&private),
        workbench_publish_view(&ConversationViewState::default())
    );
    assert_ne!(
        workbench_publish_view(&private),
        workbench_publish_view(&changed_layout)
    );
}

#[test]
fn workbench_publication_excludes_client_active_tabs() {
    let tab = desktop_client::TabRef::Document {
        id: editor_core::DocumentId::new("doc"),
    };
    let mut view = ConversationViewState {
        layout: Some(desktop_client::ZoneLayoutViewState {
            version: desktop_client::ZONE_LAYOUT_VERSION,
            zones: vec![desktop_client::ZoneViewState {
                position: "main".into(),
                open: true,
                extent_ratio: 1.0,
                layout: None,
                panels: vec![desktop_client::ZonePanelViewState {
                    id: "panel".into(),
                    tabs: vec![tab.clone()],
                    pinned_tabs: Vec::new(),
                    active_tab: Some(tab),
                    open: true,
                }],
            }],
            bottom_span_left: false,
            bottom_span_right: false,
        }),
        ..Default::default()
    };
    view.workbench.ide_layout = Some(desktop_client::WorkbenchLayoutState {
        layout: view.layout.clone(),
        show_panel_controls: true,
    });
    view.workbench.task_layouts.insert(
        ConversationId::new("task-a"),
        desktop_client::WorkbenchLayoutState {
            layout: view.layout.clone(),
            show_panel_controls: true,
        },
    );

    let published = workbench_publish_view(&view);
    assert_eq!(
        published.layout.unwrap().zones[0].panels[0].active_tab,
        None
    );
    assert_eq!(
        published
            .workbench
            .ide_layout
            .unwrap()
            .layout
            .unwrap()
            .zones[0]
            .panels[0]
            .active_tab,
        None
    );
    assert_eq!(
        published.workbench.task_layouts[&ConversationId::new("task-a")]
            .layout
            .as_ref()
            .unwrap()
            .zones[0]
            .panels[0]
            .active_tab,
        None
    );
}

#[test]
fn unresolved_task_context_preserves_execution_until_the_view_is_confirmed() {
    let task = ConversationId::new("task-b");
    let saved = ExecutionId::new("saved-selection");
    let latest = ExecutionId::new("restored-selection");
    for resolution in [
        TaskContextResolution::Pending,
        TaskContextResolution::Unavailable,
    ] {
        // 解決待ちのNoneと、解決失敗後のNoneは選択解除として扱わない。
        assert_eq!(
            execution_for_context(
                resolution.confirmed_task(Some(&task)),
                Some(&task),
                None,
                Some(saved.clone())
            ),
            Some(saved.clone())
        );
    }
    let confirmed = TaskContextResolution::Ready.confirmed_task(Some(&task));
    assert_eq!(
        execution_for_context(
            confirmed,
            Some(&task),
            Some(latest.clone()),
            Some(saved.clone())
        ),
        Some(latest)
    );
    assert_eq!(
        execution_for_context(confirmed, Some(&task), None, Some(saved)),
        None
    );
}

#[test]
fn missing_terminal_resource_owner_never_falls_back_to_current_task_record() {
    use desktop_client::{ConversationRecord, RestoredTerminal};
    let a = ConversationId::new("task-a");
    let b = ConversationId::new("task-b");
    let record = |task: ConversationId, resource: &str| ConversationRecord {
        title: task.as_str().into(),
        id: task,
        workspace: desktop_client::WorkspaceViewState::default(),
        view: Default::default(),
        selected_execution: None,
        terminals: vec![RestoredTerminal {
            id: Some(terminal::TerminalId::new(resource)),
            cwd: "workspace".into(),
            status: terminal::TerminalStatus::Running,
            columns: 80,
            rows: 24,
        }],
        environment: None,
        source_role: Default::default(),
        other_workspace: None,
        other_terminals: Vec::new(),
        other_execution: None,
        parked_sources: Vec::new(),
    };
    let mut records = vec![
        record(a.clone(), "terminal-a"),
        record(b.clone(), "terminal-b"),
    ];
    let fixed = ViewScope::Resource("terminal-a".into());
    assert_eq!(
        terminal_context_target(Some(&fixed), &b, &b, &records).map(|target| target.task),
        Some(a.clone())
    );
    records[0].terminals.clear();
    records[0].workspace.root = Some("changed-source".into());
    assert!(terminal_context_target(Some(&fixed), &b, &a, &records).is_none());
    assert_eq!(
        terminal_context_target(Some(&ViewScope::Task(a.clone())), &b, &b, &records)
            .map(|target| target.task),
        Some(a.clone())
    );
    records.remove(0);
    assert!(terminal_context_target(Some(&fixed), &b, &a, &records).is_none());
    let target =
        terminal_context_target(Some(&ViewScope::Task(a.clone())), &b, &b, &records).unwrap();
    assert_eq!(target.task, a);
    assert!(target.record.is_none());
    assert!(terminal_context_target(Some(&ViewScope::Task(a.clone())), &b, &a, &records).is_none());
    assert!(terminal_context_target(Some(&ViewScope::SharedWorkbench), &b, &a, &records).is_none());
    assert_eq!(
        terminal_context_target(Some(&ViewScope::FollowSelectedTask), &b, &a, &records)
            .map(|target| target.task),
        Some(b)
    );
}

#[test]
fn resource_tabs_track_their_own_resource_without_rebinding_others() {
    // タブは自身の Resource を持ち、起動で個体IDが変わらないことを固定する。
    let tab = crate::shell::PanelTab::external(id::VIEW_TERMINAL, "terminal:one", None);
    let bound = tab.with_resource(Some("terminal-a".to_owned()));
    assert_eq!(tab.instance_id(), bound.instance_id());
    assert!(tab.resource().is_none());
    assert_eq!(bound.resource(), Some("terminal-a"));
    // 別のセッションを参照するタブは、独立したタブのまま共存する。
    let other = crate::shell::PanelTab::external(
        id::VIEW_TERMINAL,
        "terminal:two",
        Some("terminal-b".to_owned()),
    );
    assert_ne!(bound.instance_id(), other.instance_id());
}

#[test]
fn fixed_assistant_selection_does_not_overwrite_another_tasks_saved_execution() {
    let a = ConversationId::new("task-a");
    let b = ConversationId::new("task-b");
    let execution_a = ExecutionId::new("execution-a");
    let execution_b = ExecutionId::new("execution-b");
    let key =
        ViewInstanceKey::new(crate::shell::PanelTab::tool(id::VIEW_ASSISTANT).stable_view_key())
            .unwrap();
    let mut state = WorkbenchState::default();
    let view = ConversationViewState::default();
    state.apply_task_snapshot(a.clone(), view.clone(), true);
    state.bind_view(key.clone(), ViewScope::Task(a.clone()));
    state.select_task(b.clone(), view.clone());
    state.set_mode(WorkbenchMode::Agent, Some(&a), &view);
    state.apply_task_snapshot(b.clone(), view.clone(), true);
    assert_eq!(state.selected_task(), state.context_task());
    assert_eq!(state.task_reference(&key), Some(&a));
    for selection in [Some(execution_a.clone()), None] {
        assert_eq!(
            execution_for_context(
                state.task_reference(&key),
                state.context_task(),
                selection,
                Some(execution_b.clone())
            ),
            Some(execution_b.clone())
        );
    }
    // ViewをBへ追従させた場合は、その選択と明示的な選択解除を保存できる。
    state.bind_view(key.clone(), ViewScope::FollowSelectedTask);
    assert_eq!(
        execution_for_context(
            state.task_reference(&key),
            state.context_task(),
            Some(execution_b.clone()),
            Some(execution_a.clone())
        ),
        Some(execution_b.clone())
    );
    assert_eq!(
        execution_for_context(
            state.task_reference(&key),
            state.context_task(),
            None,
            Some(execution_b.clone())
        ),
        None
    );

    // IDEで選択TaskがBのままでも、固定Viewと作業環境がAならAへ保存する。
    state.set_mode(WorkbenchMode::Ide, Some(&b), &view);
    state.apply_task_snapshot(a.clone(), view, true);
    state.bind_view(key.clone(), ViewScope::Task(a.clone()));
    assert_ne!(state.selected_task(), state.context_task());
    assert_eq!(
        execution_for_context(
            state.task_reference(&key),
            state.context_task(),
            Some(execution_a.clone()),
            Some(execution_b.clone())
        ),
        Some(execution_a)
    );
    assert_eq!(
        execution_for_context(None, state.context_task(), None, Some(execution_b.clone())),
        Some(execution_b)
    );
}
