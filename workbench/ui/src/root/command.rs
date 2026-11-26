use super::*;
use crate::app::InitialView;
use crate::commands::UiCommand;
use crate::features::id;
use extension_ui_contract::ZonePosition;
use gpui::{Context, Window};

impl DesktopRoot {
    /// Updates the root shell's status and publishes it to the window group.
    pub(crate) fn set_status(&mut self, status: String, cx: &mut Context<Self>) {
        self.group.update(cx, |group, cx| {
            group.status = status.clone();
            cx.notify();
        });
        self.shell
            .update(cx, |shell, cx| shell.set_status(status, cx));
    }

    pub(super) fn submit_path(&mut self, input: String, cx: &mut Context<Self>) {
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
    pub(super) fn close_universal_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
                let keymap = crate::keymap::KeymapRegistry::active(cx).clone();
                let provider = self
                    .shell
                    .read(cx)
                    .command_search_provider(|command| keymap.shortcut_label(command));
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
            UiCommand::ActivateViewResource { view, resource } => {
                self.activate_view_resource(view, resource, cx);
                self.schedule_persist(cx);
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
                                    let (session, services) = crate::web_bridge::host_sessions()?;
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
                                        crate::web_bridge::notify_workspace_switched();
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
