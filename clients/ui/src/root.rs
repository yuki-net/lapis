use crate::{
    app::{ConnectionIdentity, DesktopServices, InitialView},
    client_adapter::{ClientSnapshot, ClientWorker, DocumentClient, PersistedView},
    commands::UiCommand,
    extension_ui::{ViewId, ZonePosition},
    features::{
        self,
        command_search::{
            CommandSearchProvider, DoubleShiftDetector, QuickSearch, QuickSearchEvent,
        },
        conversation::{ConversationEvent, ConversationFeature},
        editor::{Editor, EditorEvent},
        files::FilesFeature,
        git::{ChangesFeature, GitEvent, GitFeature, GitInputRequest},
        id,
        preview::PreviewFeature,
        problems::{ProblemsEvent, ProblemsFeature},
        resource_manager::{ProcessWorker, ResourceManagerFeature},
        search::SearchFeature,
        settings::SettingsFeature,
        tasks::TasksFeature,
        terminal::{TerminalFeature, TerminalFeatureEvent},
    },
    shell::main::{Shell, ShellPresentation},
    workbench::{ViewInstanceKey, ViewScope, WorkbenchMode, WorkbenchState},
};
use desktop_client::EditorSession;
use gpui::{
    AnyView, AppContext, Context, Entity, Focusable, Render, Subscription, Window, div, prelude::*,
};
use std::collections::HashMap;

/// 共有ハンドルから、適用判定を持つ watch を取り出す。
fn session_view_watch_of(
    handle: &crate::app::SessionViewHandle,
) -> &desktop_client::SessionViewWatch {
    #[cfg(not(target_family = "wasm"))]
    {
        handle.watch()
    }
    #[cfg(target_family = "wasm")]
    {
        handle
    }
}

enum PathRequest {
    Open(std::path::PathBuf),
    Save(std::path::PathBuf, editor_core::DocumentId),
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
        task: Option<&editor_core::ConversationId>,
    ) -> Option<&editor_core::ConversationId> {
        (self == Self::Ready).then_some(task).flatten()
    }
}

