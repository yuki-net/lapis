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
        git::GitFeature,
        id,
        preview::PreviewFeature,
        problems::{ProblemsEvent, ProblemsFeature},
        search::SearchFeature,
        settings::SettingsFeature,
        tasks::TasksFeature,
        terminal::TerminalFeature,
    },
    shell::main::{Shell, ShellPresentation},
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
    _subscriptions: Vec<Subscription>,
    double_shift: DoubleShiftDetector,
    pub(crate) worker: ClientWorker<DocumentClient>,
    factory: crate::app::WindowFactory,
    persist_task: Option<gpui::Task<()>>,
    persist_signature: Option<PersistSignature>,
    native_smoke_requested: bool,
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
            (id::VIEW_HISTORY.into(), conversation.clone().into()),
            (id::VIEW_PREVIEW.into(), preview.clone().into()),
            (id::VIEW_SETTINGS.into(), settings.clone().into()),
            (id::VIEW_COMMAND_SEARCH.into(), quick_search.clone().into()),
        ]);
        let shell = cx.new(|cx| Shell::new(editor.clone().into(), views, cx));
        shell.update(cx, |shell, cx| {
            shell.set_locale(initial_locale.clone(), cx);
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
                if root.group.read(cx).members.len() > 1 || root.group.read(cx).closing {
                    root.set_status(
                        "別Windowのタブを閉じるか再ドックしてから会話を切り替えてください".into(),
                        cx,
                    );
                    return;
                }
                let event = event.clone();
                let view = root.persisted_view(cx);
                root.editor.update(cx, |editor, cx| {
                    editor.submit(
                        false,
                        move |client| {
                            client.persist_view(view)?;
                            match event {
                                ConversationEvent::Create => client
                                    .conversation
                                    .create(&client.editor, Default::default())
                                    .map(|_| ())
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
            _subscriptions: subscriptions,
            double_shift: DoubleShiftDetector::default(),
            worker,
            factory,
            persist_task: None,
            persist_signature: None,
            native_smoke_requested: initial.native_window_smoke,
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
        let switched = self.conversation.read(cx).active.as_ref() != Some(&snapshot.conversation);
        let record = snapshot
            .conversations
            .iter()
            .find(|record| record.id == snapshot.conversation);
        self.files
            .update(cx, |files, cx| files.update(snapshot, cx));
        self.git.update(cx, |git, cx| {
            git.set_workspace(snapshot.workspace.clone(), cx)
        });
        self.tasks.update(cx, |tasks, cx| {
            tasks.set_conversation(snapshot.conversation.clone());
            tasks.set_workspace(snapshot.workspace.clone(), cx);
            if switched {
                tasks.selected_execution =
                    record.and_then(|record| record.selected_execution.clone());
            }
        });
        self.search.update(cx, |search, cx| {
            search.set_workspace(snapshot.root.clone(), cx)
        });
        self.settings.update(cx, |settings, cx| {
            settings.set_workspace(snapshot.root.clone(), cx)
        });
        self.terminal.update(cx, |terminal, cx| {
            terminal.set_context(
                snapshot.root.clone(),
                snapshot.conversation.clone(),
                record
                    .filter(|record| record.workspace.root == snapshot.root)
                    .map(|record| record.terminals.clone())
                    .unwrap_or_default(),
                cx,
            )
        });
        self.conversation.update(cx, |conversation, cx| {
            conversation.update(
                snapshot.conversations.clone(),
                snapshot.conversation.clone(),
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
        self.shell.update(cx, |shell, cx| {
            shell.update_presentation(presentation, cx);
            if switched {
                shell.restore_view(snapshot.view.clone(), cx);
            }
        });
        self.sync_preview(cx);
    }

    /// Captures this root's editor, layout, task, terminal, and draft state for later persistence.
    pub(crate) fn persisted_view(&self, cx: &gpui::App) -> PersistedView {
        PersistedView {
            document: self.editor.read(cx).view_state(),
            layout: self.shell.read(cx).view_state(),
            execution: self.tasks.read(cx).selected_execution.clone(),
            terminals: self.terminal.read(cx).summaries(),
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
        let signature = (
            versions,
            self.editor.read(cx).view_state(),
            self.shell.read(cx).view_state(),
        );
        if self.persist_signature.as_ref() == Some(&signature) {
            return;
        }
        self.persist_signature = Some(signature);
        self.persist_task = Some(cx.spawn(async move |this, cx| {
            gpui::Timer::after(std::time::Duration::from_millis(500)).await;
            let _ = this.update(cx, |root, cx| {
                if root.group.read(cx).closing || root.group.read(cx).synchronization_required {
                    // 指紋はデバウンス用であり、見送った書き込みは再同期後に再試行できるようにする。
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
