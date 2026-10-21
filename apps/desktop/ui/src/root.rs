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
};
use gpui::{
    AnyView, AppContext, Context, Entity, Focusable, Render, Subscription, Window, div, prelude::*,
};
use lapis_desktop_client::EditorSession;
use std::collections::HashMap;

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
    git_input: Option<GitInputRequest>,
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
                            root.git_input = None;
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
                    let local = root.editor.read(cx).local_snapshot(snapshot);
                    root.apply_snapshot(&local, cx);
                    root.group
                        .update(cx, |group, cx| group.publish(*snapshot.clone(), cx));
                    root.schedule_persist(cx);
                    if root.native_smoke_requested {
                        root.native_smoke_requested = false;
                        crate::window_host::run_native_smoke(root, window, cx);
                    }
                }
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
                                ConversationEvent::RegisterProject => {
                                    if let Some(path) =
                                        client.editor.file_dialog().choose_workspace_path()
                                    {
                                        client
                                            .conversation
                                            .register_project(&client.editor, path)
                                            .map_err(|error| error.to_string())?;
                                    }
                                    Ok(())
                                }
                                ConversationEvent::RegisterSource(project) => {
                                    if let Some(path) =
                                        client.editor.file_dialog().choose_workspace_path()
                                    {
                                        client
                                            .conversation
                                            .register_source(&client.editor, &project, path)
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
            git_input: None,
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
                snapshot.projects.clone(),
                snapshot.project_error.clone(),
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
                if root.group.read(cx).closing {
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
                                .editor
                                .close_workspace()
                                .map_err(|error| error.to_string());
                        }
                        let path = match command {
                            UiCommand::OpenRecent(path) => Some(path),
                            _ => client.editor.file_dialog().choose_workspace_path(),
                        };
                        if let Some(path) = path {
                            let project = client
                                .conversation
                                .register_project(&client.editor, path.clone())
                                .map_err(|error| error.to_string())?;
                            client
                                .conversation
                                .create_task(&mut client.editor, &project, path, None)
                                .map_err(|error| error.to_string())?;
                        }
                        Ok(())
                    },
                    cx,
                )
            });
            return;
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