type PersistSignature = (
    Vec<(editor_core::DocumentId, document::Revision, bool)>,
    Option<(
        editor_core::DocumentId,
        std::ops::Range<usize>,
        usize,
        f32,
        f32,
    )>,
    desktop_client::ConversationViewState,
    Option<editor_core::ExecutionId>,
    Option<editor_core::ConversationId>,
    Vec<desktop_client::RestoredTerminal>,
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
    resources: Entity<ResourceManagerFeature>,
    search: Entity<SearchFeature>,
    conversation: Entity<ConversationFeature>,
    preview: Entity<PreviewFeature>,
    preview_revision: Option<(editor_core::DocumentId, document::Revision)>,
    line_ending_cache: Option<((editor_core::DocumentId, document::Revision), &'static str)>,
    settings: Entity<SettingsFeature>,
    quick_search: Entity<QuickSearch>,
    universal_search: Entity<QuickSearch>,
    universal_search_open: bool,
    universal_previous_focus: Option<gpui::FocusHandle>,
    path_request: Option<PathRequest>,
    git_input: Option<GitInputRequest>,
    _subscriptions: Vec<Subscription>,
    double_shift: DoubleShiftDetector,
    pub(crate) worker: ClientWorker<DocumentClient>,
    factory: crate::app::WindowFactory,
    persist_task: Option<gpui::Task<()>>,
    persist_signature: Option<PersistSignature>,
    workbench_publish_signature: Option<desktop_client::ConversationViewState>,
    /// Workspace view の購読から受け取った共有 view を UI へ反映する。
    /// 適用中に次の通知が来ても取りこぼさないよう、完了後にもう一度確認する。
    session_view: Option<crate::app::SessionViewHandle>,
    session_view_applying: bool,
    /// 反映中に見送った view 通知があるか。完了時に再確認する。
    /// 見送った時だけ立て、変化が無いのに再投入しない。
    session_view_pending: bool,
    /// 自分のタブ変更を共有へ送る debounce。
    session_view_task: Option<gpui::Task<()>>,
    /// 接続中の Workspace の共有状態が古い可能性があるか。表示に出す。
    session_view_stale: bool,
    /// Editor の処理中に見送った snapshot 再取得。Idle で実行する。
    session_view_resync_pending: bool,
    #[cfg(not(target_family = "wasm"))]
    native_smoke_requested: bool,
    #[cfg(not(target_family = "wasm"))]
    workbench_watch: Option<desktop_client::WorkbenchWatch>,
    #[cfg(not(target_family = "wasm"))]
    workbench_revision: u64,
    workbench: WorkbenchState,
    task_context_gate: crate::client_adapter::ResponseGate,
    task_context_resolution: TaskContextResolution,
    task_context: Option<(editor_core::ConversationId, Option<std::path::PathBuf>)>,
    connection: Option<ConnectionIdentity>,
    pending_initial_task: Option<editor_core::ConversationId>,
    window_title: Option<String>,
    task_url_initialized: bool,
    last_task_for_url: Option<editor_core::ConversationId>,
}

impl DesktopRoot {
    #[cfg(target_os = "windows")]
    pub(crate) fn smoke_workbench(&self) -> WorkbenchState {
        self.workbench.clone()
    }

    #[cfg(target_os = "windows")]
    pub(crate) fn restore_smoke_workbench(&mut self, workbench: WorkbenchState) {
        self.workbench = workbench;
    }

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
        let connection = services.connection.clone();
        let pending_initial_task = services.initial_task.clone();
        #[cfg(not(target_family = "wasm"))]
        let workbench_watch = services.workbench_watch.clone();
        let session_view = services.session_view.clone();
        let session_view_watch = session_view
            .as_ref()
            .map(|handle| session_view_watch_of(handle).clone());
        #[cfg(not(target_family = "wasm"))]
        let workbench_revision = workbench_watch
            .as_ref()
            .map(desktop_client::WorkbenchWatch::revision)
            .unwrap_or_default();
        let conversation_id = services.conversation.active_id().clone();
        let terminal_root = session.workspace_root().map(ToOwned::to_owned);
        let initial_locale = services.settings.settings().locale();
        if services.terminal.terminals().is_empty() {
            let summaries = services
                .conversation
                .active_record()
                .filter(|record| record.workspace.root == terminal_root)
                .map(|record| record.terminals.clone())
                .unwrap_or_default();
            services.terminal.restore_summaries(&summaries);
        }
        let document_connection = Some(session.document_connection());
        let initial_workspace = session.workspace_id().cloned();
        let worker = ClientWorker::new(DocumentClient::new(
            session,
            services.conversation,
            services.lsp,
            services.git.backend(),
            session_view.clone(),
        ));
        let process_worker = ClientWorker::new(ProcessWorker::new(
            services.process.clone(),
            initial_workspace.clone(),
        ));
        let editor =
            cx.new(|cx| Editor::new_with_subscription(worker.clone(), document_connection, cx));
        // Web の DocumentConnection は同期購読を持たないため、Host からの
        // 文書変更通知で Editor に再同期を要求する。
        #[cfg(target_family = "wasm")]
        if let Some(mut documents) = desktop_client::web_host::document_changes() {
            let editor = editor.downgrade();
            cx.spawn(async move |_, cx| {
                use futures_util::StreamExt;
                while documents.next().await.is_some() {
                    if editor
                        .update(cx, |editor, cx| editor.request_sync(cx))
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();
        }
        let files = cx.new(|_| FilesFeature::new());
        let git = cx.new(|cx| GitFeature::new(services.git, cx));
        let changes = cx.new(|cx| ChangesFeature::new(git.clone(), cx));
        let tasks = cx.new(|cx| TasksFeature::new(services.task, cx));
        let terminal = cx
            .new(|cx| TerminalFeature::new(services.terminal, terminal_root, conversation_id, cx));
        let problems = cx.new(|cx| ProblemsFeature::new(worker.clone(), cx));
        let resources =
            cx.new(|_| ResourceManagerFeature::new(process_worker, initial_workspace.clone()));
        let search = cx.new(|cx| SearchFeature::new(services.search, cx));
        let conversation = cx.new(|_| ConversationFeature::new());
        let preview = cx.new(|_| PreviewFeature::new());
        let settings =
            cx.new(|cx| SettingsFeature::with_web_access(cx, services.web_access.clone()));
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
                            root.shell
                                .update(cx, |shell, cx| shell.close_zone(ZonePosition::Right, cx));
                        }
                        QuickSearchEvent::SubmitPath(path) => {
                            root.submit_path(path, cx);
                            root.shell
                                .update(cx, |shell, cx| shell.close_zone(ZonePosition::Right, cx));
                        }
                        QuickSearchEvent::SubmitText(text) => {
                            if let Some(request) = root.git_input.take() {
                                root.git.update(cx, |git, cx| {
                                    git.complete_input(request, text, window, cx)
                                });
                            }
                            root.shell
                                .update(cx, |shell, cx| shell.close_zone(ZonePosition::Right, cx));
                        }
                    }
                    crate::gpui_compat::focus(window, &root.editor.focus_handle(cx), cx);
                });
            })
        });
        let universal_root = cx.entity().downgrade();
        let universal_search = cx.new(|cx| {
            QuickSearch::new(cx, move |event, window, cx| {
                let _ = universal_root.update(cx, |root, cx| {
                    root.close_universal_search(window, cx);
                    if let QuickSearchEvent::Execute(command) = event {
                        root.shell
                            .update(cx, |shell, cx| shell.execute_command(command, window, cx));
                    }
                });
            })
        });
        let views = HashMap::<ViewId, AnyView>::from([
            (id::VIEW_FILES.into(), files.clone().into()),
            (id::VIEW_GIT.into(), git.clone().into()),
            (id::VIEW_CHANGES.into(), changes.into()),
            (id::VIEW_ASSISTANT.into(), tasks.clone().into()),
            (id::VIEW_TERMINAL.into(), terminal.clone().into()),
            (id::VIEW_PROBLEMS.into(), problems.clone().into()),
            (id::VIEW_RESOURCES.into(), resources.clone().into()),
            (id::VIEW_SEARCH.into(), search.clone().into()),
            (id::VIEW_PROJECT_TASKS.into(), conversation.clone().into()),
            (id::VIEW_PREVIEW.into(), preview.clone().into()),
            (id::VIEW_SETTINGS.into(), settings.clone().into()),
            (id::VIEW_COMMAND_SEARCH.into(), quick_search.clone().into()),
        ]);
        let shell = cx.new(|cx| Shell::new(editor.clone().into(), views, cx));
        let branch_picker = cx.new(|cx| features::git::BranchPicker::new(git.clone(), window, cx));
        shell.update(cx, |shell, cx| {
            shell.branch_picker = Some(branch_picker);
            shell.set_locale(initial_locale.clone(), cx);
            shell.open_view_in_zone(ZonePosition::Left, id::VIEW_PROJECT_TASKS.into(), cx);
            if initial.show_tasks {
                shell.open_view_in_zone(ZonePosition::Right, id::VIEW_ASSISTANT.into(), cx);
            }
            if initial.show_terminal || initial.show_problems {
                shell.open_view_in_zone(
                    ZonePosition::Bottom,
                    if initial.show_problems {
                        id::VIEW_PROBLEMS
                    } else {
                        id::VIEW_TERMINAL
                    }
                    .into(),
                    cx,
                );
            }
            #[cfg(target_family = "wasm")]
            if let Some(view) = desktop_client::web_host::restore_view() {
                shell.restore_view(view, cx);
            }
        });
        if let Some(id) = shell.read(cx).active_terminal_resource() {
            terminal.update(cx, |terminal, cx| terminal.show_resource(id, cx));
        }
        // 起動時からターミナルタブが見えていれば、既定 profile の自動起動を促す。
        terminal.update(cx, |terminal, cx| {
            let visible = shell.read(cx).shell.terminal_tab_visible();
            terminal.sync_tab_presence(visible, cx);
        });
        // Resource Manager は表示中だけ取得する。初期 Layout の表示状態を反映する。
        resources.update(cx, |resources, cx| {
            let visible = shell
                .read(cx)
                .shell
                .view_visible_in_layout(id::VIEW_RESOURCES);
            resources.set_visible(visible, cx);
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
                    return;
                }
                // スリープやバックグラウンドから戻ったら snapshot を取り直して再同期する。
                root.resync_remote_session_view(cx);
                root.sync_session_view_stale(cx);
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
                |root, _, values: &settings::ResolvedSettings, cx| {
                    let locale = values.locale();
                    root.shell
                        .update(cx, |shell, cx| shell.set_locale(locale.clone(), cx));
                    root.group.update(cx, |group, cx| {
                        group.locale = locale;
                        cx.notify();
                    });
                    cx.refresh_windows();
                },
            ),
            cx.subscribe(&terminal, |root, _, event: &TerminalFeatureEvent, cx| {
                let id = match event {
                    TerminalFeatureEvent::Started(id) | TerminalFeatureEvent::Selected(id) => {
                        id.clone()
                    }
                };
                root.activate_terminal(id, cx);
                root.schedule_persist(cx);
            }),
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
                        shell.open_view_in_zone(
                            ZonePosition::Right,
                            id::VIEW_COMMAND_SEARCH.into(),
                            cx,
                        )
                    });
                    crate::gpui_compat::focus(
                        window,
                        &root.quick_search.read(cx).focus_handle(),
                        cx,
                    );
                }
            }),
            cx.subscribe_in(&editor, window, |root, _, event, window, cx| match event {
                EditorEvent::Snapshot(snapshot) => {
                    root.group
                        .update(cx, |group, cx| group.publish(*snapshot.clone(), cx));
                    root.synchronize_group(cx);
                    #[cfg(not(target_family = "wasm"))]
                    if root.native_smoke_requested {
                        root.native_smoke_requested = false;
                        crate::window_host::run_native_smoke(root, window, cx);
                    }
                    #[cfg(not(target_family = "wasm"))]
                    root.apply_remote_workbench(cx);
                }
                EditorEvent::Idle => {
                    root.synchronize_group(cx);
                    #[cfg(not(target_family = "wasm"))]
                    root.apply_remote_workbench(cx);
                    root.apply_remote_session_view(cx);
                    root.sync_session_view_stale(cx);
                    // Editor の処理中に見送った再同期を、Idle で実行する。
                    if root.session_view_resync_pending {
                        root.resync_remote_session_view(cx);
                    }
                }
                EditorEvent::Status(message) => {
                    root.set_status(message.clone(), cx);
                    #[cfg(not(target_family = "wasm"))]
                    root.apply_remote_workbench(cx);
                    root.apply_remote_session_view(cx);
                    root.sync_session_view_stale(cx);
                }
                EditorEvent::WorkspaceSearch(query) => {
                    root.search
                        .update(cx, |search, cx| search.start(query.clone(), cx));
                    root.shell.update(cx, |shell, cx| {
                        shell.open_view_in_zone(ZonePosition::Left, id::VIEW_SEARCH.into(), cx)
                    });
                }
                EditorEvent::Completion(definition) => {
                    let (line, column) = root.editor.read(cx).cursor_line_column();
                    root.problems.update(cx, |problems, cx| {
                        problems.request(
                            *definition,
                            lsp::LspPosition {
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
                // Layout のタブ参照を Terminal の固定参照へ反映し、
                // ターミナルタブが見えたら既定 profile の自動起動を1回促す。
                root.sync_terminal_tab(cx);
                root.sync_resource_manager_visibility(cx);
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
            if let Some(handle) = &root.session_view {
                // 終了時に購読を解除し、Host 側に残った参加者を残さない。
                #[cfg(not(target_family = "wasm"))]
                handle.close();
                #[cfg(target_family = "wasm")]
                {
                    let _ = handle;
                    desktop_client::web_host::close_session_view();
                }
            }
            let worker = root.worker.clone();
            let receiver = worker.request(move |client| client.persist_view(view));
            async move {
                let _worker = worker;
                loop {
                    match receiver.try_recv() {
                        Ok(result) => {
                            if let Err(error) = result {
                                tracing::error!(target: "lapis::root", "終了時の保存に失敗しました: {error}");
                            }
                            break;
                        }
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
                        Err(std::sync::mpsc::TryRecvError::Empty) => {
                            crate::timer::Timer::after(std::time::Duration::from_millis(16)).await;
                        }
                    }
                }
            }
        }));
        crate::gpui_compat::focus(window, &editor.focus_handle(cx), cx);
        #[cfg(not(target_family = "wasm"))]
        if let Some(watch) = workbench_watch.clone() {
            cx.spawn(async move |root, cx| {
                loop {
                    // 通知の取りこぼしを避けるため、確認より先に購読を登録する。
                    let receiver = watch.subscribe();
                    if root
                        .update(cx, |root, cx| root.apply_remote_workbench(cx))
                        .is_err()
                    {
                        return;
                    }
                    let notified = cx
                        .background_executor()
                        .spawn(async move { receiver.recv().is_ok() });
                    if !notified.await {
                        return;
                    }
                }
            })
            .detach();
        }
        // 他Clientが共有したタブ構成を、Workbenchの変更と同じくworker経由で適用する。
        #[cfg(not(target_family = "wasm"))]
        if let Some(watch) = session_view_watch.clone() {
            cx.spawn(async move |root, cx| {
                loop {
                    // 通知の取りこぼしを避けるため、確認より先に購読を登録する。
                    let receiver = watch.subscribe();
                    if root
                        .update(cx, |root, cx| root.apply_remote_session_view(cx))
                        .is_err()
                    {
                        return;
                    }
                    let notified = cx
                        .background_executor()
                        .spawn(async move { receiver.recv().is_ok() });
                    if !notified.await {
                        return;
                    }
                }
            })
            .detach();
        }
        // Web は接続の event pump から通知を受ける。watch の blocking 受信は使わない。
        #[cfg(target_family = "wasm")]
        if session_view_watch.is_some()
            && let Some(mut changes) = desktop_client::web_host::session_view_changes()
        {
            cx.spawn(async move |root, cx| {
                use futures_util::StreamExt;
                while changes.next().await.is_some() {
                    if root
                        .update(cx, |root, cx| {
                            root.apply_remote_session_view(cx);
                            root.sync_session_view_stale(cx);
                        })
                        .is_err()
                    {
                        return;
                    }
                }
            })
            .detach();
        }
        // 起動時に復元した Workspace は、最初の通知を待たずに購読を始める。
        if session_view_watch.is_some() {
            let weak = cx.entity().downgrade();
            cx.defer(move |cx| {
                let _ = weak.update(cx, |root, cx| root.apply_remote_session_view(cx));
            });
        }
        Self {
            group,
            shell,
            editor,
            files,
            git,
            tasks,
            terminal,
            problems,
            resources,
            search,
            conversation,
            preview,
            preview_revision: None,
            line_ending_cache: None,
            settings,
            quick_search,
            universal_search,
            universal_search_open: false,
            universal_previous_focus: None,
            path_request: None,
            git_input: None,
            _subscriptions: subscriptions,
            double_shift: DoubleShiftDetector::default(),
            worker,
            factory,
            persist_task: None,
            persist_signature: None,
            workbench_publish_signature: None,
            session_view,
            session_view_applying: false,
            session_view_pending: false,
            session_view_task: None,
            session_view_stale: false,
            session_view_resync_pending: false,
            #[cfg(not(target_family = "wasm"))]
            native_smoke_requested: initial.native_window_smoke,
            #[cfg(not(target_family = "wasm"))]
            workbench_watch,
            #[cfg(not(target_family = "wasm"))]
            workbench_revision,
            workbench: WorkbenchState::default(),
            task_context: None,
            task_context_gate: Default::default(),
            task_context_resolution: Default::default(),
            connection,
            pending_initial_task,
            window_title: None,
            task_url_initialized: false,
            last_task_for_url: None,
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
        // URL の fragment から渡された Task は、初回 snapshot で存在を確認してから選ぶ。
        if let Some(task) = self.pending_initial_task.take()
            && let Some(record) = snapshot
                .conversations
                .iter()
                .find(|record| record.id == task)
        {
            if self.workbench.mode() == WorkbenchMode::Ide {
                self.workbench
                    .select_task(task.clone(), record.view.clone());
            } else {
                let root = cx.entity().downgrade();
                cx.defer(move |cx| {
                    let _ = root.update(cx, |root, cx| root.select_task(task.clone(), cx));
                });
            }
        }
        self.files
            .update(cx, |files, cx| files.update(snapshot, cx));
        self.resources.update(cx, |resources, cx| {
            resources.set_workspace(snapshot.workspace.clone(), cx)
        });
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
        if self.task_url_initialized && self.last_task_for_url.as_ref() != Some(&selected) {
            self.notify_task_url(&selected);
        }
        self.last_task_for_url = Some(selected.clone());
        self.task_url_initialized = true;
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
            settings.set_workspace(
                snapshot.root.clone(),
                snapshot
                    .workspace
                    .as_ref()
                    .map(|workspace| workspace.as_str().to_owned()),
                cx,
            )
        });
        let terminal_key =
            ViewInstanceKey::new(crate::shell::PanelTab::tool(id::VIEW_TERMINAL).stable_view_key())
                .unwrap();
        let terminal_scope = self.workbench.view_scope(&terminal_key);
        let resource = match terminal_scope {
            Some(ViewScope::Resource(resource)) => {
                Some(terminal::TerminalId::new(resource.clone()))
            }
            _ => None,
        };
        let terminal_target = terminal_context_target(
            terminal_scope,
            &selected,
            self.terminal.read(cx).context_task(),
            &snapshot.conversations,
        );
        // Resource参照タブが表示中なら、そのHost Terminalを優先して再接続する。
        let resource = self.shell.read(cx).active_terminal_resource().or(resource);
        self.terminal.update(cx, |terminal, cx| {
            terminal.bind_resource(resource);
            if let Some(target) = terminal_target {
                terminal.set_context(
                    target
                        .record
                        .and_then(|record| record.workspace.root.clone()),
                    target.task,
                    target
                        .record
                        .map(|record| record.terminals.clone())
                        .unwrap_or_default(),
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
        let workspace_name = self
            .connection
            .as_ref()
            .and_then(|connection| connection.workspace_name.clone())
            .unwrap_or_else(|| snapshot.workspace_name.clone());
        let presentation = ShellPresentation {
            documents: snapshot.tabs.clone(),
            active: snapshot.active.clone(),
            root: snapshot.root.clone(),
            workspace_name,
            workspace_id: snapshot
                .workspace
                .as_ref()
                .map(|workspace| workspace.as_str().to_owned()),
            connection: self.connection.clone(),
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
        self.sync_terminal_tab(cx);
        self.sync_resource_manager_visibility(cx);
        self.sync_preview(cx);
    }

    fn sync_task_context(
        &mut self,
        snapshot: &ClientSnapshot,
        selected: &editor_core::ConversationId,
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

    /// Terminalタブの参照先をShellとTerminal Featureへ同時に反映する。
    fn activate_terminal(&mut self, id: terminal::TerminalId, cx: &mut Context<Self>) {
        self.shell
            .update(cx, |shell, cx| shell.open_terminal_tab(id.clone(), cx));
        self.terminal
            .update(cx, |terminal, cx| terminal.show_resource(id, cx));
        self.sync_terminal_auto_start(cx);
        cx.notify();
    }

    /// ターミナルタブが見えているとき、既定 profile の自動起動を1回だけ促す。
    ///
    /// Feature 側で冪等なので、タブが表示されるたびに呼んでよい。
    fn sync_terminal_auto_start(&mut self, cx: &mut Context<Self>) {
        let visible = self.shell.read(cx).shell.terminal_tab_visible();
        self.terminal
            .update(cx, |terminal, cx| terminal.sync_tab_presence(visible, cx));
    }

    /// Resource Manager パネルの表示状態を Feature へ伝える。非表示の間は取得を止める。
    fn sync_resource_manager_visibility(&mut self, cx: &mut Context<Self>) {
        let visible = self
            .shell
            .read(cx)
            .shell
            .view_visible_in_layout(id::VIEW_RESOURCES);
        self.resources
            .update(cx, |resources, cx| resources.set_visible(visible, cx));
    }

    /// 復元したLayoutのうち、表示中のTerminalタブをHostの既存PTYへ再接続する。
    fn sync_terminal_tab(&mut self, cx: &mut Context<Self>) {
        let shell = self.shell.read(cx);
        let update = terminal_binding_update(
            shell.active_terminal_resource(),
            shell.shell.terminal_tab_visible(),
        );
        match update {
            TerminalBindingUpdate::Show(id) => self
                .terminal
                .update(cx, |terminal, cx| terminal.show_resource(id, cx)),
            // 復元した Layout に参照先が無いため、前の Task の固定参照を解除する。
            // 残すと現在の Task に無い Terminal ID を参照し続け、既存セッションの
            // 表示も既定 profile の自動起動も妨げる。
            TerminalBindingUpdate::Unbind => self
                .terminal
                .update(cx, |terminal, _| terminal.release_resource()),
            // タブが画面に出ていないだけなら、PTY を維持するため参照を残す。
            TerminalBindingUpdate::Keep => {}
        }
        self.sync_terminal_auto_start(cx);
    }

    /// HostのWorkbench変更を、未確定の入力を待ってからUIへ反映する。
    #[cfg(not(target_family = "wasm"))]
    fn apply_remote_workbench(&mut self, cx: &mut Context<Self>) {
        let Some(watch) = self.workbench_watch.clone() else {
            return;
        };
        let Some((revision, view)) = watch.take_if_changed(self.workbench_revision) else {
            return;
        };
        if self.editor.read(cx).has_in_flight() {
            // 完了時のIdleで再試行する。revisionは進めず変更を保持する。
            return;
        }
        self.workbench_revision = revision;
        let Some(shared) = view.workspace.clone() else {
            return;
        };
        let layout = desktop_client::ConversationViewState {
            workspace: None,
            ..view
        };
        // 適用したHostの表示をそのまま送り返さない。
        self.workbench_publish_signature = Some(workbench_publish_view(&layout));
        self.shell
            .update(cx, |shell, cx| shell.restore_view(layout, cx));
        self.sync_terminal_tab(cx);
        let worker = self.worker.clone();
        let _ = worker.submit(
            cx,
            move |client| client.restore_workbench(shared),
            |root, result, cx| {
                if let Ok(snapshot) = result {
                    root.apply_snapshot(&snapshot, cx);
                }
                // 反映中に届いた次の変更を、完了後すぐに適用する。
                root.apply_remote_workbench(cx);
            },
        );
    }

    /// 他Clientが共有したタブ構成をUIへ反映する。
    ///
    /// 未確定の入力がある間は適用せず、Idleで再試行する。適用した値はそのまま
    /// 送り返さない。適用中に届いた次の変更は、反映完了後にもう一度適用する。
    fn apply_remote_session_view(&mut self, cx: &mut Context<Self>) {
        if self.session_view.is_none() {
            return;
        }
        if self.session_view_applying {
            // 反映中の通知は捨てずに記録し、完了後にもう一度確認する。
            self.session_view_pending = true;
            return;
        }
        if self.editor.read(cx).has_in_flight() {
            return;
        }
        self.session_view_applying = true;
        let worker = self.worker.clone();
        let submitted = worker.submit(
            cx,
            |client| client.apply_session_view(),
            |root, result, cx| {
                root.session_view_applying = false;
                let applied = match result {
                    Ok(Some(snapshot)) => {
                        root.apply_snapshot(&snapshot, cx);
                        true
                    }
                    Ok(None) => false,
                    Err(error) => {
                        eprintln!("Workspace view を反映できません: {error}");
                        false
                    }
                };
                // 反映した時と、反映中に見送った通知がある時だけ再確認する。
                // 変化が無いのに再投入すると、worker への送信が終わらない背景ループになる。
                let pending = std::mem::take(&mut root.session_view_pending);
                if applied || pending {
                    root.apply_remote_session_view(cx);
                }
            },
        );
        if submitted.is_err() {
            self.session_view_applying = false;
        }
    }

    /// 自分のタブ開閉とアクティブ文書を、短い debounce で共有へ送る。
    ///
    /// 保存の debounce とは分離し、タブ操作が続く間は最後の状態だけを送る。
    /// 再同期の要求より後にだけ送り、共有済みの状態を古い値で上書きしない。
    /// タブ構成とアクティブ文書が変わっていない編集では待機を延長しない。
    pub(crate) fn schedule_session_view_publish(&mut self, cx: &mut Context<Self>) {
        if self.session_view.is_none()
            || self.group.read(cx).closing
            || self.group.read(cx).synchronization_required
            || self.task_context_resolution == TaskContextResolution::Pending
            || self.editor.read(cx).has_in_flight()
        {
            return;
        }
        // 既に同じ構成の送信待ちがあるなら、カーソル移動や入力で待機を延ばさない。
        // 送信自体は worker 側で直前の構成と比較して省略される。
        if self.session_view_task.is_some() {
            return;
        }
        self.session_view_task = Some(cx.spawn(async move |this, cx| {
            crate::timer::Timer::after(std::time::Duration::from_millis(150)).await;
            let _ = this.update(cx, |root, cx| {
                // 見送る場合も待機印を消す。残すと以後の予約がすべて止まる。
                root.session_view_task = None;
                if root.group.read(cx).closing
                    || root.group.read(cx).synchronization_required
                    || root.task_context_resolution == TaskContextResolution::Pending
                    || root.editor.read(cx).has_in_flight()
                {
                    return;
                }
                let worker = root.worker.clone();
                let _ = worker.submit(
                    cx,
                    |client| client.publish_session_view(),
                    |_, result, _| {
                        // 失敗したら署名を確定させず、次の変更で再送させる。
                        if let Err(error) = result {
                            eprintln!("Workspace view を共有できません: {error}");
                        }
                    },
                );
            });
        }));
    }

    /// バックグラウンド復帰や再接続で snapshot を取り直し、取りこぼしを再同期する。
    ///
    /// 継続中の debounce があれば取り消し、再同期した snapshot より先に古い値を
    /// 送らないようにする。Editor の処理中は保留し、Idle で実行する。
    fn resync_remote_session_view(&mut self, cx: &mut Context<Self>) {
        if self.session_view.is_none() || self.group.read(cx).closing {
            return;
        }
        if self.editor.read(cx).has_in_flight() {
            self.session_view_resync_pending = true;
            return;
        }
        self.session_view_resync_pending = false;
        self.session_view_task.take();
        let worker = self.worker.clone();
        let _ = worker.submit(
            cx,
            |client| client.resync_session_view(),
            |root, result, cx| match result {
                Ok(Some(snapshot)) => root.apply_snapshot(&snapshot, cx),
                Ok(None) => {}
                Err(error) => eprintln!("Workspace view を再同期できません: {error}"),
            },
        );
    }

    /// 共有 view が古い可能性を表示へ反映する。状態が変わった時だけ再描画する。
    fn sync_session_view_stale(&mut self, cx: &mut Context<Self>) {
        // Workspace を閉じた後は共有対象が無いため、古い表示の警告を出さない。
        let stale = self.shell.read(cx).session.workspace_id.is_some()
            && self
                .session_view
                .as_ref()
                .is_some_and(|handle| session_view_watch_of(handle).is_stale());
        if self.session_view_stale == stale {
            return;
        }
        self.session_view_stale = stale;
        let message = if stale {
            "共有中のタブ構成は最新ではない可能性があります"
        } else {
            ""
        };
        // 通常の status とは別枠で出す。他のメッセージを上書きしない。
        self.shell
            .update(cx, |shell, cx| shell.set_warning(message.to_owned(), cx));
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
                            if let Some(path) = client.choose_workspace_path() {
                                client
                                    .conversation
                                    .register_project(&mut client.editor, path)
                                    .map_err(|error| error.to_string())?;
                            }
                            Ok(())
                        }
                        ConversationEvent::RegisterSource(project) => {
                            if let Some(path) = client.choose_workspace_path() {
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
    fn select_task(&mut self, id: editor_core::ConversationId, cx: &mut Context<Self>) {
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
            self.notify_task_url(&id);
            self.workbench.select_task(id, record.view.clone());
            self.synchronize_group(cx);
            return;
        }
        if self.workbench.context_task() == Some(&id) {
            self.notify_task_url(&id);
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

    fn notify_task_url(&mut self, id: &editor_core::ConversationId) {
        self.last_task_for_url = Some(id.clone());
        #[cfg(target_family = "wasm")]
        crate::web::notify_task_selected(id.as_str());
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
        view: desktop_client::ConversationViewState,
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

    fn execution_for_persistence(&self, cx: &gpui::App) -> Option<editor_core::ExecutionId> {
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
    ) -> Option<editor_core::ConversationId> {
        let terminal = self.terminal.read(cx);
        let record = self
            .conversation
            .read(cx)
            .records
            .iter()
            .find(|record| &record.id == terminal.context_task())?;
        terminal.persistence_owner_for(&record.id, record.workspace.root.as_deref())
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
        // 表示の反映は500msの保存debounceから分離し、変更があれば同じフレームで通知する。
        let workbench = workbench_publish_view(&layout);
        if self.workbench_publish_signature.as_ref() != Some(&workbench)
            && self.workbench.context_task().is_some()
            && !self.group.read(cx).closing
            && !self.group.read(cx).synchronization_required
            && self.task_context_resolution != TaskContextResolution::Pending
        {
            self.workbench_publish_signature = Some(workbench.clone());
            let published = self.worker.submit(
                cx,
                move |client| client.publish_workbench(workbench),
                |root, result, _| {
                    if let Err(error) = result {
                        root.workbench_publish_signature = None;
                        eprintln!("WorkbenchをHostへ通知できません: {error}");
                    }
                },
            );
            if published.is_err() {
                self.workbench_publish_signature = None;
            }
        }
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
        self.schedule_session_view_publish(cx);
        let terminal = self.terminal.read(cx);
        let owner = self.terminal_owner_for_persistence(cx);
        let summaries = if owner.is_some() {
            terminal.persistence_summaries()
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
            crate::timer::Timer::after(std::time::Duration::from_millis(500)).await;
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
                            let title = root.shell.read(cx).t("notification.error.persistence");
                            crate::notification::publish_error(
                                cx,
                                "workspace.persistence_failed",
                                &title,
                                error,
                            );
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

    /// 全体検索を閉じ、以前のフォーカスへ戻す。
    fn close_universal_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.universal_search_open {
            self.universal_search_open = false;
            if let Some(focus) = self.universal_previous_focus.take() {
                crate::gpui_compat::focus(window, &focus, cx);
            }
            cx.notify();
        }
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
            shell.open_view_in_zone(ZonePosition::Right, id::VIEW_COMMAND_SEARCH.into(), cx)
        });
        crate::gpui_compat::focus(window, &self.quick_search.read(cx).focus_handle(), cx);
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
                            client.close_session_view();
                            return client
                                .conversation
                                .close_workspace(&mut client.editor)
                                .map_err(|error| error.to_string());
                        }
                        let path = match command {
                            UiCommand::OpenRecent(path) => Some(path),
                            _ => client.choose_workspace_path(),
                        };
                        if let Some(path) = path {
                            client
                                .conversation
                                .open_project(&mut client.editor, path)
                                .map_err(|error| error.to_string())?;
                            // Workspace を開いた直後に共有 snapshot を取り込み、
                            // 同じ Workspace を開いている別 Client のタブ構成へ合わせる。
                            if let Err(error) = client.apply_session_view() {
                                eprintln!("Workspace view を取得できません: {error}");
                            }
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
            command @ (UiCommand::MoveTab(_, _)
            | UiCommand::CloseTabs(_)
            | UiCommand::ReleaseTab(_, _)) => self.host_command(0, command, window, cx),
            UiCommand::QuickSearch => {
                self.double_shift.reset();
                let locale = self.group.read(cx).locale.clone();
                let mut localizer = crate::localization::Localizer::bundled();
                localizer.set_active(&locale);
                let provider = CommandSearchProvider::from_registry(
                    &features::bundled_registry(),
                    &localizer,
                    crate::keymap::KeymapRegistry::active(cx),
                );
                if !self.universal_search_open {
                    self.universal_previous_focus = window.focused(cx);
                }
                self.universal_search_open = true;
                self.universal_search.update(cx, |search, cx| {
                    search.open_universal(provider, &locale, cx)
                });
                crate::gpui_compat::focus(
                    window,
                    &self.universal_search.read(cx).focus_handle(),
                    cx,
                );
                cx.notify();
            }
            UiCommand::ActivateDocument(id) => {
                self.editor.update(cx, |editor, cx| editor.activate(id, cx));
                crate::gpui_compat::focus(window, &self.editor.focus_handle(cx), cx);
            }
            UiCommand::ActivateTerminal(id) => {
                self.activate_terminal(id, cx);
                self.schedule_persist(cx);
            }
            UiCommand::TerminalTabClosed => {
                // 別のターミナルタブが残っていれば、その表示に任せる。
                if !self.shell.read(cx).shell.terminal_tab_exists() {
                    self.terminal
                        .update(cx, |terminal, cx| terminal.close_session(cx));
                    self.schedule_persist(cx);
                }
            }
            UiCommand::CloseDocument(id) => self.close_hosts(0, Some(id), window, cx),
            UiCommand::SelectTask(id) => self.select_task(id, cx),
            UiCommand::SetWorkbenchMode(mode) => self.set_workbench_mode(mode, cx),
            UiCommand::SetViewBinding { view, scope } => self.set_view_binding(view, scope, cx),
            UiCommand::BindViewToSelectedTask(view) => self.bind_view_to_selected_task(view, cx),
            UiCommand::AcceptRemoteWorkbench => {
                let admitted = self.worker.submit(
                    cx,
                    |client| {
                        client
                            .conversation
                            .accept_workbench_remote()
                            .map_err(|error| error.to_string())
                    },
                    |root, result, cx| match result {
                        Ok(Some(view)) => {
                            root.restore_workbench_layout(view, cx);
                            root.set_status("HostのWorkbench状態を採用しました".into(), cx);
                        }
                        Ok(None) => {
                            root.set_status("解決するWorkbenchの競合はありません".into(), cx)
                        }
                        Err(error) => root.set_status(error, cx),
                    },
                );
                if let Err(error) = admitted {
                    self.set_status(error, cx);
                }
            }
            UiCommand::RetryLocalWorkbench => {
                let admitted = self.worker.submit(
                    cx,
                    |client| {
                        client
                            .conversation
                            .retry_workbench_local()
                            .map_err(|error| error.to_string())
                    },
                    |root, result, cx| match result {
                        Ok(()) => root.set_status("Workbenchの変更を再送しました".into(), cx),
                        Err(error) => root.set_status(error, cx),
                    },
                );
                if let Err(error) = admitted {
                    self.set_status(error, cx);
                }
            }
            UiCommand::Undo => self
                .editor
                .update(cx, |editor, cx| editor.undo(&crate::app::Undo, window, cx)),
            UiCommand::Redo => self
                .editor
                .update(cx, |editor, cx| editor.redo(&crate::app::Redo, window, cx)),
            UiCommand::Find => self
                .editor
                .update(cx, |editor, cx| editor.find(&crate::app::Find, window, cx)),
            UiCommand::FindNext => self.editor.update(cx, |editor, cx| {
                editor.find_next(&crate::app::FindNext, window, cx)
            }),
            UiCommand::FindWorkspace => self.editor.update(cx, |editor, cx| {
                editor.find_workspace(&crate::app::FindWorkspace, window, cx)
            }),
            UiCommand::Complete => self.editor.update(cx, |editor, cx| {
                editor.complete(&crate::app::Complete, window, cx)
            }),
            UiCommand::GoToDefinition => self.editor.update(cx, |editor, cx| {
                editor.go_to_definition(&crate::app::GoToDefinition, window, cx)
            }),
            UiCommand::Quit => self.close_application(window, cx),
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

            UiCommand::SwitchWorkspace(workspace) => {
                #[cfg(target_family = "wasm")]
                {
                    let Some(handle) = window.window_handle().downcast::<Self>() else {
                        return;
                    };
                    self.set_status("Workspaceを切り替えています…".into(), cx);
                    cx.spawn_in(window, async move |_, cx| {
                        match desktop_client::web_host::switch_workspace(&workspace).await {
                            Ok(()) => {
                                // ブラウザーのWindowは閉じると再生成できないため、
                                // 同じWindowのRootを新しいWorkspaceのSessionで組み直す。
                                let outcome = handle.update(cx, |root, window, cx| {
                                    let (session, services) = crate::web::host_sessions()?;
                                    *root = DesktopRoot::new(
                                        session,
                                        services,
                                        InitialView::default(),
                                        window,
                                        cx,
                                    );
                                    Ok::<_, String>(())
                                });
                                let error = match outcome {
                                    Ok(Ok(())) => None,
                                    Ok(Err(error)) => Some(error),
                                    Err(error) => Some(error.to_string()),
                                };
                                match error {
                                    // 画面の再構築が成功してから URL と保存済み選択を追従させる。
                                    None => {
                                        desktop_client::web_host::finish_workspace_switch();
                                        if let Some(info) =
                                            desktop_client::web_host::connection_info()
                                        {
                                            crate::web::notify_workspace_switched(&info);
                                        }
                                    }
                                    Some(error) => {
                                        // 画面を作れない場合は、attachmentと複製を元へ戻す。
                                        match desktop_client::web_host::rollback_workspace_switch()
                                            .await
                                        {
                                            Ok(()) => {
                                                let _ = handle.update(cx, |root, _, cx| {
                                                    root.set_status(
                                                        format!(
                                                            "Workspaceを開けません: {error}（元のWorkspaceへ戻しました）"
                                                        ),
                                                        cx,
                                                    );
                                                });
                                            }
                                            Err(recovery) => {
                                                web_sys::console::error_1(
                                                    &format!(
                                                        "Workspace切替後のUIを組み立てられず、元のWorkspaceへも戻せません: {error} / {recovery}"
                                                    )
                                                    .into(),
                                                );
                                                if let Some(window) = web_sys::window() {
                                                    let _ = window.location().reload();
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            Err(error) => {
                                let _ = handle.update(cx, |root, _, cx| {
                                    root.set_status(
                                        format!("Workspaceを切り替えられません: {error}"),
                                        cx,
                                    );
                                });
                            }
                        }
                    })
                    .detach();
                }
                #[cfg(not(target_family = "wasm"))]
                let _ = workspace;
            }
            UiCommand::ConnectHost => {
                let factory = self.factory.clone();
                let admitted = self.worker.submit(
                    cx,
                    move |_| factory.open_host(),
                    |root, result, cx| {
                        if let Err(error) = result {
                            let title = root.shell.read(cx).t("notification.error.host-launch");
                            crate::notification::publish_error(
                                cx,
                                "host.connection_launcher_failed",
                                &title,
                                error,
                            );
                        }
                    },
                );
                if let Err(error) = admitted {
                    let title = self.shell.read(cx).t("notification.error.host-launch");
                    crate::notification::publish_error(
                        cx,
                        "host.connection_launcher_failed",
                        &title,
                        error,
                    );
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
                        Err(error) => {
                            let title = root.shell.read(cx).t("notification.error.window");
                            crate::notification::publish_error(
                                cx,
                                "window.creation_failed",
                                &title,
                                error,
                            );
                        }
                    },
                );
                if let Err(error) = admitted {
                    let title = self.shell.read(cx).t("notification.error.window");
                    crate::notification::publish_error(cx, "window.creation_failed", &title, error);
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title = {
            let session = &self.shell.read(cx).session;
            desktop_client::window_title(
                Some(session.workspace_name.as_str()),
                session.workspace_id.as_deref(),
            )
        };
        if self.window_title.as_deref() != Some(title.as_str()) {
            window.set_window_title(&title);
            self.window_title = Some(title);
        }
        let panel_height = gpui::px(520.)
            .min((window.viewport_size().height - gpui::px(100.)).max(gpui::px(100.)));
        div()
            .size_full()
            .relative()
            .on_action(cx.listener(|root, _: &crate::app::Quit, window, cx| {
                cx.stop_propagation();
                root.close_application(window, cx);
            }))
            .on_modifiers_changed(cx.listener(
                |root, event: &gpui::ModifiersChangedEvent, window, cx| {
                    if root
                        .double_shift
                        .modifiers_changed(event.modifiers, crate::time::Instant::now())
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
            .when(self.universal_search_open, |root| {
                root.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .justify_center()
                        .items_start()
                        .pt(gpui::px(72.))
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                this.close_universal_search(window, cx);
                            }),
                        )
                        .child(
                            crate::components::floating_panel(
                                "universal-search",
                                crate::components::SurfaceVariant::Popover,
                                cx,
                            )
                            .debug_selector(|| "universal-search-popup".into())
                            .w(gpui::relative(0.9))
                            .max_w(gpui::px(820.))
                            .h(panel_height)
                            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation()
                            })
                            .child(self.universal_search.clone()),
                        ),
                )
            })
    }
}

struct TerminalContextTarget<'a> {
    task: editor_core::ConversationId,
    record: Option<&'a desktop_client::ConversationRecord>,
}

/// 表示中の Terminal タブへ反映する固定参照の更新。
enum TerminalBindingUpdate {
    /// 参照先があるので、その PTY へ再アタッチする。
    Show(terminal::TerminalId),
    /// 参照先の無いタブが表示中なので、前の Task の固定参照を解除する。
    Unbind,
    /// Terminal タブが表示されていないので、固定参照をそのまま残す
    /// （パネルを閉じただけでも PTY は維持する）。
    Keep,
}

fn terminal_binding_update(
    resource: Option<terminal::TerminalId>,
    visible: bool,
) -> TerminalBindingUpdate {
    match resource {
        Some(id) => TerminalBindingUpdate::Show(id),
        None if visible => TerminalBindingUpdate::Unbind,
        None => TerminalBindingUpdate::Keep,
    }
}

/// Resourceの所有元が未確定なら、既存コンテキストへフォールバックしない。
fn terminal_context_target<'a>(
    scope: Option<&ViewScope>,
    selected: &editor_core::ConversationId,
    current: &editor_core::ConversationId,
    records: &'a [desktop_client::ConversationRecord],
) -> Option<TerminalContextTarget<'a>> {
    if let Some(ViewScope::Resource(resource)) = scope {
        return records
            .iter()
            .find(|record| {
                record.terminals.iter().any(|terminal| {
                    terminal
                        .id
                        .as_ref()
                        .is_some_and(|id| id.as_str() == resource)
                })
            })
            .map(|record| TerminalContextTarget {
                task: record.id.clone(),
                record: Some(record),
            });
    }
    let task = match scope {
        Some(ViewScope::Task(task)) => task,
        Some(ViewScope::FollowSelectedTask) => selected,
        _ => current,
    };
    let record = records.iter().find(|record| &record.id == task);
    // 現在のTaskが一時的に見つからない間は保持する。別Taskへの明示変更は
    // 表示先を変更して旧Taskへの入力を止め、参照の保存はレコード確定まで待つ。
    (record.is_some() || task != current).then(|| TerminalContextTarget {
        task: task.clone(),
        record,
    })
}

/// 表示中の選択を保存できるのは、そのViewが保存先Taskを参照している場合だけ。
/// 共有状態として通知するWorkbench部分だけを取り出す。Cursorやdraftは保存経路に任せる。
fn workbench_publish_view(
    layout: &desktop_client::ConversationViewState,
) -> desktop_client::ConversationViewState {
    layout.clone().for_shared_workbench()
}

fn execution_for_context(
    view_task: Option<&editor_core::ConversationId>,
    context_task: Option<&editor_core::ConversationId>,
    view_selection: Option<editor_core::ExecutionId>,
    saved_selection: Option<editor_core::ExecutionId>,
) -> Option<editor_core::ExecutionId> {
    if view_task.is_some() && view_task == context_task {
        view_selection
    } else {
        saved_selection
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desktop_client::ConversationViewState;
    use editor_core::{ConversationId, ExecutionId};

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
        assert!(
            terminal_context_target(Some(&ViewScope::Task(a.clone())), &b, &a, &records).is_none()
        );
        assert!(
            terminal_context_target(Some(&ViewScope::SharedWorkbench), &b, &a, &records).is_none()
        );
        assert_eq!(
            terminal_context_target(Some(&ViewScope::FollowSelectedTask), &b, &a, &records)
                .map(|target| target.task),
            Some(b)
        );
    }

    #[test]
    fn terminal_binding_is_released_only_when_an_unresolved_tab_is_visible() {
        let id = terminal::TerminalId::new("terminal-a");
        assert!(matches!(
            terminal_binding_update(Some(id.clone()), false),
            TerminalBindingUpdate::Show(found) if found == id
        ));
        assert!(matches!(
            terminal_binding_update(Some(id.clone()), true),
            TerminalBindingUpdate::Show(found) if found == id
        ));
        // 参照先の無いタブが表示中なら、前の Task の固定参照を解除する。
        assert!(matches!(
            terminal_binding_update(None, true),
            TerminalBindingUpdate::Unbind
        ));
        // パネルや Zone を閉じただけなら、PTY を維持するため固定参照を残す。
        assert!(matches!(
            terminal_binding_update(None, false),
            TerminalBindingUpdate::Keep
        ));
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
