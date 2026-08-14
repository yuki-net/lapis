use super::*;

impl Editor {
    pub(super) fn start_background_tasks(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_secs(2)).await;
                if this
                    .update(cx, |editor, cx| {
                        if !editor.session.poll_external_changes().is_empty() {
                            editor.status = "外部ファイル変更を検出しました".to_owned();
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_millis(250)).await;
                if this
                    .update(cx, |editor, cx| match editor.tasks.session.refresh() {
                        Ok(true) => {
                            if editor.tasks.selected_execution.is_none() {
                                editor.tasks.selected_execution = editor
                                    .tasks
                                    .session
                                    .records()
                                    .first()
                                    .map(|record| record.execution.id.clone());
                            }
                            cx.notify();
                        }
                        Ok(false) => {}
                        Err(error) => {
                            editor.status = format!("Task 更新失敗: {error}");
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_secs(1)).await;
                if this
                    .update(cx, |editor, cx| {
                        if !editor.feature_registry.is_running(id::FEATURE_GIT) {
                            return;
                        }
                        let Some(root) = editor.session.workspace_root().map(ToOwned::to_owned)
                        else {
                            return;
                        };
                        if editor.git.session.refresh(&root).unwrap_or(false) {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_millis(250)).await;
                if this
                    .update(cx, |editor, cx| match editor.terminal.session.refresh() {
                        Ok(true) => cx.notify(),
                        Ok(false) => {}
                        Err(error) => {
                            editor.status = format!("Terminal refresh failed: {error}");
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_millis(500)).await;
                if this
                    .update(cx, |editor, cx| {
                        editor.refresh_feature_activation();
                        if !editor.feature_registry.is_running(id::FEATURE_RUST) {
                            if editor.problems.lsp.is_started()
                                && let Err(error) = editor.problems.lsp.shutdown()
                            {
                                editor.status = format!("LSP shutdown: {error}");
                                cx.notify();
                            }
                            return;
                        }
                        let synced = editor.problems.lsp.sync_active(&editor.session);
                        let refreshed = editor.problems.lsp.refresh();
                        match (synced, refreshed) {
                            (Ok(sync_changed), Ok(diagnostics_changed)) => {
                                if sync_changed || diagnostics_changed {
                                    cx.notify();
                                }
                            }
                            (Err(error), _) | (_, Err(error)) => {
                                editor.status = format!("LSP: {error}");
                                cx.notify();
                            }
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_millis(100)).await;
                if this
                    .update(cx, |editor, cx| {
                        if editor.search.workspace.refresh() {
                            editor.feature_registry.deactivate_command(
                                &crate::extension_ui::CommandId::new(id::COMMAND_FIND_WORKSPACE),
                            );
                            editor.status = if let Some(error) = editor.search.workspace.error() {
                                format!("Workspace search failed: {error}")
                            } else {
                                format!(
                                    "Workspace search: {} hits",
                                    editor.search.workspace.hits().len()
                                )
                            };
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_millis(50)).await;
                if this
                    .update(cx, |editor, cx| {
                        if let Some(result) = editor
                            .problems
                            .completion_receiver
                            .as_ref()
                            .and_then(|receiver| receiver.try_recv().ok())
                        {
                            editor.problems.completion_receiver = None;
                            editor.status = match result {
                                Ok(items) if items.is_empty() => "補完候補はありません".to_owned(),
                                Ok(items) => format!(
                                    "補完: {}",
                                    items
                                        .iter()
                                        .take(6)
                                        .map(|item| item.label.as_str())
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                ),
                                Err(error) => format!("補完失敗: {error}"),
                            };
                            cx.notify();
                        }
                        if let Some(result) = editor
                            .problems
                            .definition_receiver
                            .as_ref()
                            .and_then(|receiver| receiver.try_recv().ok())
                        {
                            editor.problems.definition_receiver = None;
                            match result {
                                Ok(Some(target)) => match editor.session.open_definition(&target) {
                                    Ok(offset) => {
                                        editor.restore_active_view();
                                        editor.move_to(offset, cx);
                                        editor.status = format!("定義: {}", target.path.display());
                                    }
                                    Err(error) => editor.status = format!("定義移動失敗: {error}"),
                                },
                                Ok(None) => editor.status = "定義が見つかりません".to_owned(),
                                Err(error) => editor.status = format!("定義移動失敗: {error}"),
                            }
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_secs(1)).await;
                if this
                    .update(cx, |editor, _| {
                        let _ = editor.capture_conversation();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }
}
