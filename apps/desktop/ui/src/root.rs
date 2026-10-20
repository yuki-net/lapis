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
    AnyView, AppContext, Context, Entity, Focusable, PromptButton, PromptLevel, Render,
    Subscription, Window, div, prelude::*,
};
use lapis_desktop_client::{DocumentCloseDisposition, EditorSession};
use std::collections::HashMap;

pub(crate) struct DesktopRoot {
    shell: Entity<Shell>,
    editor: Entity<Editor>,
    files: Entity<FilesFeature>,
    git: Entity<GitFeature>,
    tasks: Entity<TasksFeature>,
    terminal: Entity<TerminalFeature>,
    problems: Entity<ProblemsFeature>,
    search: Entity<SearchFeature>,
    conversation: Entity<ConversationFeature>,
    preview: Entity<PreviewFeature>,
    settings: Entity<SettingsFeature>,
    quick_search: Entity<QuickSearch>,
    _subscriptions: Vec<Subscription>,
    double_shift: DoubleShiftDetector,
    worker: ClientWorker<DocumentClient>,
    factory: crate::app::WindowFactory,
    persisting: bool,
    closing: bool,
}

impl DesktopRoot {
    pub(crate) fn new(
        session: EditorSession,
        mut services: DesktopServices,
        initial: InitialView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let factory = crate::app::WindowFactory::new(&session, &services);
        let conversation_id = services.conversation.active_id().clone();
        let initial_locale = services.settings.settings().locale;
        if services.terminal.terminals().is_empty() {
            let summaries = services
                .conversation
                .active_record()
                .map(|record| record.terminals.clone())
                .unwrap_or_default();
            services.terminal.restore_summaries(&summaries);
        }
        let worker = ClientWorker::new(DocumentClient {
            editor: session,
            conversation: services.conversation,
            lsp: services.lsp,
            edit_chain: EditChain::default(),
        });
        let editor = cx.new(|cx| Editor::new(worker.clone(), cx));
        let files = cx.new(|_| FilesFeature::new());
        let git = cx.new(|cx| GitFeature::new(services.git, cx));
        let tasks = cx.new(|cx| TasksFeature::new(services.task, cx));
        let terminal = cx.new(|cx| TerminalFeature::new(services.terminal, conversation_id, cx));
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
                        QuickSearchEvent::Dismiss => root
                            .shell
                            .update(cx, |shell, cx| shell.close_panel(PanelPosition::Right, cx)),
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
            shell.set_locale(initial_locale, cx);
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
        let mut subscriptions = vec![
            cx.subscribe(
                &settings,
                |root, _, values: &lapis_settings::GlobalSettings, cx| {
                    root.shell
                        .update(cx, |shell, cx| shell.set_locale(values.locale.clone(), cx));
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
                EditorEvent::Snapshot(snapshot) => root.apply_snapshot(snapshot, cx),
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
                let text = editor.document().content_for_save();
                let selected = editor.selected_text();
                root.tasks
                    .update(cx, |tasks, _| tasks.set_selected_text(selected));
                root.problems
                    .update(cx, |problems, cx| problems.set_target(target, cx));
                root.preview
                    .update(cx, |preview, cx| preview.update(text, cx));
                root.shell.update(cx, |shell, cx| {
                    if shell.session.cursor != cursor {
                        shell.session.cursor = cursor;
                        cx.notify();
                    }
                });
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
        cx.spawn(async move |this, cx| {
            loop {
                gpui::Timer::after(std::time::Duration::from_secs(1)).await;
                if this
                    .update(cx, |root, cx| {
                        if root.persisting || root.closing || root.editor.read(cx).has_pending() {
                            return;
                        }
                        let view = root.persisted_view(cx);
                        root.persisting = true;
                        let admitted = root.worker.submit(
                            cx,
                            move |client| client.persist_view(view),
                            |root, result, cx| {
                                root.persisting = false;
                                if let Err(error) = result {
                                    root.set_status(error, cx);
                                }
                            },
                        );
                        if let Err(error) = admitted {
                            root.persisting = false;
                            root.set_status(error, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        window.focus(&editor.focus_handle(cx));
        Self {
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
            settings,
            quick_search,
            _subscriptions: subscriptions,
            double_shift: DoubleShiftDetector::default(),
            worker,
            factory,
            persisting: false,
            closing: false,
        }
    }

    fn apply_snapshot(&mut self, snapshot: &ClientSnapshot, cx: &mut Context<Self>) {
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
        let presentation = ShellPresentation {
            documents: snapshot.tabs.clone(),
            active: snapshot.active.clone(),
            root: snapshot.root.clone(),
            workspace_name: snapshot.workspace_name.clone(),
            cursor: editor.cursor_line_column(),
            line_ending: if editor.document().content_for_save().contains("\r\n") {
                "CRLF"
            } else {
                "LF"
            },
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
    }

    fn persisted_view(&self, cx: &Context<Self>) -> PersistedView {
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

    fn begin_flush(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Result<std::sync::mpsc::Receiver<Result<(), String>>, String> {
        if self.closing {
            return Err("Windowの終了処理中です".into());
        }
        self.closing = true;
        self.editor
            .update(cx, |editor, cx| editor.set_enabled(false, cx));
        let view = self.persisted_view(cx);
        match self
            .worker
            .try_request(move |client| client.persist_view(view))
        {
            Ok(receiver) => Ok(receiver),
            Err(error) => {
                self.closing = false;
                self.editor
                    .update(cx, |editor, cx| editor.set_enabled(true, cx));
                Err(error)
            }
        }
    }
    fn quit_after_flush(&mut self, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        let own_id = cx.entity_id();
        let windows = cx.windows();
        let mut started_windows = Vec::new();
        let mut admission_error = None;
        let mut pending = Vec::new();
        match self.begin_flush(cx) {
            Ok(receiver) => pending.push((self.worker.clone(), receiver)),
            Err(error) => {
                self.set_status(error, cx);
                return;
            }
        }
        for handle in &windows {
            let result = handle.update(cx, |view, _, cx| {
                let root = view.downcast::<DesktopRoot>().ok()?;
                if root.entity_id() == own_id {
                    return None;
                }
                Some(root.update(cx, |root, cx| {
                    root.begin_flush(cx)
                        .map(|receiver| (root.worker.clone(), receiver))
                }))
            });
            match result {
                Ok(Some(Ok(pending_window))) => {
                    pending.push(pending_window);
                    started_windows.push(*handle);
                }
                Ok(Some(Err(error))) => admission_error = Some(error),
                Err(error) => admission_error = Some(error.to_string()),
                Ok(None) => {}
            }
        }
        cx.spawn(async move |this, cx| {
            let mut error = admission_error;
            for (_worker, receiver) in pending {
                loop {
                    match receiver.try_recv() {
                        Ok(result) => {
                            if let Err(failure) = result {
                                error = Some(failure);
                            }
                            break;
                        }
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            error = Some("Client workerが終了しました".into());
                            break;
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => {
                            gpui::Timer::after(std::time::Duration::from_millis(16)).await;
                        }
                    }
                }
            }
            let _ = this.update(cx, |root, cx| {
                if let Some(error) = error {
                    root.closing = false;
                    root.editor
                        .update(cx, |editor, cx| editor.set_enabled(true, cx));
                    root.set_status(error, cx);
                    for handle in started_windows {
                        let _ = handle.update(cx, |view, _, cx| {
                            if let Ok(entity) = view.downcast::<DesktopRoot>()
                                && entity.entity_id() != own_id
                            {
                                entity.update(cx, |root, cx| {
                                    root.closing = false;
                                    root.editor
                                        .update(cx, |editor, cx| editor.set_enabled(true, cx));
                                });
                            }
                        });
                    }
                } else {
                    cx.quit();
                }
            });
        })
        .detach();
    }

    fn request_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.closing {
            return false;
        }
        if self.editor.read(cx).needs_recovery() {
            let receiver = window.prompt(
                PromptLevel::Warning,
                "未確定の入力があります",
                Some("復旧する場合はCancelを選択し、入力を新しい文書へ退避してください。"),
                &[
                    PromptButton::new("Copy draft and close"),
                    PromptButton::new("Discard and close"),
                    PromptButton::cancel("Cancel"),
                ],
                cx,
            );
            let handle = window.window_handle();
            cx.spawn(async move |this, cx| {
                let Ok(answer) = receiver.await else {
                    return;
                };
                if answer > 1 {
                    return;
                }
                let _ = this.update(cx, |root, cx| {
                    if answer == 0 {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                            root.editor.read(cx).document().content_for_save(),
                        ));
                    }
                    root.closing = true;
                    let _ = handle.update(cx, |_, window, _| window.remove_window());
                });
            })
            .detach();
            return false;
        }
        self.closing = true;
        self.editor
            .update(cx, |editor, cx| editor.set_enabled(false, cx));
        let view = self.persisted_view(cx);
        let handle = window.window_handle();
        let admitted = self.worker.submit(
            cx,
            move |client| client.persist_view(view),
            move |root, result, cx| match result {
                Ok(()) => {
                    let _ = handle.update(cx, |_, window, _| window.remove_window());
                }
                Err(error) => {
                    root.closing = false;
                    root.editor
                        .update(cx, |editor, cx| editor.set_enabled(true, cx));
                    root.set_status(error, cx);
                }
            },
        );
        if let Err(error) = admitted {
            self.closing = false;
            self.editor
                .update(cx, |editor, cx| editor.set_enabled(true, cx));
            self.set_status(error, cx);
        }
        false
    }
    fn set_status(&mut self, status: String, cx: &mut Context<Self>) {
        self.shell
            .update(cx, |shell, cx| shell.set_status(status, cx));
    }

    fn command(&mut self, command: UiCommand, window: &mut Window, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        match command {
            UiCommand::Theme(theme) => self
                .settings
                .update(cx, |settings, cx| settings.select_theme(theme, cx)),
            UiCommand::QuickSearch => {
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
            UiCommand::CloseDocument(id) => self.close_document(id, window, cx),
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

    fn close_document(
        &mut self,
        id: lapis_editor_core::DocumentId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let clean = self
            .shell
            .read(cx)
            .session
            .documents
            .iter()
            .find(|tab| tab.id == id)
            .is_some_and(|tab| !tab.dirty)
            && !self.editor.read(cx).has_pending();
        if clean {
            self.editor.update(cx, |editor, cx| {
                editor.submit(
                    false,
                    move |client| {
                        client
                            .editor
                            .close_document(&id, DocumentCloseDisposition::PreserveChanges)
                            .map(|_| ())
                            .map_err(|error| error.to_string())
                    },
                    cx,
                )
            });
            return;
        }
        let receiver = window.prompt(
            PromptLevel::Warning,
            "文書を閉じますか？",
            Some("未保存の変更は、保存するか明示的に破棄してください。"),
            &[
                PromptButton::new("Save"),
                PromptButton::new("Discard"),
                PromptButton::cancel("Cancel"),
            ],
            cx,
        );
        cx.spawn(async move |this, cx| {
            let Ok(answer) = receiver.await else {
                return;
            };
            if answer > 1 {
                return;
            }
            let _ = this.update(cx, |root, cx| {
                root.editor.update(cx, |editor, cx| {
                    editor.submit(
                        false,
                        move |client| {
                            client.editor.activate_document(&id);
                            if answer == 0 {
                                let result = client
                                    .editor
                                    .save_document()
                                    .map_err(|error| error.to_string())?;
                                if result == lapis_desktop_client::DocumentAction::Cancelled {
                                    return Ok(());
                                }
                            }
                            client
                                .editor
                                .close_document(
                                    &id,
                                    if answer == 1 {
                                        DocumentCloseDisposition::DiscardChanges
                                    } else {
                                        DocumentCloseDisposition::PreserveChanges
                                    },
                                )
                                .map(|_| ())
                                .map_err(|error| error.to_string())
                        },
                        cx,
                    )
                });
            });
        })
        .detach();
    }
}
impl Render for DesktopRoot {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .on_action(cx.listener(|root, _: &crate::app::Quit, _, cx| {
                cx.stop_propagation();
                root.quit_after_flush(cx);
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
