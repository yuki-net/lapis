use crate::{
    app::{ConnectionIdentity, DesktopServices, InitialView},
    client_adapter::{ClientWorker, DocumentClient},
    commands::UiCommand,
    features::{
        self,
        command_search::{DoubleShiftDetector, QuickSearch, QuickSearchEvent},
        conversation::ConversationFeature,
        editor::{Editor, EditorEvent},
        files::FilesFeature,
        git::{ChangesFeature, GitEvent, GitFeature, GitInputRequest},
        id,
        preview::PreviewFeature,
        resource_manager::{ProcessWorker, ResourceManagerFeature},
        search::SearchFeature,
        settings::SettingsFeature,
        settings_sources::SettingsSources,
        tasks::TasksFeature,
        terminal::TerminalContext,
        update::UpdateFeature,
    },
    shell::main::Shell,
    workbench::{ViewScope, WorkbenchState},
};
use desktop_client::EditorSession;
use extension_ui_contract::{ViewId, ZonePosition};
use gpui::{
    AnyView, AppContext, Context, Entity, Focusable, Render, Subscription, Window, div, prelude::*,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

mod command;
mod conversation;
mod session_view;
mod snapshot;
#[cfg(test)]
mod tests;
mod workbench;

static NEXT_THEME_SOURCE: AtomicU64 = AtomicU64::new(1);

/// 有効設定ではなく、現在実行中で有効な表示宣言があるかを使う。
fn extension_view_available(
    snapshot: Option<&desktop_client::ExtensionContributionSnapshot>,
    owner: &str,
    content_type: &str,
) -> bool {
    snapshot.is_some_and(|snapshot| snapshot.has_active_view(owner, content_type))
}

/// 切断中や起動中は直前の表示状態を維持し、確定した目録だけで切り替える。
fn extension_view_hidden(
    snapshot: Option<&desktop_client::ExtensionContributionSnapshot>,
    owner: &str,
    content_type: &str,
) -> Option<bool> {
    let snapshot = snapshot?;
    if snapshot
        .extensions()
        .iter()
        .find(|entry| entry.id == owner)
        .is_some_and(|entry| entry.active.is_none())
    {
        return None;
    }
    Some(!snapshot.has_active_view(owner, content_type))
}

fn contributed_zone(zone: extension_ui_contract::ContributedZone) -> ZonePosition {
    match zone {
        extension_ui_contract::ContributedZone::Main => ZonePosition::Main,
        extension_ui_contract::ContributedZone::Left => ZonePosition::Left,
        extension_ui_contract::ContributedZone::Bottom => ZonePosition::Bottom,
        extension_ui_contract::ContributedZone::Right => ZonePosition::Right,
    }
}

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
    /// Terminal のタブ個体と Session を所有する機能。Composition Root だけが具体型を知る。
    terminal: std::rc::Rc<std::cell::RefCell<features::terminal::TerminalLifecycle>>,
    terminal_view: ViewId,
    pub(crate) editor: Entity<Editor>,
    files: Entity<FilesFeature>,
    git: Entity<GitFeature>,
    tasks: Entity<TasksFeature>,
    resources: Entity<ResourceManagerFeature>,
    search: Entity<SearchFeature>,
    conversation: Entity<ConversationFeature>,
    preview: Entity<PreviewFeature>,
    preview_revision: Option<(editor_core::DocumentId, document::Revision)>,
    line_ending_cache: Option<((editor_core::DocumentId, document::Revision), &'static str)>,
    settings: Entity<SettingsFeature>,
    /// 更新の通知・同意画面。install layout を検出できない実行では持たない。
    update: Option<Entity<UpdateFeature>>,
    quick_search: Entity<QuickSearch>,
    universal_search: Entity<QuickSearch>,
    locale_packs: Vec<localization::LocalePack>,
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
        services: DesktopServices,
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
        let terminal_context = conversation_id.clone();
        let terminal_root = session.workspace_root().map(ToOwned::to_owned);
        let initial_locale = services.settings.settings().locale();
        let document_connection = Some(session.document_connection());
        let initial_workspace = session.workspace_id().cloned();
        #[cfg_attr(target_family = "wasm", allow(unused_mut))]
        let mut document_client = DocumentClient::new(
            session,
            services.conversation,
            services.git.backend(),
            session_view.clone(),
        );
        #[cfg(not(target_family = "wasm"))]
        let extension_contributions = services
            .extension_loader
            .as_ref()
            .map(|_| services.settings.extension_contributions());
        #[cfg(not(target_family = "wasm"))]
        if let (Some(loader), Some(contributions)) = (
            services.extension_loader.clone(),
            extension_contributions.clone(),
        ) {
            let mut loading =
                crate::extension_loading::ExtensionLoading::new(contributions, loader);
            loading.set_workspace(initial_workspace.clone());
            document_client.extension_loading = Some(loading);
        }
        let worker = ClientWorker::new(document_client);
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
        let resources =
            cx.new(|_| ResourceManagerFeature::new(process_worker, initial_workspace.clone()));
        let search = cx.new(|cx| SearchFeature::new(services.search, cx));
        let conversation = cx.new(|_| ConversationFeature::new());
        let preview = cx.new(|_| PreviewFeature::new());
        // 設定画面より先に供給元の置き場を用意する。Settings はここだけを購読する。
        SettingsSources::init(cx);
        let settings = cx.new(|cx| {
            SettingsFeature::with_web_access(
                cx,
                services.web_access.clone(),
                services.web_access_control.clone(),
            )
        });
        settings.update(cx, |settings, _| {
            settings.set_legacy_terminal_settings_hidden(false)
        });
        #[cfg(target_family = "wasm")]
        let extension_contributions = desktop_client::web_host::extension_contributions();
        let locale_contributions = extension_contributions.clone();
        settings.update(cx, |settings, cx| {
            settings.set_extension_contributions(extension_contributions, cx)
        });
        #[cfg(not(target_family = "wasm"))]
        settings.update(cx, |settings, _| {
            settings.set_extension_manager(services.extension_manager.clone());
            settings
                .set_extension_capability_manager(services.extension_capability_manager.clone());
        });
        let update = services
            .updater
            .clone()
            .map(|updater| cx.new(|cx| UpdateFeature::new(updater, cx)));
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
            (id::VIEW_RESOURCES.into(), resources.clone().into()),
            (id::VIEW_SEARCH.into(), search.clone().into()),
            (id::VIEW_PROJECT_TASKS.into(), conversation.clone().into()),
            (id::VIEW_PREVIEW.into(), preview.clone().into()),
            (id::VIEW_SETTINGS.into(), settings.clone().into()),
            (id::VIEW_COMMAND_SEARCH.into(), quick_search.clone().into()),
        ]);
        let shell = cx.new(|cx| {
            Shell::new(
                editor.clone().into(),
                views,
                features::bundled_registry(),
                cx,
            )
        });
        let extension_terminal = services.extension_terminal;
        let branch_picker = cx.new(|cx| features::git::BranchPicker::new(git.clone(), window, cx));
        let terminal_backend = services.terminal.backend();
        // Terminal を設定の供給元として登録する。
        let terminal_settings = features::terminal::register_settings_source(cx);
        let terminal_context = std::rc::Rc::new(std::cell::RefCell::new(TerminalContext {
            root: terminal_root.clone(),
            conversation: terminal_context,
        }));
        let terminal =
            features::terminal::terminal_lifecycle(crate::features::terminal::TerminalTabSource {
                backend: terminal_backend,
                context: terminal_context,
                settings: terminal_settings,
                extension_settings: if extension_terminal {
                    locale_contributions.clone()
                } else {
                    None
                },
            });
        shell.update(cx, |shell, cx| {
            shell.branch_picker = Some(branch_picker.into());
            shell.set_locale(initial_locale.clone(), cx);
            if let Some(update) = update.clone() {
                update.update(cx, |update, cx| {
                    update.set_locale(initial_locale.clone(), cx)
                });
            }
            // Terminal を「宣言の供給者＋個体の所有者」として登録する。Shell は
            // 拡張の現在の稼働状態を受け取るまで View を隠す。
            if extension_terminal {
                shell.set_view_available(ViewId::new(id::VIEW_TERMINAL), false, cx);
                shell.set_view_hidden(ViewId::new(id::VIEW_TERMINAL), true, cx);
            }
            shell.register_content_renderer("lapis.content.terminal-grid.v1", terminal.clone(), cx);
            shell.configure_view_lifecycles(terminal.clone(), ViewId::new(id::VIEW_TERMINAL), cx);
            shell.open_view_in_zone(ZonePosition::Left, id::VIEW_PROJECT_TASKS.into(), cx);
            if initial.show_tasks {
                shell.open_view_in_zone(ZonePosition::Right, id::VIEW_ASSISTANT.into(), cx);
            }
            if initial.show_terminal {
                shell.open_view_in_zone(ZonePosition::Bottom, id::VIEW_TERMINAL.into(), cx);
            }
            #[cfg(target_family = "wasm")]
            if let Some(view) = desktop_client::web_host::restore_view() {
                shell.restore_view(view, cx);
            }
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
                |root, _, values: &desktop_client::ResolvedSettings, cx| {
                    let locale = values.locale();
                    root.shell
                        .update(cx, |shell, cx| shell.set_locale(locale.clone(), cx));
                    if let Some(update) = root.update.clone() {
                        update.update(cx, |update, cx| {
                            update.set_locale(locale.clone(), cx);
                            update.settings_changed(cx);
                        });
                    }
                    root.group.update(cx, |group, cx| {
                        group.locale = locale;
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
            cx.subscribe_in(&editor, window, |root, _, event, _window, cx| match event {
                EditorEvent::Snapshot(snapshot) => {
                    root.group
                        .update(cx, |group, cx| group.publish(*snapshot.clone(), cx));
                    root.synchronize_group(cx);
                    #[cfg(not(target_family = "wasm"))]
                    if root.native_smoke_requested {
                        root.native_smoke_requested = false;
                        crate::window_host::run_native_smoke(root, _window, cx);
                    }
                    #[cfg(not(target_family = "wasm"))]
                    root.apply_remote_workbench(cx);
                }
                EditorEvent::Idle => {
                    // 入力中に保留した共有の active は、この中の `receive_snapshot` →
                    // `accept_snapshot` で反映される。
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
            }),
            cx.subscribe(&conversation, |root, _, event, cx| {
                root.handle_conversation_event(event.clone(), cx);
            }),
            cx.observe(&editor, |root, editor, cx| {
                let editor = editor.read(cx);
                let cursor = editor.cursor_line_column();
                let selected = editor.selected_text();
                root.tasks
                    .update(cx, |tasks, _| tasks.set_selected_text(selected));
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
                // Layout のターミナルタブと実体を一致させ、表示中のタブだけ自動起動する。
                root.sync_resource_tabs(cx);
                root.sync_resource_manager_visibility(cx);
                root.schedule_persist(cx);
            }),
        ];
        let theme_source = NEXT_THEME_SOURCE.fetch_add(1, Ordering::Relaxed);
        subscriptions.push(cx.on_release(move |_, cx| {
            let selected = crate::theme::active_id(cx);
            let _ = crate::theme::replace_source(theme_source, Vec::new(), &selected, cx);
        }));

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
        if let Some(session) = locale_contributions {
            let receiver = session.subscribe();
            let settings_watch = services.settings.subscribe();
            let mut pending_terminal = extension_terminal && initial.show_terminal;
            cx.spawn(async move |root, cx| {
                let mut first = true;
                loop {
                    let mut changed = first;
                    first = false;
                    while receiver.try_recv().is_ok() {
                        changed = true;
                    }
                    while settings_watch.try_recv().is_ok() {
                        changed = true;
                    }
                    let alive = root
                        .update(cx, |root, cx| {
                            if changed {
                                let snapshot = session.snapshot();
                                for entity in root.terminal.borrow().entities() {
                                    entity.update(cx, |terminal, cx| terminal.refresh_extension_settings(cx));
                                }
                                if extension_terminal {
                                    let terminal_active = snapshot
                                        .as_ref()
                                        .is_some_and(|snapshot| snapshot.has_active_view("lapis.terminal", "lapis.content.terminal-grid.v1"));
                                    root.settings.update(cx, |settings, _| {
                                        settings.set_legacy_terminal_settings_hidden(terminal_active)
                                    });
                                    let available = extension_view_available(snapshot.as_ref(), "lapis.terminal", "lapis.content.terminal-grid.v1");
                                    let hidden = extension_view_hidden(snapshot.as_ref(), "lapis.terminal", "lapis.content.terminal-grid.v1");
                                    let declaration = snapshot.as_ref().and_then(|snapshot| snapshot.active_view("lapis.terminal", "lapis.content.terminal-grid.v1"));
                                    let shells = root.group.read(cx).members.values().map(|member| member.shell.clone()).collect::<Vec<_>>();
                                    if let Some(snapshot) = snapshot.as_ref() {
                                        let live = snapshot
                                            .extensions()
                                            .iter()
                                            .filter(|extension| extension.active == Some(true))
                                            .map(|extension| extension.id.clone())
                                            .collect::<Vec<_>>();
                                        let catalog = snapshot
                                            .extensions()
                                            .iter()
                                            .map(|extension| extension.id.clone())
                                            .collect::<Vec<_>>();
                                        for shell in &shells {
                                            shell.update(cx, |shell, cx| {
                                                shell.sync_extension_availability(
                                                    live.clone(),
                                                    catalog.clone(),
                                                    cx,
                                                )
                                            });
                                        }
                                    }
                                    if let Some(view) = declaration {
                                        let next = ViewId::new(view.id.clone());
                                        let old = root.terminal_view.clone();
                                        let legacy = ViewId::new(id::VIEW_TERMINAL);
                                        let sources = shells.iter().map(|shell| {
                                            let shell = shell.read(cx);
                                            if shell.can_rekey_extension_view(&old, &next) { Some(old.clone()) }
                                            else if shell.can_rekey_extension_view(&legacy, &next) { Some(legacy.clone()) }
                                            else { None }
                                        }).collect::<Option<Vec<_>>>();
                                        if let Some(sources) = sources.filter(|sources| root.group.read(cx).layout.can_rekey_views(sources, &next)) {
                                            root.group.update(cx, |group, _| group.layout.rekey_views(&sources, &next));
                                            for (shell, source) in shells.iter().zip(&sources) {
                                                if source != &next {
                                                    shell.update(cx, |shell, cx| { shell.rekey_extension_view(source, &next, cx); });
                                                }
                                            }
                                            root.terminal_view = next;
                                        } else {
                                            tracing::warn!("拡張 View ID が既存の View と競合しています");
                                        }
                                    }
                                    // 旧 View ID の付け替えが終わるまで新しい宣言を登録しない。
                                    // 先に登録すると、遷移先 ID が既存宣言と競合して付け替えを拒否する。
                                    if let Some(snapshot) = snapshot.as_ref() {
                                        let declarations = snapshot
                                            .active_views()
                                            .filter_map(|(owner, view)| {
                                                if view.content_type != "lapis.content.terminal-grid.v1" {
                                                    return None;
                                                }
                                                let contribution = extension_ui_contract::UiContribution::view(
                                                    view.id.clone(),
                                                    extension_ui_contract::UiSlot::BottomDock,
                                                    view.title.clone(),
                                                    owner,
                                                    100,
                                                )
                                                .multiple_instances()
                                                .references_external_resource()
                                                .feature_owned_scroll();
                                                Some((owner.to_owned(), (view.clone(), contribution)))
                                            })
                                            .fold(std::collections::BTreeMap::<String, Vec<_>>::new(), |mut grouped, (owner, view)| {
                                                grouped.entry(owner).or_default().push(view);
                                                grouped
                                            })
                                            .into_iter()
                                            .collect::<Vec<_>>();
                                        for shell in &shells {
                                            if let Err(error) = shell.update(cx, |shell, cx| {
                                                shell.sync_extension_declarations(declarations.clone(), cx)
                                            }) {
                                                tracing::warn!(target: "lapis::extensions", "拡張宣言を登録できませんでした: {}", error.as_str());
                                            }
                                        }
                                    }
                                    let terminal_view = root.terminal_view.clone();
                                    let declaration = declaration.filter(|view| view.id == terminal_view.as_str());
                                    let available = available && declaration.is_some();
                                    for shell in shells {
                                        shell.update(cx, |shell, cx| {
                                            shell.set_view_available(terminal_view.clone(), available, cx);
                                            if let (Some(snapshot), Some(view)) = (snapshot.as_ref(), declaration) {
                                                shell.set_extension_view_command(
                                                    terminal_view.clone(),
                                                    view.open_command.clone().map(Into::into),
                                                    contributed_zone(view.default_zone),
                                                    view.multiple_instances,
                                                    cx,
                                                );
                                                shell.set_extension_view_title(
                                                    terminal_view.clone(),
                                                    snapshot.clone(),
                                                    "lapis.terminal".to_owned(),
                                                    localization::MessageId::new(view.title.clone()),
                                                    cx,
                                                );
                                            }
                                            if let Some(hidden) = hidden {
                                                shell.set_view_hidden(terminal_view.clone(), hidden, cx);
                                            }
                                        });
                                    }
                                    if available && pending_terminal {
                                        let zone = declaration.map(|view| contributed_zone(view.default_zone)).unwrap_or(ZonePosition::Bottom);
                                        root.shell.update(cx, |shell, cx| shell.open_view_in_zone(zone, terminal_view.clone(), cx));
                                        pending_terminal = false;
                                    }
                                }
                                let packs = snapshot
                                    .as_ref()
                                    .map(|snapshot| snapshot.core_locales())
                                    .unwrap_or_default();
                                let definitions = snapshot.as_ref().map(|snapshot| {
                                    snapshot.contributions().iter()
                                        .flat_map(|contribution| contribution.declaration.themes.iter())
                                        .map(|theme| {
                                            let parsed = crate::theme::ThemeDefinition::from_json(&theme.definition.to_string())?;
                                            if parsed.id.as_str() != theme.id { return Err("theme id mismatch".to_owned()); }
                                            Ok(parsed)
                                        })
                                        .collect::<Result<Vec<_>, String>>()
                                }).unwrap_or_else(|| Ok(Vec::new()));
                                match definitions {
                                    Ok(definitions) => {
                                        let selected = crate::features::settings_coordinator::SettingsCoordinator::try_shared(cx)
                                            .map(|settings| extension_ui_contract::ThemeId::new(settings.read(cx).values.theme().to_owned()))
                                            .unwrap_or_else(|| crate::theme::active_id(cx));
                                        if let Err(id) = crate::theme::replace_source(theme_source, definitions, &selected, cx) {
                                            tracing::warn!(target: "lapis::theme", "テーマ ID が重複しています: {}", id.as_str());
                                        }
                                    }
                                    Err(error) => tracing::warn!(target: "lapis::theme", "テーマ定義が不正です: {error}"),
                                }
                                root.locale_packs = packs.to_vec();
                                let locale = root.group.read(cx).locale.clone();
                                root.shell.update(cx, |shell, cx| {
                                    shell.set_locale_packs(packs, &locale, cx)
                                });
                                root.quick_search.update(cx, |search, cx| {
                                    search.set_locale_packs(packs, &locale, cx)
                                });
                                root.universal_search.update(cx, |search, cx| {
                                    search.set_locale_packs(packs, &locale, cx)
                                });
                                if let Some(update) = &root.update {
                                    update.update(cx, |update, cx| {
                                        update.set_locale_packs(packs, &locale, cx)
                                    });
                                }
                                cx.refresh_windows();
                            }
                        })
                        .is_ok();
                    if !alive {
                        break;
                    }
                    crate::timer::Timer::after(std::time::Duration::from_millis(200)).await;
                }
            })
            .detach();
        }
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
            terminal,
            terminal_view: ViewId::new(id::VIEW_TERMINAL),
            editor,
            files,
            git,
            tasks,
            resources,
            search,
            conversation,
            preview,
            preview_revision: None,
            line_ending_cache: None,
            settings,
            update,
            quick_search,
            universal_search,
            locale_packs: Vec::new(),
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
            .children(self.update.clone())
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
