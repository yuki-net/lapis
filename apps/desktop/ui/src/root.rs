use crate::{
    app::{DesktopServices, InitialView},
    client_adapter::{ClientSnapshot, ClientWorker, DocumentClient, EditChain, PersistedView},
    commands::UiCommand,
    extension_ui::{PanelPosition, ViewId},
    features::{
        self,
        command_search::{
            CommandSearchProvider, DoubleShiftDetector, QuickSearch, QuickSearchEvent,
        },
        conversation::{ConversationEvent, ConversationFeature},
        editor::{Editor, EditorEvent},
        files::FilesFeature,
        git::{GitEvent, GitFeature, GitInputRequest},
        id,
        preview::PreviewFeature,
        problems::{ProblemsEvent, ProblemsFeature},
        search::SearchFeature,
        settings::SettingsFeature,
        tasks::TasksFeature,
        terminal::TerminalFeature,
    },
    shell::main::{Shell, ShellPresentation},
    workbench::{ViewInstanceKey, ViewScope, WorkbenchMode, WorkbenchState},
};
use gpui::{
    AnyView, AppContext, Context, Entity, Focusable, Render, Subscription, Window, div, prelude::*,
};
use lapis_desktop_client::EditorSession;
use std::collections::HashMap;

enum PathRequest {
    Open(std::path::PathBuf),
    Save(std::path::PathBuf, lapis_editor_core::DocumentId),
    CloseSave(std::sync::mpsc::Sender<Result<String, String>>),
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum TaskContextResolution {
    #[default]
    Unavailable,
    Pending,
    Ready,
}

impl TaskContextResolution {
    fn confirmed_task(
        self,
        task: Option<&lapis_editor_core::ConversationId>,
    ) -> Option<&lapis_editor_core::ConversationId> {
        (self == Self::Ready).then_some(task).flatten()
    }
}

type PersistSignature = (
    Vec<(
        lapis_editor_core::DocumentId,
        lapis_document::Revision,
        bool,
    )>,
    Option<(
        lapis_editor_core::DocumentId,
        std::ops::Range<usize>,
        usize,
        f32,
        f32,
    )>,
    lapis_desktop_client::ConversationViewState,
    Option<lapis_editor_core::ExecutionId>,
    Option<lapis_editor_core::ConversationId>,
    Vec<lapis_desktop_client::RestoredTerminal>,
);

pub(crate) struct DesktopRoot {
    pub(crate) shell: Entity<Shell>,
    pub(crate) group: Entity<crate::window_host::WindowGroup>,
    pub(crate) editor: Entity<Editor>,
    files: Entity<FilesFeature>,
    git: Entity<GitFeature>,
    tasks: Entity<TasksFeature>,
    terminal: Entity<TerminalFeature>,
    problems: Entity<ProblemsFeature>,
    search: Entity<SearchFeature>,
    conversation: Entity<ConversationFeature>,
    preview: Entity<PreviewFeature>,
    preview_revision: Option<(lapis_editor_core::DocumentId, lapis_document::Revision)>,
    line_ending_cache: Option<(
        (lapis_editor_core::DocumentId, lapis_document::Revision),
        &'static str,
    )>,
    settings: Entity<SettingsFeature>,
    quick_search: Entity<QuickSearch>,
    path_request: Option<PathRequest>,
    git_input: Option<GitInputRequest>,
    _subscriptions: Vec<Subscription>,
    double_shift: DoubleShiftDetector,
    pub(crate) worker: ClientWorker<DocumentClient>,
    factory: crate::app::WindowFactory,
    persist_task: Option<gpui::Task<()>>,
    persist_signature: Option<PersistSignature>,
    native_smoke_requested: bool,
    workbench: WorkbenchState,
    task_context_gate: crate::client_adapter::ResponseGate,
    task_context_resolution: TaskContextResolution,
    terminal_context_resolved: bool,
    task_context: Option<(
        lapis_editor_core::ConversationId,
        Option<std::path::PathBuf>,
    )>,
}

impl DesktopRoot {
    /// Creates a root window's features, shared client worker, and window group.
    /// Installs command, snapshot, persistence, and close subscriptions and focuses the editor.
    pub(crate) fn new(
        session: EditorSession,
        mut services: DesktopServices,
        initial: InitialView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let factory = crate::app::WindowFactory::new(&session, &services);
        let conversation_id = services.conversation.active_id().clone();
        let terminal_root = session.workspace_root().map(ToOwned::to_owned);
        let initial_locale = services.settings.settings().locale;
        if services.terminal.terminals().is_empty() {
            let summaries = services
                .conversation
                .active_record()
                .filter(|record| record.workspace.root == terminal_root)
                .map(|record| record.terminals.clone())
                .unwrap_or_default();
            services.terminal.restore_summaries(&summaries);
        }
        let worker = ClientWorker::new(DocumentClient {
            editor: session,
            conversation: services.conversation,
            lsp: services.lsp,
            edit_chain: EditChain::default(),
            git: services.git.backend(),
            snapshot_sequence: 0,
            document_owners: Default::default(),
        });
        let editor = cx.new(|cx| Editor::new(worker.clone(), cx));
        let files = cx.new(|_| FilesFeature::new());
        let git = cx.new(|cx| GitFeature::new(services.git, cx));
        let tasks = cx.new(|cx| TasksFeature::new(services.task, cx));
        let terminal = cx
            .new(|cx| TerminalFeature::new(services.terminal, terminal_root, conversation_id, cx));
        let problems = cx.new(|cx| ProblemsFeature::new(worker.clone(), cx));
        let search = cx.new(|cx| SearchFeature::new(services.search, cx));
        let conversation = cx.new(|_| ConversationFeature::new());
        let preview = cx.new(|_| PreviewFeature::new());
        let settings = cx.new(|_| SettingsFeature::new(services.settings));
        let root = cx.entity().downgrade();
        let quick_search = cx.new(|cx| {
            QuickSearch::new(cx, move |event, window, cx| {
                let _ = root.update(cx, |root, cx| {
                    match event {
                        QuickSearchEvent::Execute(command) => root
                            .shell
                            .update(cx, |shell, cx| shell.execute_command(command, window, cx)),
                        QuickSearchEvent::Dismiss => {
                            if let Some(PathRequest::CloseSave(sender)) = root.path_request.take() {
                                let _ = sender.send(Err("保存をキャンセルしました".into()));
                            }
                            root.git_input = None;
                            root.shell.update(cx, |shell, cx| {
                                shell.close_panel(PanelPosition::Right, cx)
                            });
                        }
                        QuickSearchEvent::SubmitPath(path) => {
                            root.submit_path(path, cx);
                            root.shell.update(cx, |shell, cx| {
                                shell.close_panel(PanelPosition::Right, cx)
                            });
                        }
                        QuickSearchEvent::SubmitText(text) => {
                            if let Some(request) = root.git_input.take() {
                                root.git.update(cx, |git, cx| {
                                    git.complete_input(request, text, window, cx)
                                });
                            }
                            root.shell.update(cx, |shell, cx| {
                                shell.close_panel(PanelPosition::Right, cx)
                            });
                        }
                    }
                    window.focus(&root.editor.focus_handle(cx));
                });
            })
        });
        let views = HashMap::<ViewId, AnyView>::from([
            (id::VIEW_FILES.into(), files.clone().into()),
            (id::VIEW_GIT.into(), git.clone().into()),
            (id::VIEW_ASSISTANT.into(), tasks.clone().into()),
            (id::VIEW_TERMINAL.into(), terminal.clone().into()),
            (id::VIEW_PROBLEMS.into(), problems.clone().into()),
            (id::VIEW_SEARCH.into(), search.clone().into()),
            (id::VIEW_PROJECT_TASKS.into(), conversation.clone().into()),
            (id::VIEW_PREVIEW.into(), preview.clone().into()),
            (id::VIEW_SETTINGS.into(), settings.clone().into()),
            (id::VIEW_COMMAND_SEARCH.into(), quick_search.clone().into()),
        ]);
        let shell = cx.new(|cx| Shell::new(editor.clone().into(), views, cx));
        shell.update(cx, |shell, cx| {
            shell.set_locale(initial_locale.clone(), cx);
            shell.activate_panel_view(PanelPosition::Left, id::VIEW_PROJECT_TASKS.into(), cx);
            if initial.show_tasks {
                shell.activate_panel_view(PanelPosition::Right, id::VIEW_ASSISTANT.into(), cx);
            }
            if initial.show_terminal || initial.show_problems {
                shell.activate_panel_view(
                    PanelPosition::Bottom,
                    if initial.show_problems {
                        id::VIEW_PROBLEMS
                    } else {
                        id::VIEW_TERMINAL
                    }
                    .into(),
                    cx,
                );
            }
        });
        let group = cx.new(|_| {
            crate::window_host::WindowGroup::new(
                window,
                editor.clone(),
                shell.clone(),
                initial_locale,
            )
        });
        shell.update(cx, |shell, _| shell.group = Some(group.entity_id()));
        let mut subscriptions = vec![
            cx.observe_window_activation(window, |root, window, cx| {
                if !window.is_window_active() {
                    root.shell
                        .update(cx, |shell, cx| shell.cancel_tab_drag(window, cx));
                }
            }),
            cx.observe_window_bounds(window, |root, window, cx| {
                root.group.update(cx, |group, _| {
                    if let Some(member) = group.members.get_mut(&0) {
                        member.bounds = window.window_bounds();
                    }
                });
            }),
            cx.observe(&group, |root, _, cx| root.synchronize_group(cx)),
            cx.subscribe(
                &settings,
                |root, _, values: &lapis_settings::GlobalSettings, cx| {
                    root.shell
                        .update(cx, |shell, cx| shell.set_locale(values.locale.clone(), cx));
                    root.group.update(cx, |group, cx| {
                        group.locale = values.locale.clone();
                        cx.notify();
                    });
                    cx.refresh_windows();
                },
            ),
            cx.subscribe_in(&shell, window, |root, _, command, window, cx| {
                root.command(command.clone(), window, cx)
            }),
            cx.subscribe_in(&files, window, |root, _, command, window, cx| {
                root.command(command.clone(), window, cx)
            }),
            cx.subscribe_in(&search, window, |root, _, command, window, cx| {
                root.command(command.clone(), window, cx)
            }),
            cx.subscribe_in(&git, window, |root, _, event, window, cx| match event {
                GitEvent::DiscardWorktree {
                    workspace,
                    worktree,
                } => {
                    let workspace = workspace.clone();
                    let worktree = worktree.clone();
                    let view = root.persisted_view(cx);
                    root.editor.update(cx, |editor, cx| {
                        editor.submit(
                            false,
                            move |client| {
                                if client.editor.workspace_id() != Some(&workspace) {
                                    return Err("Worktreeの操作対象が変わっています".into());
                                }
                                client.persist_view(view)?;
                                client
                                    .conversation
                                    .discard_worktree(
                                        &mut client.editor,
                                        client.git.as_ref(),
                                        &worktree,
                                    )
                                    .map_err(|error| error.to_string())
                            },
                            cx,
                        )
                    });
                }
                GitEvent::OpenWorktree {
                    workspace,
                    worktree,
                } => {
                    let workspace = workspace.clone();
                    let worktree = worktree.clone();
                    let view = root.persisted_view(cx);
                    root.editor.update(cx, |editor, cx| {
                        editor.submit(
                            false,
                            move |client| {
                                if client.editor.workspace_id() != Some(&workspace) {
                                    return Err(
                                        "Taskの操作対象が変わっています。選び直してください".into(),
                                    );
                                }
                                client.persist_view(view)?;
                                client
                                    .conversation
                                    .set_worktree(
                                        &mut client.editor,
                                        client.git.as_ref(),
                                        worktree.as_ref(),
                                    )
                                    .map_err(|error| error.to_string())
                            },
                            cx,
                        )
                    });
                }
                GitEvent::Input(request) => {
                    root.git_input = Some(request.clone());
                    root.quick_search
                        .update(cx, |search, cx| search.open_input(request.title(), "", cx));
                    root.shell.update(cx, |shell, cx| {
                        shell.activate_panel_view(
                            PanelPosition::Right,
                            id::VIEW_COMMAND_SEARCH.into(),
                            cx,
                        )
                    });
                    window.focus(&root.quick_search.read(cx).focus_handle());
                }
            }),
            cx.subscribe_in(&editor, window, |root, _, event, window, cx| match event {
                EditorEvent::Snapshot(snapshot) => {
                    root.group
                        .update(cx, |group, cx| group.publish(*snapshot.clone(), cx));
                    root.synchronize_group(cx);
                    if root.native_smoke_requested {
                        root.native_smoke_requested = false;
                        crate::window_host::run_native_smoke(root, window, cx);
                    }
                }
                EditorEvent::Idle => root.synchronize_group(cx),
                EditorEvent::Status(message) => root.set_status(message.clone(), cx),
                EditorEvent::WorkspaceSearch(query) => {
                    root.search
                        .update(cx, |search, cx| search.start(query.clone(), cx));
                    root.shell.update(cx, |shell, cx| {
                        shell.activate_panel_view(PanelPosition::Left, id::VIEW_SEARCH.into(), cx)
                    });
                }
                EditorEvent::Completion(definition) => {
                    let (line, column) = root.editor.read(cx).cursor_line_column();
                    root.problems.update(cx, |problems, cx| {
                        problems.request(
                            *definition,
                            lapis_lsp::LspPosition {
                                line: line.saturating_sub(1) as u32,
                                utf16_column: column.saturating_sub(1) as u32,
                            },
                            cx,
                        )
                    });
                    let _ = window;
                }
            }),
            cx.subscribe(&conversation, |root, _, event, cx| {
                root.handle_conversation_event(event.clone(), cx);
            }),
            cx.subscribe(&problems, |root, _, event, cx| match event {
                ProblemsEvent::Status(message) => root.set_status(message.clone(), cx),
                ProblemsEvent::Definition(target) => {
                    let target = target.clone();
                    root.editor.update(cx, |editor, cx| {
                        editor.submit(
                            false,
                            move |client| {
                                client
                                    .editor
                                    .open_definition(&target)
                                    .map(|_| ())
                                    .map_err(|error| error.to_string())
                            },
                            cx,
                        )
                    });
                }
            }),
            cx.observe(&editor, |root, editor, cx| {
                let editor = editor.read(cx);
                let cursor = editor.cursor_line_column();
                let target = editor
                    .document_id()
                    .map(|id| (id, editor.document().revision()));
                let selected = editor.selected_text();
                root.tasks
                    .update(cx, |tasks, _| tasks.set_selected_text(selected));
                root.problems
                    .update(cx, |problems, cx| problems.set_target(target, cx));
                root.shell.update(cx, |shell, cx| {
                    if shell.session.cursor != cursor {
                        shell.session.cursor = cursor;
                        cx.notify();
                    }
                });
                root.sync_preview(cx);
                root.schedule_persist(cx);
            }),
            cx.observe(&shell, |root, _, cx| {
                root.record_window_layout(cx);
                root.sync_preview(cx);
                root.schedule_persist(cx);
            }),
            cx.observe(&terminal, |root, _, cx| root.schedule_persist(cx)),
        ];

        let weak = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            weak.update(cx, |root, cx| root.request_close(window, cx))
                .unwrap_or(true)
        });
        subscriptions.push(cx.on_app_quit(|root, cx| {
            let view = root.persisted_view(cx);
            let worker = root.worker.clone();
            let receiver = worker.request(move |client| client.persist_view(view));
            async move {
                let _worker = worker;
                loop {
                    match receiver.try_recv() {
                        Ok(result) => {
                            if let Err(error) = result {
                                eprintln!("終了時の保存に失敗しました: {error}");
                            }
                            break;
                        }
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
                        Err(std::sync::mpsc::TryRecvError::Empty) => {
                            gpui::Timer::after(std::time::Duration::from_millis(16)).await;
                        }
                    }
                }
            }
        }));
        window.focus(&editor.focus_handle(cx));
        Self {
            group,
            shell,
            editor,
            files,
            git,
            tasks,
            terminal,
            problems,
            search,
            conversation,
            preview,
            preview_revision: None,
            line_ending_cache: None,
            settings,
            quick_search,
            path_request: None,
            git_input: None,
            _subscriptions: subscriptions,
            double_shift: DoubleShiftDetector::default(),
            worker,
            factory,
            persist_task: None,
            persist_signature: None,
            native_smoke_requested: initial.native_window_smoke,
            workbench: WorkbenchState::default(),
            task_context: None,
            task_context_gate: Default::default(),
            task_context_resolution: Default::default(),
            terminal_context_resolved: false,
        }
    }

    fn sync_preview(&mut self, cx: &mut Context<Self>) {
        if !self.shell.read(cx).preview_visible() {
            return;
        }
        let editor = self.editor.read(cx);
        let Some(document_id) = editor.document_id() else {
            if self.preview_revision.take().is_some() {
                self.preview
                    .update(cx, |preview, cx| preview.update(String::new(), cx));
            }
            return;
        };
        let version = (document_id, editor.document().revision());
        if self.preview_revision.as_ref() == Some(&version) {
            return;
        }
        let text = editor.document().content_for_save();
        self.preview_revision = Some(version);
        self.preview
            .update(cx, |preview, cx| preview.update(text, cx));
    }

    /// Updates root feature contexts and shell presentation from a snapshot filtered for this host.
    /// Restores the saved shell layout when the active conversation changes.
    pub(crate) fn apply_snapshot(&mut self, snapshot: &ClientSnapshot, cx: &mut Context<Self>) {
        let switched = self.workbench.context_task() != Some(&snapshot.conversation);
        let workbench_view = self.workbench.apply_task_snapshot(
            snapshot.conversation.clone(),
            snapshot.view.clone(),
            switched,
        );
        let record = snapshot
            .conversations
            .iter()
            .find(|record| record.id == snapshot.conversation);
        self.files
            .update(cx, |files, cx| files.update(snapshot, cx));
        self.git.update(cx, |git, cx| {
            git.set_workspace(snapshot.workspace.clone(), cx);
            let role = record
                .map(|record| format!("{:?}", record.source_role))
                .unwrap_or_default();
            git.set_context_label(format!(
                "{} · {}",
                role,
                snapshot
                    .root
                    .as_ref()
                    .map(|root| root.display().to_string())
                    .unwrap_or_default()
            ));
        });
        let selected = self
            .workbench
            .selected_task()
            .cloned()
            .filter(|id| snapshot.conversations.iter().any(|record| &record.id == id))
            .unwrap_or_else(|| snapshot.conversation.clone());
        if self.workbench.selected_task() != Some(&selected) {
            self.workbench
                .select_task(selected.clone(), snapshot.view.clone());
        }
        let tasks_key = ViewInstanceKey::new(
            crate::shell::PanelTab::tool(id::VIEW_ASSISTANT).stable_view_key(),
        )
        .unwrap();
        let task_reference = self
            .workbench
            .task_reference(&tasks_key)
            .cloned()
            .unwrap_or_else(|| selected.clone());
        self.sync_task_context(snapshot, &task_reference, cx);
        self.search.update(cx, |search, cx| {
            search.set_workspace(snapshot.root.clone(), cx)
        });
        self.settings.update(cx, |settings, cx| {
            settings.set_workspace(snapshot.root.clone(), cx)
        });
        let terminal_key =
            ViewInstanceKey::new(crate::shell::PanelTab::tool(id::VIEW_TERMINAL).stable_view_key())
                .unwrap();
        let terminal_scope = self.workbench.view_scope(&terminal_key);
        let resource = match terminal_scope {
            Some(ViewScope::Resource(resource)) => {
                Some(lapis_terminal::TerminalId::new(resource.clone()))
            }
            _ => None,
        };
        let terminal_record = terminal_context_record(
            terminal_scope,
            &selected,
            self.terminal.read(cx).context_task(),
            &snapshot.conversations,
        );
        self.terminal_context_resolved = terminal_record.is_some();
        self.terminal.update(cx, |terminal, cx| {
            terminal.bind_resource(resource);
            if let Some(record) = terminal_record {
                terminal.set_context(
                    record.workspace.root.clone(),
                    record.id.clone(),
                    record.terminals.clone(),
                    cx,
                );
            }
        });
        self.conversation.update(cx, |conversation, cx| {
            conversation.context = Some(snapshot.conversation.clone());
            conversation.update(
                snapshot.conversations.clone(),
                snapshot.projects.clone(),
                snapshot.project_error.clone(),
                selected,
                cx,
            )
        });
        let editor = self.editor.read(cx);
        let line_ending_version = editor
            .document_id()
            .map(|id| (id, editor.document().revision()));
        let line_ending = match self.line_ending_cache.as_ref() {
            Some((cached, ending)) if line_ending_version.as_ref() == Some(cached) => *ending,
            _ => {
                let ending = if editor.document().contains_crlf() {
                    "CRLF"
                } else {
                    "LF"
                };
                self.line_ending_cache = line_ending_version.map(|version| (version, ending));
                ending
            }
        };
        let presentation = ShellPresentation {
            documents: snapshot.tabs.clone(),
            active: snapshot.active.clone(),
            root: snapshot.root.clone(),
            workspace_name: snapshot.workspace_name.clone(),
            cursor: editor.cursor_line_column(),
            line_ending,
            encoding: editor.document().encoding(),
            recent_workspaces: snapshot
                .conversations
                .iter()
                .filter_map(|record| record.workspace.root.clone())
                .collect(),
        };
        let retained = self.workbench.retained_views();
        let hidden = self.workbench.hidden_views();
        self.shell.update(cx, |shell, cx| {
            shell.update_presentation(presentation, cx);
            if let Some(view) = workbench_view {
                shell.restore_workbench_view(view, &retained, cx);
            }
            shell.apply_scope_visibility(hidden, cx);
        });
        self.sync_preview(cx);
    }

    fn sync_task_context(
        &mut self,
        snapshot: &ClientSnapshot,
        selected: &lapis_editor_core::ConversationId,
        cx: &mut Context<Self>,
    ) {
        let record = snapshot
            .conversations
            .iter()
            .find(|record| &record.id == selected);
        let key = (
            selected.clone(),
            record.and_then(|record| record.workspace.root.clone()),
        );
        if self.task_context.as_ref() == Some(&key) {
            return;
        }
        self.task_context = Some(key.clone());
        self.task_context_resolution = TaskContextResolution::Pending;
        self.task_context_gate.invalidate();
        let token = self.task_context_gate.issue();
        self.tasks.update(cx, |tasks, cx| {
            tasks.set_conversation(selected.clone());
            tasks.set_workspace(None, cx);
        });
        let selected_execution = record.and_then(|record| record.selected_execution.clone());
        let task = selected.clone();
        let admitted = self.worker.submit(
            cx,
            move |client| {
                client
                    .conversation
                    .task_workspace_id(&task, &client.editor)
                    .map_err(|error| error.to_string())
            },
            move |root, result, cx| {
                // A→B→Aや作業ソース変更後の古い解決結果を適用しない。
                if !root.task_context_gate.accept(token) || root.task_context.as_ref() != Some(&key)
                {
                    return;
                }
                match result {
                    Ok(workspace) => {
                        root.tasks.update(cx, |tasks, cx| {
                            tasks.set_workspace(workspace, cx);
                            tasks.selected_execution = selected_execution;
                        });
                        root.task_context_resolution = TaskContextResolution::Ready;
                    }
                    Err(error) => {
                        root.task_context_resolution = TaskContextResolution::Unavailable;
                        root.set_status(error, cx);
                    }
                }
                root.persist_signature = None;
                root.schedule_persist(cx);
            },
        );
        if let Err(error) = admitted {
            self.task_context_resolution = TaskContextResolution::Unavailable;
            self.set_status(error, cx);
            self.persist_signature = None;
            self.schedule_persist(cx);
        }
    }

    /// Task切替前のUI状態だけをWorkbenchへ保存します。Editor/Terminalの
    /// Resource所有権や実行中Sessionはこの経路から変更しません。
    fn capture_workbench_view(&mut self, cx: &gpui::App) {
        let task = self.workbench.context_task().cloned();
        let view = self.shell.read(cx).view_state();
        self.workbench.prepare_task_switch(task.as_ref(), &view);
    }

    /// 一覧のTask選択は共通APIへ渡し、作業環境を変える操作だけWindow数で制限する。
    pub(crate) fn handle_conversation_event(
        &mut self,
        event: ConversationEvent,
        cx: &mut Context<Self>,
    ) {
        if let ConversationEvent::Switch(id) = event {
            self.dispatch_workbench_command(UiCommand::SelectTask(id), cx);
            return;
        }
        if self.group.read(cx).members.len() > 1 || self.group.read(cx).closing {
            self.set_status(
                "別Windowのタブを閉じるか再ドックしてから会話を切り替えてください".into(),
                cx,
            );
            return;
        }
        let view = self.persisted_view(cx);
        self.editor.update(cx, |editor, cx| {
            editor.submit(
                false,
                move |client| {
                    client.persist_view(view)?;
                    match event {
                        ConversationEvent::RegisterProject => {
                            if let Some(path) = client.editor.file_dialog().choose_workspace_path()
                            {
                                client
                                    .conversation
                                    .register_project(&mut client.editor, path)
                                    .map_err(|error| error.to_string())?;
                            }
                            Ok(())
                        }
                        ConversationEvent::RegisterSource(project) => {
                            if let Some(path) = client.editor.file_dialog().choose_workspace_path()
                            {
                                client
                                    .conversation
                                    .register_source(&mut client.editor, &project, path)
                                    .map_err(|error| error.to_string())?;
                            }
                            Ok(())
                        }
                        ConversationEvent::Create {
                            project,
                            primary,
                            sub,
                        } => client
                            .conversation
                            .create_task(&mut client.editor, &project, primary, sub)
                            .map(|_| ())
                            .map_err(|error| error.to_string()),
                        ConversationEvent::Source(role) => client
                            .conversation
                            .switch_source(&mut client.editor, role)
                            .map_err(|error| error.to_string()),
                        ConversationEvent::Switch(id) => client
                            .conversation
                            .switch(&id, &mut client.editor)
                            .map(|_| ())
                            .map_err(|error| error.to_string()),
                    }
                },
                cx,
            )
        });
    }

    /// 左のTask一覧、Command、ショートカットが共有するTask選択APIです。
    fn select_task(&mut self, id: lapis_editor_core::ConversationId, cx: &mut Context<Self>) {
        if self.group.read(cx).closing || self.group.read(cx).synchronization_required {
            self.set_status(
                "終了・再同期処理の完了後にTaskを選択してください".into(),
                cx,
            );
            return;
        }
        if self.editor.read(cx).context_transition_pending()
            || (self.workbench.mode() == WorkbenchMode::Agent && self.editor.read(cx).has_pending())
        {
            self.set_status("処理の完了後にTaskを選択してください".into(), cx);
            return;
        }
        if self.workbench.mode() == WorkbenchMode::Ide {
            let Some(snapshot) = self.group.read(cx).latest.clone() else {
                return;
            };
            let Some(record) = snapshot.conversations.iter().find(|record| record.id == id) else {
                self.set_status("Taskが見つかりません".into(), cx);
                return;
            };
            self.workbench.select_task(id, record.view.clone());
            self.synchronize_group(cx);
            return;
        }
        if self.workbench.context_task() == Some(&id) {
            return;
        }
        if self.group.read(cx).members.len() > 1 || self.group.read(cx).closing {
            self.set_status(
                "別Windowのタブを閉じるか再ドックしてからTaskを切り替えてください".into(),
                cx,
            );
            return;
        }
        self.capture_workbench_view(cx);
        let view = self.persisted_view(cx);
        self.editor.update(cx, |editor, cx| {
            editor.submit(
                false,
                move |client| {
                    client.persist_view(view)?;
                    client
                        .conversation
                        .switch(&id, &mut client.editor)
                        .map(|_| ())
                        .map_err(|error| error.to_string())
                },
                cx,
            )
        });
    }

    fn dispatch_workbench_command(&mut self, command: UiCommand, cx: &mut Context<Self>) {
        match command {
            UiCommand::BindViewToSelectedTask(view) => self.bind_view_to_selected_task(view, cx),
            UiCommand::SelectTask(id) => self.select_task(id, cx),
            UiCommand::SetWorkbenchMode(mode) => self.set_workbench_mode(mode, cx),
            UiCommand::SetViewBinding { view, scope } => self.set_view_binding(view, scope, cx),
            _ => {}
        }
    }

    /// Mode変更もTask選択と同じWorkbench APIを経由します。
    fn set_workbench_mode(&mut self, mode: WorkbenchMode, cx: &mut Context<Self>) {
        if self.workbench.mode() == mode {
            return;
        }
        if self.group.read(cx).closing
            || self.group.read(cx).synchronization_required
            || self.editor.read(cx).has_pending()
        {
            self.set_status("処理の完了後にModeを切り替えてください".into(), cx);
            return;
        }
        let task = self.workbench.context_task().cloned();
        let current = self.shell.read(cx).view_state();
        let mut candidate = self.workbench.clone();
        let restored = candidate.set_mode(mode, task.as_ref(), &current);
        let target = candidate.mode_context().cloned();
        if target == task || target.is_none() {
            self.workbench = candidate;
            if let Some(view) = restored {
                self.restore_workbench_layout(view, cx);
            }
            self.schedule_persist(cx);
            return;
        }
        if self.group.read(cx).members.len() > 1 {
            self.set_status(
                "別Windowのタブを閉じるか再ドックしてからModeを切り替えてください".into(),
                cx,
            );
            return;
        }
        let previous = self.persisted_view(cx);
        if !self
            .editor
            .update(cx, |editor, _| editor.begin_context_transition())
        {
            return;
        }
        let target = target.unwrap();
        let admitted = self.worker.submit(
            cx,
            move |client| {
                let result = client.persist_view(previous).and_then(|_| {
                    client
                        .conversation
                        .switch(&target, &mut client.editor)
                        .map_err(|error| error.to_string())
                });
                // 失敗時も確認済みの実状態を通知し、部分成功や切断を表示終了と混同しない。
                Ok((result, client.reconcile_snapshot()))
            },
            move |root, outcome, cx| {
                let (result, snapshot) = match outcome {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        root.editor
                            .update(cx, |editor, cx| editor.finish_context_transition(None, cx));
                        root.group.update(cx, |group, cx| {
                            group.synchronization_required = true;
                            group.status = error;
                            cx.notify();
                        });
                        return;
                    }
                };
                let success = result.is_ok();
                if success {
                    let current = root.shell.read(cx).view_state();
                    root.workbench.commit_mode_transition(candidate, &current);
                }
                root.editor.update(cx, |editor, cx| {
                    editor.finish_context_transition(Some(&snapshot), cx)
                });
                root.group
                    .update(cx, |group, cx| group.publish(snapshot, cx));
                root.synchronize_group(cx);
                if success {
                    if mode == WorkbenchMode::Ide
                        && let Some(view) = restored
                    {
                        root.restore_workbench_layout(view, cx);
                    }
                    root.schedule_persist(cx);
                } else if let Err(error) = result {
                    root.set_status(error, cx);
                }
            },
        );
        if let Err(error) = admitted {
            self.editor
                .update(cx, |editor, cx| editor.finish_context_transition(None, cx));
            self.set_status(error, cx);
        }
    }

    fn bind_view_to_selected_task(&mut self, view: ViewInstanceKey, cx: &mut Context<Self>) {
        if let Some(task) = self.workbench.selected_task().cloned() {
            self.set_view_binding(view, ViewScope::Task(task), cx);
        }
    }

    fn sync_view_scopes(&mut self, cx: &mut Context<Self>) {
        for (tab, _) in self.shell.read(cx).shell.layout.placements() {
            let key = ViewInstanceKey::new(tab.stable_view_key()).unwrap();
            let scope = if tab
                .view_id()
                .is_some_and(|view| view.as_str() == id::VIEW_ASSISTANT)
            {
                ViewScope::FollowSelectedTask
            } else if (self.workbench.mode() == WorkbenchMode::Agent
                || matches!(tab, crate::shell::PanelTab::Document(_)))
                && let Some(task) = self.workbench.context_task().cloned()
            {
                ViewScope::Task(task)
            } else {
                ViewScope::SharedWorkbench
            };
            self.workbench.ensure_view_binding(key, scope);
        }
        let hidden = self.workbench.hidden_views();
        self.shell
            .update(cx, |shell, cx| shell.apply_scope_visibility(hidden, cx));
    }

    fn restore_workbench_layout(
        &mut self,
        view: lapis_desktop_client::ConversationViewState,
        cx: &mut Context<Self>,
    ) {
        let retained = self.workbench.retained_views();
        let hidden = self.workbench.hidden_views();
        self.shell.update(cx, |shell, cx| {
            shell.restore_workbench_view(view, &retained, cx);
            shell.apply_scope_visibility(hidden, cx);
        });
    }

    /// View所属の変更は表示状態だけに限定し、Host Resourceの終了・複製を行いません。
    fn set_view_binding(
        &mut self,
        view: ViewInstanceKey,
        scope: ViewScope,
        cx: &mut Context<Self>,
    ) {
        let key = view.clone();
        if !self
            .shell
            .read(cx)
            .shell
            .layout
            .placements()
            .iter()
            .any(|(tab, _)| tab.stable_view_key() == view.as_str())
        {
            self.set_status("表示対象のViewがありません".into(), cx);
            return;
        }
        if let ViewScope::Task(task) = &scope
            && !self
                .conversation
                .read(cx)
                .records
                .iter()
                .any(|record| &record.id == task)
        {
            self.set_status("所属先のTaskがありません".into(), cx);
            return;
        }
        if let ViewScope::Resource(resource) = &scope {
            let terminal_key = crate::shell::PanelTab::tool(id::VIEW_TERMINAL).stable_view_key();
            let supported = view.as_str() == terminal_key
                && (self
                    .terminal
                    .read(cx)
                    .summaries()
                    .iter()
                    .any(|terminal| terminal.id.as_str() == resource)
                    || self.conversation.read(cx).records.iter().any(|record| {
                        record.terminals.iter().any(|terminal| {
                            terminal
                                .id
                                .as_ref()
                                .is_some_and(|id| id.as_str() == resource)
                        })
                    }));
            if !supported {
                self.set_status("このViewのResource固定参照には対応していません".into(), cx);
                return;
            }
        }
        self.workbench.bind_view(view, scope);
        debug_assert!(self.workbench.view_scope(&key).is_some());
        self.sync_view_scopes(cx);
        self.synchronize_group(cx);
        self.schedule_persist(cx);
        cx.notify();
    }

    fn execution_for_persistence(&self, cx: &gpui::App) -> Option<lapis_editor_core::ExecutionId> {
        execution_for_context(
            self.task_context_resolution
                .confirmed_task(self.task_context.as_ref().map(|(task, _)| task)),
            self.workbench.context_task(),
            self.tasks.read(cx).selected_execution.clone(),
            self.conversation
                .read(cx)
                .records
                .iter()
                .find(|record| Some(&record.id) == self.workbench.context_task())
                .and_then(|record| record.selected_execution.clone()),
        )
    }

    fn terminal_owner_for_persistence(
        &self,
        cx: &gpui::App,
    ) -> Option<lapis_editor_core::ConversationId> {
        self.terminal_context_resolved
            .then(|| self.terminal.read(cx).persistence_owner())
            .flatten()
    }

    /// Captures this root's editor, layout, task, terminal, and draft state for later persistence.
    pub(crate) fn persisted_view(&self, cx: &gpui::App) -> PersistedView {
        let mut layout = self.shell.read(cx).view_state();
        self.workbench.persisted_view(&mut layout);
        PersistedView {
            document: self.editor.read(cx).view_state(),
            layout,
            execution: self.execution_for_persistence(cx),
            terminals: self.terminal.read(cx).summaries(),
            terminal_owner: self.terminal_owner_for_persistence(cx),
            input: self
                .editor
                .read(cx)
                .document_id()
                .map(|_| self.editor.read(cx).document().content_for_save()),
        }
    }

    /// Starts closing this root's entire group and returns false to defer native window removal.
    fn request_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.close_hosts(crate::shell::window_group::ROOT_HOST, None, window, cx);
        false
    }

    /// Debounces persistence by 500 milliseconds when document versions, editor view, or layout change.
    /// Skips the write while closing; asynchronous persistence errors become status messages.
    pub(crate) fn schedule_persist(&mut self, cx: &mut Context<Self>) {
        self.sync_view_scopes(cx);
        let mut layout = self.shell.read(cx).view_state();
        let task = self.workbench.context_task().cloned();
        self.workbench.prepare_task_switch(task.as_ref(), &layout);
        self.workbench.persisted_view(&mut layout);
        let versions = self
            .group
            .read(cx)
            .latest
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .documents
                    .iter()
                    .map(|cache| {
                        (
                            cache.id.clone(),
                            cache.document.revision(),
                            cache.document.is_dirty(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let terminal = self.terminal.read(cx);
        let owner = self.terminal_owner_for_persistence(cx);
        let summaries = if owner.is_some() {
            terminal
                .summaries()
                .iter()
                .map(|terminal| lapis_desktop_client::RestoredTerminal {
                    id: Some(terminal.id.clone()),
                    cwd: terminal.cwd.clone(),
                    status: terminal.status,
                    columns: terminal.screen.size.columns,
                    rows: terminal.screen.size.rows,
                })
                .collect()
        } else {
            Vec::new()
        };
        let signature = (
            versions,
            self.editor.read(cx).view_state(),
            layout,
            self.execution_for_persistence(cx),
            owner,
            summaries,
        );
        if self.persist_signature.as_ref() == Some(&signature) {
            return;
        }
        self.persist_signature = Some(signature);
        self.persist_task = Some(cx.spawn(async move |this, cx| {
            gpui::Timer::after(std::time::Duration::from_millis(500)).await;
            let _ = this.update(cx, |root, cx| {
                if root.group.read(cx).closing
                    || root.group.read(cx).synchronization_required
                    || root.editor.read(cx).has_in_flight()
                    || root.task_context_resolution == TaskContextResolution::Pending
                {
                    // 指紋はデバウンス用であり、見送った書き込みは再同期後に再試行できるようにする。
                    // Workspace切替の確認前の表示を、Worker側の新Taskへ保存しない。
                    root.persist_signature = None;
                    return;
                }
                let view = root.persisted_view(cx);
                let _ = root.worker.submit(
                    cx,
                    move |client| {
                        client
                            .edit_chain
                            .apply(client.edit_chain.epoch(), || Ok(()))?;
                        client.persist_view(view)
                    },
                    |root, result, cx| {
                        if let Err(error) = result {
                            root.set_status(error, cx);
                        }
                    },
                );
            });
        }));
    }

    /// Updates the root shell's status and publishes it to the window group.
    pub(crate) fn set_status(&mut self, status: String, cx: &mut Context<Self>) {
        self.group.update(cx, |group, cx| {
            group.status = status.clone();
            cx.notify();
        });
        self.shell
            .update(cx, |shell, cx| shell.set_status(status, cx));
    }

    fn submit_path(&mut self, input: String, cx: &mut Context<Self>) {
        let Some(request) = self.path_request.take() else {
            return;
        };
        let (root, document) = match request {
            PathRequest::Open(root) => (root, None),
            PathRequest::Save(root, document) => (root, Some(document)),
            PathRequest::CloseSave(sender) => {
                let _ = sender.send(Ok(input));
                return;
            }
        };
        self.editor.update(cx, |editor, cx| {
            editor.submit(
                false,
                move |client| {
                    let target = client
                        .editor
                        .workspace_path(&root, &input)
                        .map_err(|error| error.to_string())?;
                    if let Some(document) = document {
                        if client.editor.active_document_id() != Some(&document) {
                            return Err(
                                "保存対象が切り替わりました。保存をやり直してください".to_owned()
                            );
                        }
                        client
                            .editor
                            .save_document_to(target)
                            .map_err(|error| error.to_string())?;
                        Ok(())
                    } else {
                        client
                            .editor
                            .open_path(target)
                            .map_err(|error| error.to_string())
                    }
                },
                cx,
            );
        });
    }

    pub(crate) fn remote_path_scope(&self, cx: &gpui::App) -> Option<std::path::PathBuf> {
        self.factory
            .path_scope()
            .map(|initial| self.shell.read(cx).session.root.clone().unwrap_or(initial))
    }

    fn show_path_request(
        &mut self,
        request: PathRequest,
        title: &'static str,
        suggested: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.path_request = Some(request);
        self.quick_search
            .update(cx, |search, cx| search.open_path(title, suggested, cx));
        self.shell.update(cx, |shell, cx| {
            shell.activate_panel_view(PanelPosition::Right, id::VIEW_COMMAND_SEARCH.into(), cx)
        });
        window.focus(&self.quick_search.read(cx).focus_handle());
    }

    pub(crate) fn request_close_path(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> std::sync::mpsc::Receiver<Result<String, String>> {
        let (sender, receiver) = std::sync::mpsc::channel();
        self.show_path_request(
            PathRequest::CloseSave(sender),
            "Hostに保存して閉じる",
            "new-file.txt",
            window,
            cx,
        );
        receiver
    }

    /// Dispatches root commands, ignoring them while closing.
    /// Workspace switches are rejected with a status message while detached windows exist.
    pub(crate) fn command(
        &mut self,
        command: UiCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.group.read(cx).closing {
            return;
        }
        if self.group.read(cx).synchronization_required
            && !matches!(
                command,
                UiCommand::CloseWindow | UiCommand::CloseDocument(_) | UiCommand::Copy
            )
        {
            self.set_status(
                "状態を再確認するため、もう一度CloseまたはQuitを実行してください".into(),
                cx,
            );
            return;
        }
        if self.group.read(cx).members.len() > 1
            && matches!(
                command,
                UiCommand::OpenWorkspace | UiCommand::OpenRecent(_) | UiCommand::CloseWorkspace
            )
        {
            self.set_status(
                "別Windowのタブを閉じるか再ドックしてからWorkspaceを切り替えてください".into(),
                cx,
            );
            return;
        }
        if matches!(
            command,
            UiCommand::OpenWorkspace | UiCommand::OpenRecent(_) | UiCommand::CloseWorkspace
        ) {
            let view = self.persisted_view(cx);
            self.editor.update(cx, |editor, cx| {
                editor.submit(
                    false,
                    move |client| {
                        client.persist_view(view)?;
                        if matches!(command, UiCommand::CloseWorkspace) {
                            return client
                                .conversation
                                .close_workspace(&mut client.editor)
                                .map_err(|error| error.to_string());
                        }
                        let path = match command {
                            UiCommand::OpenRecent(path) => Some(path),
                            _ => client.editor.file_dialog().choose_workspace_path(),
                        };
                        if let Some(path) = path {
                            client
                                .conversation
                                .open_project(&mut client.editor, path)
                                .map_err(|error| error.to_string())?;
                        }
                        Ok(())
                    },
                    cx,
                )
            });
            return;
        }
        if let Some(root) = self.remote_path_scope(cx) {
            let request = match &command {
                UiCommand::OpenFile => Some(PathRequest::Open(root)),
                UiCommand::SaveAs => self
                    .editor
                    .read(cx)
                    .document_id()
                    .map(|id| PathRequest::Save(root, id)),
                UiCommand::Save if self.editor.read(cx).document().path().is_none() => self
                    .editor
                    .read(cx)
                    .document_id()
                    .map(|id| PathRequest::Save(root, id)),
                _ => None,
            };
            if let Some(request) = request {
                let (title, suggested) = match &request {
                    PathRequest::Open(_) => ("Hostのファイルを開く", String::new()),
                    PathRequest::Save(_, _) => (
                        "Hostに名前を付けて保存",
                        self.editor
                            .read(cx)
                            .document()
                            .path()
                            .and_then(|path| path.file_name())
                            .and_then(|name| name.to_str())
                            .unwrap_or("new-file.txt")
                            .to_owned(),
                    ),
                    PathRequest::CloseSave(_) => unreachable!(),
                };
                self.show_path_request(request, title, &suggested, window, cx);
                return;
            }
        }
        match command {
            UiCommand::Save if self.editor.read(cx).document_id().is_none() => {}
            command @ (UiCommand::DetachTab(_)
            | UiCommand::DockTab(_)
            | UiCommand::MoveTab(_, _)
            | UiCommand::ReleaseTab(_, _)) => self.host_command(0, command, window, cx),
            UiCommand::Theme(theme) => self
                .settings
                .update(cx, |settings, cx| settings.select_theme(theme, cx)),
            UiCommand::QuickSearch => {
                self.path_request = None;
                self.git_input = None;
                self.double_shift.reset();
                let provider = CommandSearchProvider::from_registry(
                    &features::bundled_registry(),
                    &crate::localization::Localizer::bundled(),
                    &crate::keymap::KeymapRegistry::bundled(),
                );
                self.quick_search
                    .update(cx, |search, cx| search.open(provider, cx));
                self.shell.update(cx, |shell, cx| {
                    shell.activate_panel_view(
                        PanelPosition::Right,
                        id::VIEW_COMMAND_SEARCH.into(),
                        cx,
                    )
                });
                window.focus(&self.quick_search.read(cx).focus_handle());
            }
            UiCommand::ActivateDocument(id) => {
                self.editor.update(cx, |editor, cx| editor.activate(id, cx));
                window.focus(&self.editor.focus_handle(cx));
            }
            UiCommand::CloseDocument(id) => self.close_hosts(0, Some(id), window, cx),
            UiCommand::SelectTask(id) => self.select_task(id, cx),
            UiCommand::SetWorkbenchMode(mode) => self.set_workbench_mode(mode, cx),
            UiCommand::SetViewBinding { view, scope } => self.set_view_binding(view, scope, cx),
            UiCommand::BindViewToSelectedTask(view) => self.bind_view_to_selected_task(view, cx),
            UiCommand::Undo => self
                .editor
                .update(cx, |editor, cx| editor.undo(&crate::app::Undo, window, cx)),
            UiCommand::Redo => self
                .editor
                .update(cx, |editor, cx| editor.redo(&crate::app::Redo, window, cx)),
            UiCommand::Copy => self
                .editor
                .update(cx, |editor, cx| editor.copy(&crate::app::Copy, window, cx)),
            UiCommand::Cut => self
                .editor
                .update(cx, |editor, cx| editor.cut(&crate::app::Cut, window, cx)),
            UiCommand::Paste => self.editor.update(cx, |editor, cx| {
                editor.paste(&crate::app::Paste, window, cx)
            }),
            UiCommand::CloneRepository => self.set_status("Clone from Git は準備中です".into(), cx),

            UiCommand::ConnectHost => {
                let factory = self.factory.clone();
                let admitted = self.worker.submit(
                    cx,
                    move |_| factory.open_host(),
                    |root, result, cx| {
                        if let Err(error) = result {
                            root.set_status(error, cx);
                        }
                    },
                );
                if let Err(error) = admitted {
                    self.set_status(error, cx);
                }
            }
            UiCommand::NewWindow => {
                let factory = self.factory.clone();
                let admitted = self.worker.submit(
                    cx,
                    move |_| Ok(factory.build()),
                    |root, result, cx| match result {
                        Ok((editor, services)) => crate::app::open_desktop_window(
                            editor,
                            services,
                            InitialView {
                                empty_window: true,
                                ..Default::default()
                            },
                            cx,
                        ),
                        Err(error) => root.set_status(error, cx),
                    },
                );
                if let Err(error) = admitted {
                    self.set_status(error, cx);
                }
            }
            UiCommand::CloseWindow => {
                self.request_close(window, cx);
            }
            command => {
                self.editor.update(cx, |editor, cx| {
                    editor.submit(
                        false,
                        move |client| match command {
                            UiCommand::NewDocument => {
                                client.editor.new_document();
                                Ok(())
                            }
                            UiCommand::OpenFile => client
                                .editor
                                .choose_file()
                                .map(|_| ())
                                .map_err(|error| error.to_string()),
                            UiCommand::OpenWorkspace => client
                                .editor
                                .choose_workspace()
                                .map(|_| ())
                                .map_err(|error| error.to_string()),
                            UiCommand::OpenRecent(root) => client
                                .editor
                                .open_workspace(root)
                                .map_err(|error| error.to_string()),
                            UiCommand::OpenPath(path) => client
                                .editor
                                .open_path(path)
                                .map_err(|error| error.to_string()),
                            UiCommand::Save => client
                                .editor
                                .save_document()
                                .map(|_| ())
                                .map_err(|error| error.to_string()),
                            UiCommand::SaveAs => client
                                .editor
                                .save_document_as()
                                .map(|_| ())
                                .map_err(|error| error.to_string()),
                            UiCommand::CloseWorkspace => client
                                .editor
                                .close_workspace()
                                .map_err(|error| error.to_string()),
                            UiCommand::NewFolder => client
                                .editor
                                .new_folder()
                                .map_err(|error| error.to_string()),
                            _ => Ok(()),
                        },
                        cx,
                    )
                });
            }
        }
    }
}
impl Render for DesktopRoot {
    /// Renders the root shell with application quit and command-search keyboard handlers.
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .on_action(cx.listener(|root, _: &crate::app::Quit, window, cx| {
                cx.stop_propagation();
                root.close_application(window, cx);
            }))
            .on_modifiers_changed(cx.listener(
                |root, event: &gpui::ModifiersChangedEvent, window, cx| {
                    if root
                        .double_shift
                        .modifiers_changed(event.modifiers, std::time::Instant::now())
                    {
                        root.command(UiCommand::QuickSearch, window, cx);
                    }
                },
            ))
            .on_key_down(cx.listener(|root, event: &gpui::KeyDownEvent, _, _| {
                if event.keystroke.key != "shift" {
                    root.double_shift.normal_key_pressed();
                }
            }))
            .child(self.shell.clone())
    }
}

/// Resourceの所有元が未確定なら、既存コンテキストへフォールバックしない。
fn terminal_context_record<'a>(
    scope: Option<&ViewScope>,
    selected: &lapis_editor_core::ConversationId,
    current: &lapis_editor_core::ConversationId,
    records: &'a [lapis_desktop_client::ConversationRecord],
) -> Option<&'a lapis_desktop_client::ConversationRecord> {
    if let Some(ViewScope::Resource(resource)) = scope {
        return records.iter().find(|record| {
            record.terminals.iter().any(|terminal| {
                terminal
                    .id
                    .as_ref()
                    .is_some_and(|id| id.as_str() == resource)
            })
        });
    }
    let task = match scope {
        Some(ViewScope::Task(task)) => task,
        Some(ViewScope::FollowSelectedTask) => selected,
        _ => current,
    };
    records.iter().find(|record| &record.id == task)
}

/// 表示中の選択を保存できるのは、そのViewが保存先Taskを参照している場合だけ。
fn execution_for_context(
    view_task: Option<&lapis_editor_core::ConversationId>,
    context_task: Option<&lapis_editor_core::ConversationId>,
    view_selection: Option<lapis_editor_core::ExecutionId>,
    saved_selection: Option<lapis_editor_core::ExecutionId>,
) -> Option<lapis_editor_core::ExecutionId> {
    if view_task.is_some() && view_task == context_task {
        view_selection
    } else {
        saved_selection
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lapis_desktop_client::ConversationViewState;
    use lapis_editor_core::{ConversationId, ExecutionId};

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
        use lapis_desktop_client::{ConversationRecord, RestoredTerminal};
        let a = ConversationId::new("task-a");
        let b = ConversationId::new("task-b");
        let record = |task: ConversationId, resource: &str| ConversationRecord {
            title: task.as_str().into(),
            id: task,
            workspace: lapis_desktop_client::WorkspaceSnapshot::default(),
            view: Default::default(),
            selected_execution: None,
            terminals: vec![RestoredTerminal {
                id: Some(lapis_terminal::TerminalId::new(resource)),
                cwd: "workspace".into(),
                status: lapis_terminal::TerminalStatus::Running,
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
            terminal_context_record(Some(&fixed), &b, &b, &records).map(|record| &record.id),
            Some(&a)
        );
        records[0].terminals.clear();
        records[0].workspace.root = Some("changed-source".into());
        assert!(terminal_context_record(Some(&fixed), &b, &a, &records).is_none());
        assert_eq!(
            terminal_context_record(Some(&ViewScope::Task(a.clone())), &b, &b, &records)
                .map(|record| &record.id),
            Some(&a)
        );
        records.remove(0);
        assert!(terminal_context_record(Some(&fixed), &b, &a, &records).is_none());
        assert!(
            terminal_context_record(Some(&ViewScope::Task(a.clone())), &b, &b, &records).is_none()
        );
        assert!(
            terminal_context_record(Some(&ViewScope::SharedWorkbench), &b, &a, &records).is_none()
        );
        assert_eq!(
            terminal_context_record(Some(&ViewScope::FollowSelectedTask), &b, &a, &records)
                .map(|record| &record.id),
            Some(&b)
        );
    }

    #[test]
    fn fixed_assistant_selection_does_not_overwrite_another_tasks_saved_execution() {
        let a = ConversationId::new("task-a");
        let b = ConversationId::new("task-b");
        let execution_a = ExecutionId::new("execution-a");
        let execution_b = ExecutionId::new("execution-b");
        let key = ViewInstanceKey::new(
            crate::shell::PanelTab::tool(id::VIEW_ASSISTANT).stable_view_key(),
        )
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
}
