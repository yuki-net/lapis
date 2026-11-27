use super::*;
use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use terminal::TerminalId;
use terminal::{
    TerminalBackend, TerminalDelta, TerminalEvent, TerminalInput, TerminalRow, TerminalRowUpdate,
    TerminalSize, TerminalStatus,
};

#[gpui::test]
fn close_confirmation_covers_running_and_inactive_sessions(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext;
    cx.update(|cx| {
        let (_, backend) = feature();
        let entity = cx.new(|cx| {
            TerminalFeature::test_new(
                TerminalSession::new(backend),
                Some("workspace".into()),
                ConversationId::new("task-a"),
                cx,
            )
        });
        entity.update(cx, |feature, cx| {
            feature.requests = TerminalRequests::default();
            assert!(!feature.requires_close_confirmation());
            feature.pending_auto_start = true;
            assert!(feature.requires_close_confirmation());
            feature.pending_auto_start = false;
            feature.bound_resource = Some(terminal::TerminalId::new("reconnecting"));
            assert!(feature.requires_close_confirmation());
            feature.bound_resource = None;
            feature.terminals.push(terminal::TerminalSnapshot::empty(
                terminal::TerminalId::new("terminal-a"),
                "workspace".into(),
                TerminalStatus::Exited,
                TerminalSize::new(80, 24),
            ));
            assert!(!feature.requires_close_confirmation());
            feature.terminals[0].status = TerminalStatus::Running;
            assert!(feature.requires_close_confirmation());
            feature.set_context(
                Some("workspace".into()),
                ConversationId::new("task-b"),
                vec![],
                cx,
            );
            assert!(feature.terminals.is_empty());
            assert!(feature.requires_close_confirmation());
        });
    });
}

#[gpui::test]
fn persistence_summary_ignores_output_and_tracks_restore_metadata(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext;
    cx.update(|cx| {
        let (_, backend) = feature();
        let entity = cx.new(|cx| {
            TerminalFeature::test_new(
                TerminalSession::new(backend),
                Some("workspace".into()),
                ConversationId::new("task-a"),
                cx,
            )
        });
        entity.update(cx, |feature, _| {
            feature.terminals.push(terminal::TerminalSnapshot::empty(
                terminal::TerminalId::new("terminal-a"),
                "workspace".into(),
                TerminalStatus::Running,
                TerminalSize::new(80, 24),
            ));
            let before = feature.persistence_summaries();
            feature.terminals[0].revision += 1;
            feature.terminals[0]
                .screen
                .scrollback
                .extend((0..1024).map(|_| terminal::TerminalRow::empty(80)));
            assert_eq!(feature.persistence_summaries(), before);
            feature.terminals[0].screen =
                terminal::TerminalScreen::empty(TerminalSize::new(120, 40));
            feature.terminals[0].status = TerminalStatus::Exited;
            let after = feature.persistence_summaries();
            assert_eq!(after[0].id, before[0].id);
            assert_eq!(after[0].cwd, before[0].cwd);
            assert_eq!(after[0].status, TerminalStatus::Exited);
            assert_eq!((after[0].columns, after[0].rows), (120, 40));
            assert_ne!(after, before);
        });
    });
}

#[gpui::test]
fn fixed_terminal_never_falls_back_and_pending_context_is_not_persistable(
    cx: &mut gpui::TestAppContext,
) {
    use gpui::AppContext;
    cx.update(|cx| {
        let (_, backend) = feature();
        let entity = cx.new(|cx| {
            TerminalFeature::test_new(
                TerminalSession::new(backend),
                Some("workspace".into()),
                ConversationId::new("task-a"),
                cx,
            )
        });
        entity.update(cx, |feature, _| {
            feature.terminals.push(terminal::TerminalSnapshot::empty(
                terminal::TerminalId::new("first"),
                "workspace".into(),
                terminal::TerminalStatus::Running,
                terminal::TerminalSize::new(80, 24),
            ));
            feature.bind_resource(Some(terminal::TerminalId::new("missing")));
            assert!(feature.active_id().is_none());
            assert!(
                feature
                    .persistence_owner_for(
                        &ConversationId::new("task-a"),
                        Some(std::path::Path::new("workspace"))
                    )
                    .is_none()
            );
            feature.bind_resource(Some(terminal::TerminalId::new("first")));
            assert_eq!(feature.active_id().unwrap().as_str(), "first");
            // 作成直後で保存一覧にまだないIDも、実際の接続先が確定していれば保存できる。
            assert_eq!(
                feature.persistence_owner_for(
                    &ConversationId::new("task-a"),
                    Some(std::path::Path::new("workspace"))
                ),
                Some(ConversationId::new("task-a"))
            );
            assert!(
                feature
                    .persistence_owner_for(
                        &ConversationId::new("task-b"),
                        Some(std::path::Path::new("workspace"))
                    )
                    .is_none()
            );
            assert!(
                feature
                    .persistence_owner_for(
                        &ConversationId::new("task-a"),
                        Some(std::path::Path::new("changed-source"))
                    )
                    .is_none()
            );
            assert_eq!(
                feature.persistence_owner(),
                Some(ConversationId::new("task-a"))
            );
            let token = feature.requests.begin_action();
            assert!(feature.persistence_owner().is_none());
            feature.requests.finish_action(token);
            assert!(feature.persistence_owner().is_some());
            feature.requests.context_unavailable = true;
            assert!(feature.persistence_owner().is_none());
        });
    });
}

#[gpui::test]
fn terminal_uses_declared_settings_and_falls_back_to_previous_values(
    cx: &mut gpui::TestAppContext,
) {
    use gpui::AppContext;
    struct Memory(Mutex<settings::SettingsDocument>);
    impl settings::SettingsRepository for Memory {
        fn load(&self) -> Result<settings::SettingsDocument, settings::SettingsError> {
            Ok(self.0.lock().unwrap().clone())
        }
        fn save(&self, value: &settings::SettingsDocument) -> Result<(), settings::SettingsError> {
            *self.0.lock().unwrap() = value.clone();
            Ok(())
        }
    }
    let mut document = settings::SettingsDocument::empty(settings::SettingsScope::User);
    document.insert(
        settings::TERMINAL_FONT_SIZE,
        settings::SettingValue::integer(14),
    );
    let settings =
        desktop_client::SettingsSession::load(Arc::new(Memory(Mutex::new(document)))).unwrap();
    let extensions = settings.extension_contributions();
    let generation = extensions.begin_reload();
    let declaration: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../../../extensions/terminal/contributions.json"
    ))
    .unwrap();
    extensions.apply(generation, serde_json::from_value(serde_json::json!({
        "extensions":[{"id":"lapis.terminal","version":"1.0.0","installed":true,"enabled":true,"active":true}],
        "contributions":[{"extension_id":"lapis.terminal","declaration":declaration.clone()}]
    })).unwrap()).unwrap();
    let other = settings.extension_contributions();
    let generation = other.begin_reload();
    other.apply(generation, serde_json::from_value(serde_json::json!({
        "extensions":[{"id":"lapis.terminal","version":"1.0.0","installed":true,"enabled":true,"active":true}],
        "contributions":[{"extension_id":"lapis.terminal","declaration":declaration}]
    })).unwrap()).unwrap();
    let events = extensions.subscribe();
    let settings_watch = settings.subscribe();
    cx.update(|cx| {
        let backend = Arc::new(Backend::default());
        let entity = cx.new(|cx| {
            TerminalFeature::new(
                TerminalSession::new(backend),
                None,
                ConversationId::new("task"),
                std::rc::Rc::new(std::cell::RefCell::new(
                    super::super::TerminalProfileCatalog::default(),
                )),
                Some(extensions.clone()),
                cx,
            )
        });
        assert_eq!(entity.read(cx).font_size, 14.0);
        extensions
            .update(
                settings::SettingsScope::User,
                &[
                    (
                        "extensions/lapis.terminal/font-size",
                        Some(settings::SettingValue::integer(18)),
                    ),
                    (
                        "extensions/lapis.terminal/font-family",
                        Some(settings::SettingValue::text("Example Mono")),
                    ),
                    (
                        "extensions/lapis.terminal/line-height-percent",
                        Some(settings::SettingValue::integer(140)),
                    ),
                    (
                        "extensions/lapis.terminal/caret-shape",
                        Some(settings::SettingValue::text("bar")),
                    ),
                    (
                        "extensions/lapis.terminal/default-profile",
                        Some(settings::SettingValue::text("cmd")),
                    ),
                ],
            )
            .unwrap();
        assert!(events.try_recv().is_ok());
        assert!(settings_watch.try_recv().is_ok());
        assert_eq!(
            terminal_number(
                &other.resolved().unwrap(),
                "font-size",
                desktop_client::ResolvedSettings::terminal_font_size
            ),
            Some(18)
        );
        entity.update(cx, |feature, cx| feature.refresh_extension_settings(cx));
        assert_eq!(entity.read(cx).font_size, 18.0);
        assert_eq!(entity.read(cx).font_family.as_ref(), "Example Mono");
        assert_eq!(entity.read(cx).line_height_percent, 140);
        assert_eq!(
            entity.read(cx).caret_shape,
            settings::TerminalCaretShape::Bar
        );
        assert_eq!(
            entity
                .read(cx)
                .default_profile
                .as_ref()
                .map(|id| id.as_str()),
            Some("cmd")
        );
        extensions
            .update(
                settings::SettingsScope::User,
                &[("extensions/lapis.terminal/font-size", None)],
            )
            .unwrap();
        entity.update(cx, |feature, cx| feature.refresh_extension_settings(cx));
        assert_eq!(entity.read(cx).font_size, 14.0);
    });
}

fn model_row(columns: u16, text: &str) -> TerminalRow {
    let mut row = TerminalRow::empty(columns);
    for (column, character) in text.chars().take(columns as usize).enumerate() {
        row.cells[column].text = character.to_string();
    }
    row
}

fn model_delta(revision: u64, size: TerminalSize, text: &str) -> TerminalDelta {
    TerminalDelta {
        revision,
        size,
        changed_rows: vec![TerminalRowUpdate {
            row: 0,
            value: model_row(size.columns, text),
        }],
        cursor: Default::default(),
        modes: Default::default(),
        scrollback: None,
    }
}

#[gpui::test]
fn older_action_or_poll_responses_do_not_restore_terminal_list_or_owner(
    cx: &mut gpui::TestAppContext,
) {
    use gpui::AppContext;

    cx.update(|cx| {
        let (_, backend) = feature();
        let entity = cx.new(|cx| {
            TerminalFeature::test_new(
                TerminalSession::new(backend),
                Some("workspace".into()),
                ConversationId::new("task-a"),
                cx,
            )
        });
        entity.update(cx, |feature, _| {
            let newer = terminal::TerminalSnapshot::empty(
                terminal::TerminalId::new("newer"),
                "workspace".into(),
                TerminalStatus::Running,
                TerminalSize::new(80, 24),
            );
            let older = terminal::TerminalSnapshot::empty(
                terminal::TerminalId::new("older"),
                "workspace".into(),
                TerminalStatus::Running,
                TerminalSize::new(80, 24),
            );
            let newer_owners =
                HashMap::from([("newer".to_owned(), Some("owner-newer".to_owned()))]);
            let older_owners =
                HashMap::from([("older".to_owned(), Some("owner-older".to_owned()))]);

            // 新しい応答の後に古い poll が届いても、一覧と owner を戻さない。
            feature.apply_projection(2, vec![newer.clone()], newer_owners.clone());
            feature.apply_projection(1, vec![older.clone()], older_owners.clone());
            assert_eq!(feature.terminals[0].id, newer.id);
            assert_eq!(feature.owners, newer_owners);

            // 新しい poll の後に古い action が届いても同じ。
            feature.apply_projection(4, vec![newer.clone()], newer_owners.clone());
            feature.apply_projection(3, vec![older], older_owners);
            assert_eq!(feature.terminals[0].id, newer.id);
            assert_eq!(feature.owners, newer_owners);
        });
    });
}

#[test]
fn resync_attach_error_keeps_prior_owner_projection() {
    let (mut sessions, backend) = feature();
    let id = sessions
        .session
        .start(Path::new("workspace"), 80, 24)
        .unwrap();
    backend.events.lock().unwrap().insert(
        id.as_str().to_owned(),
        vec![
            TerminalEvent::Owner {
                owner: Some("session-a".to_owned()),
            },
            // revision gap で再同期に入り、attach はこのテストの backend では失敗する。
            TerminalEvent::Delta(model_delta(5, TerminalSize::new(80, 24), "gap")),
        ],
    );

    assert!(sessions.refresh().is_err());
    assert_eq!(sessions.session.owner(&id), Some("session-a"));
}

#[gpui::test]
fn refresh_errors_project_the_owner_to_the_feature(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext;

    cx.update(|cx| {
        let entity = cx.new(|cx| {
            TerminalFeature::test_new(
                TerminalSession::new(Arc::new(Backend::default())),
                None,
                ConversationId::new("task-a"),
                cx,
            )
        });

        // poll エラー: 別 Terminal の poll が失敗しても、先の owner を UI に投影する。
        let (mut sessions, backend) = feature();
        let first = sessions
            .session
            .start(Path::new("workspace"), 80, 24)
            .unwrap();
        let second = sessions
            .session
            .start(Path::new("workspace"), 80, 24)
            .unwrap();
        backend.events.lock().unwrap().insert(
            first.as_str().to_owned(),
            vec![TerminalEvent::Owner {
                owner: Some("owner-poll".to_owned()),
            }],
        );
        *backend.fail_poll_for.lock().unwrap() = Some(second.as_str().to_owned());
        assert!(
            sessions
                .refresh()
                .map_err(|error| error.to_string())
                .is_err()
        );
        let version = sessions.version();
        let owners = sessions.owners().clone();
        let terminals = sessions.session.terminals().to_vec();
        entity.update(cx, |feature, _| {
            feature.apply_projection(version, terminals, owners);
            assert_eq!(
                feature.owners.get(first.as_str()),
                Some(&Some("owner-poll".to_owned()))
            );
        });

        // 再同期 attach エラー: revision gap の後の attach が失敗しても owner を投影する。
        let (mut sessions, backend) = feature();
        let id = sessions
            .session
            .start(Path::new("workspace"), 80, 24)
            .unwrap();
        backend.events.lock().unwrap().insert(
            id.as_str().to_owned(),
            vec![
                TerminalEvent::Owner {
                    owner: Some("owner-resync".to_owned()),
                },
                TerminalEvent::Delta(model_delta(5, TerminalSize::new(80, 24), "gap")),
            ],
        );
        assert!(
            sessions
                .refresh()
                .map_err(|error| error.to_string())
                .is_err()
        );
        let version = sessions.version();
        let owners = sessions.owners().clone();
        let terminals = sessions.session.terminals().to_vec();
        entity.update(cx, |feature, _| {
            feature.apply_projection(version, terminals, owners);
            assert_eq!(
                feature.owners.get(id.as_str()),
                Some(&Some("owner-resync".to_owned()))
            );
        });
    });
}

#[test]
fn owner_updates_survive_a_later_terminal_poll_error() {
    let (mut sessions, backend) = feature();
    let first = sessions
        .session
        .start(Path::new("workspace"), 80, 24)
        .unwrap();
    let second = sessions
        .session
        .start(Path::new("workspace"), 80, 24)
        .unwrap();
    backend.events.lock().unwrap().insert(
        first.as_str().to_owned(),
        vec![TerminalEvent::Owner {
            owner: Some("session-a".to_owned()),
        }],
    );
    *backend.fail_poll_for.lock().unwrap() = Some(second.as_str().to_owned());

    let before = sessions.version();
    assert!(sessions.refresh().is_err());
    // エラーでも部分的な更新を投影できるよう、版は進める。
    assert!(sessions.version() > before);
    assert_eq!(sessions.session.owner(&first), Some("session-a"));
}

#[test]
fn stale_poll_does_not_release_a_newer_poll() {
    let mut requests = TerminalRequests::default();
    let stale = requests.begin_poll().unwrap();
    requests.invalidate();
    let current = requests.begin_poll().unwrap();
    assert!(!requests.finish_poll(stale));
    // 古い応答では現在の poll の pending を解除しない。
    assert!(requests.begin_poll().is_none());
    assert!(requests.finish_poll(current));
    assert!(requests.begin_poll().is_some());
}

#[test]
fn projection_version_bumps_on_actions_and_changes_only() {
    let (mut sessions, backend) = feature();
    let initial = sessions.version();
    let terminals = sessions
        .action(|session| session.start(Path::new("workspace"), 80, 24).map(|_| ()))
        .1;
    let id = terminals.last().unwrap().id.clone();
    let after_start = sessions.version();
    assert!(after_start > initial);

    // 変更が無い refresh では版を進めない。
    assert!(!sessions.refresh().unwrap());
    assert_eq!(sessions.version(), after_start);

    backend.events.lock().unwrap().insert(
        id.as_str().to_owned(),
        vec![TerminalEvent::Delta(model_delta(
            1,
            TerminalSize::new(80, 24),
            "x",
        ))],
    );
    assert!(sessions.refresh().unwrap());
    assert!(sessions.version() > after_start);
}

#[test]
fn failed_context_admission_suppresses_polling_until_retry_is_admitted() {
    let mut requests = TerminalRequests::default();
    let previous_poll = requests.begin_poll().unwrap();
    requests.invalidate();
    requests.context_unavailable = true;
    assert!(!requests.finish_poll(previous_poll));
    assert!(requests.begin_poll().is_none());
    requests.context_unavailable = false;
    let poll = requests.begin_poll().unwrap();
    assert!(requests.finish_poll(poll));
}

#[test]
fn polling_during_an_action_does_not_drop_the_action_result() {
    let worker = crate::client_adapter::ClientWorker::new(());
    let mut requests = TerminalRequests::default();
    let token = requests.begin_action();
    let poll = requests
        .begin_poll()
        .expect("polling must not wait for a pending action");
    assert!(requests.finish_poll(poll));
    let receive = worker
        .try_request(|_| Err::<(), _>("terminal input failed".to_owned()))
        .unwrap();
    let result = receive
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    assert!(requests.finish_action(token));
    assert_eq!(result.unwrap_err(), "terminal input failed");
}

#[test]
fn stale_actions_do_not_block_polling_and_pending_count_is_tracked() {
    let mut requests = TerminalRequests::default();
    let first = requests.begin_action();
    let second = requests.begin_action();
    assert!(!requests.finish_action(first));
    let poll = requests
        .begin_poll()
        .expect("polling does not wait for actions");
    assert!(requests.begin_poll().is_none());
    assert!(requests.finish_poll(poll));
    requests.gate.invalidate();
    assert!(!requests.finish_action(second));
    assert_eq!(requests.pending_actions, 0);
    let rejected = requests.begin_action();
    assert!(requests.finish_action(rejected));
    assert_eq!(requests.pending_actions, 0);
    assert!(requests.begin_poll().is_some());
}

#[test]
fn action_snapshots_retain_output_consumed_by_a_superseded_poll_even_on_failure() {
    let (mut sessions, backend) = feature();
    let id = sessions
        .session
        .start(Path::new("workspace"), 80, 24)
        .unwrap();
    backend.events.lock().unwrap().insert(
        id.as_str().to_owned(),
        vec![TerminalEvent::Delta(model_delta(
            1,
            TerminalSize::new(80, 24),
            "background output",
        ))],
    );
    let mut requests = TerminalRequests::default();
    let poll = requests.begin_poll().unwrap();
    assert!(sessions.refresh().unwrap());
    let action = requests.begin_action();
    let (outcome, terminals) = sessions.action(|_| Err(TerminalError::new("input failed")));
    assert!(requests.finish_poll(poll));
    assert!(requests.finish_action(action));
    assert_eq!(outcome.unwrap_err(), "input failed");
    assert!(
        terminals[0].screen.rows[0]
            .plain_text()
            .starts_with("background output")
    );
    assert!(!sessions.refresh().unwrap());
}

#[test]
fn polls_and_actions_use_independent_response_gates() {
    let mut requests = TerminalRequests::default();
    let poll = requests.begin_poll().unwrap();
    let action = requests.begin_action();
    // poll の結果は action の結果を破棄しない。
    assert!(requests.finish_poll(poll));
    assert!(requests.finish_action(action));
    let next_poll = requests.begin_poll().unwrap();
    assert!(requests.begin_poll().is_none());
    assert!(requests.finish_poll(next_poll));
}

#[derive(Default)]
struct Backend {
    next: AtomicUsize,
    terminated: AtomicUsize,
    events: Mutex<HashMap<String, Vec<TerminalEvent>>>,
    inputs: Mutex<Vec<TerminalId>>,
    sent_inputs: Mutex<Vec<TerminalInput>>,
    input_calls: AtomicUsize,
    /// 0 始まりの input 呼び出し番号。一度だけ失敗させる。
    fail_input_calls: Mutex<Vec<usize>>,
    polls: AtomicUsize,
    fail_poll_for: Mutex<Option<String>>,
    /// 起動時に要求された profile。自動起動と例外操作の検証に使う。
    started_profiles: Mutex<Vec<Option<terminal::TerminalProfileId>>>,
    /// Host が公開する profile 一覧。
    profiles: Mutex<terminal::TerminalProfiles>,
    fail_profiles: std::sync::atomic::AtomicBool,
}

impl TerminalBackend for Backend {
    fn start(&self, cwd: &Path, columns: u16, rows: u16) -> Result<TerminalId, TerminalError> {
        self.start_profile(cwd, columns, rows, None)
    }
    fn start_profile(
        &self,
        _: &Path,
        _: u16,
        _: u16,
        profile: Option<&terminal::TerminalProfileId>,
    ) -> Result<TerminalId, TerminalError> {
        self.started_profiles.lock().unwrap().push(profile.cloned());
        Ok(TerminalId::new(format!(
            "terminal-{}",
            self.next.fetch_add(1, Ordering::Relaxed)
        )))
    }
    fn profiles(&self, _: &Path) -> Result<terminal::TerminalProfiles, TerminalError> {
        if self.fail_profiles.load(Ordering::Relaxed) {
            return Err(TerminalError::new("profiles unavailable"));
        }
        Ok(self.profiles.lock().unwrap().clone())
    }
    fn input(&self, id: &TerminalId, input: &TerminalInput) -> Result<(), TerminalError> {
        self.inputs.lock().unwrap().push(id.clone());
        let call = self.input_calls.fetch_add(1, Ordering::Relaxed);
        let failed = {
            let mut failures = self.fail_input_calls.lock().unwrap();
            failures
                .iter()
                .position(|index| *index == call)
                .map(|position| failures.remove(position))
                .is_some()
        };
        if failed {
            return Err(TerminalError::new("input failed"));
        }
        self.sent_inputs.lock().unwrap().push(input.clone());
        Ok(())
    }
    fn resize(&self, _: &TerminalId, _: u16, _: u16) -> Result<(), TerminalError> {
        Ok(())
    }
    fn poll(&self, id: &TerminalId) -> Result<Vec<TerminalEvent>, TerminalError> {
        self.polls.fetch_add(1, Ordering::Relaxed);
        if self.fail_poll_for.lock().unwrap().as_deref() == Some(id.as_str()) {
            return Err(TerminalError::new("poll failed"));
        }
        Ok(self
            .events
            .lock()
            .unwrap()
            .remove(id.as_str())
            .unwrap_or_default())
    }
    fn terminate(&self, _: &TerminalId) -> Result<(), TerminalError> {
        self.terminated.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

fn feature() -> (TerminalSessions, Arc<Backend>) {
    let backend = Arc::new(Backend::default());
    (
        TerminalSessions::new(
            TerminalSession::new(backend.clone()),
            Some("workspace".into()),
            ConversationId::new("first"),
        ),
        backend,
    )
}

#[gpui::test]
fn terminal_view_draws_rows_reuses_shaped_lines_and_coalesces_notifies(
    cx: &mut gpui::TestAppContext,
) {
    use gpui::AppContext;

    cx.update(|cx| {
        crate::theme::init(&extension_ui_contract::ThemeId::new("lapis.dark"), None, cx);
    });
    let window = cx.add_window(|_, cx| {
        let (_, backend) = feature();
        let mut feature = TerminalFeature::test_new(
            TerminalSession::new(backend),
            Some("workspace".into()),
            ConversationId::new("task-a"),
            cx,
        );
        let mut snapshot = terminal::TerminalSnapshot::empty(
            terminal::TerminalId::new("first"),
            "workspace".into(),
            TerminalStatus::Running,
            TerminalSize::new(80, 24),
        );
        // ASCII 行と、セル境界と glyph advance がずれやすい日本語行を混在させる。
        snapshot.screen.rows = (0..24)
            .map(|index| {
                if index % 2 == 0 {
                    model_row(80, "hello world")
                } else {
                    let mut row = model_row(80, "こんにちは世界");
                    for (column, cell) in row.cells.iter_mut().enumerate() {
                        if column < 7 && !cell.text.is_empty() {
                            // ワイド文字は 2 セル分を占有する。
                            cell.width = 2;
                        }
                    }
                    row
                }
            })
            .collect();
        feature.terminals.push(snapshot);
        feature
    });

    for _ in 0..2 {
        cx.update_window(window.into(), |_, window, cx| {
            window.refresh();
            window.draw(cx).clear();
        })
        .unwrap();
        cx.run_until_parked();
    }

    // 同じ frame 内の複数 delta は 1 回の notify にまとまり、描画で消費される。
    window
        .update(cx, |feature, _, cx| {
            feature.request_output_render(cx);
            feature.request_output_render(cx);
            assert!(feature.pending_output_render);
        })
        .unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        window.refresh();
        window.draw(cx).clear();
    })
    .unwrap();
    window
        .update(cx, |feature, _, _| {
            assert!(!feature.pending_output_render);
        })
        .unwrap();
}

fn drain_inputs(
    cx: &mut gpui::TestAppContext,
    feature: &gpui::Entity<TerminalFeature>,
    done: impl Fn(&TerminalFeature) -> bool,
) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        if cx.update(|cx| done(feature.read(cx))) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "terminal input did not settle"
        );
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(16));
        cx.run_until_parked();
        std::thread::yield_now();
    }
}

#[gpui::test]
fn terminal_text_is_batched_and_control_key_flushes_it_immediately(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext;
    let (entity, backend) = cx.update(|cx| {
        let backend = Arc::new(Backend::default());
        let mut session = TerminalSession::new(backend.clone());
        session.start(Path::new("workspace"), 80, 24).unwrap();
        let entity = cx.new(|cx| {
            TerminalFeature::test_new(
                session,
                Some("workspace".into()),
                ConversationId::new("task-a"),
                cx,
            )
        });
        entity.update(cx, |feature, cx| {
            feature.send_text("ec".into(), cx);
            feature.send_text("ho".into(), cx);
            assert!(
                backend.sent_inputs.lock().unwrap().is_empty(),
                "text waits for the batch window"
            );
            feature.send_input(
                TerminalInput::Key(terminal::TerminalKeyEvent {
                    key: terminal::TerminalKey::Enter,
                    modifiers: Default::default(),
                }),
                cx,
            );
        });
        (entity, backend)
    });
    let sent = backend.clone();
    drain_inputs(cx, &entity, move |_| {
        sent.sent_inputs.lock().unwrap().len() == 2
    });
    assert_eq!(
        *backend.sent_inputs.lock().unwrap(),
        vec![
            TerminalInput::Text("echo".into()),
            TerminalInput::Key(terminal::TerminalKeyEvent {
                key: terminal::TerminalKey::Enter,
                modifiers: Default::default(),
            })
        ]
    );
}

#[gpui::test]
fn terminal_input_failure_shows_error_resyncs_and_retries_without_duplicates(
    cx: &mut gpui::TestAppContext,
) {
    use gpui::AppContext;
    let (entity, backend) = cx.update(|cx| {
        let backend = Arc::new(Backend::default());
        let mut session = TerminalSession::new(backend.clone());
        session.start(Path::new("workspace"), 80, 24).unwrap();
        // 1 回目の batch で Text だけ受理し、Key と再送 Key を失敗させる。
        *backend.fail_input_calls.lock().unwrap() = vec![1, 2];
        let entity = cx.new(|cx| {
            TerminalFeature::test_new(
                session,
                Some("workspace".into()),
                ConversationId::new("task-a"),
                cx,
            )
        });
        entity.update(cx, |feature, cx| {
            feature.send_text("ab".into(), cx);
            feature.send_input(
                TerminalInput::Key(terminal::TerminalKeyEvent {
                    key: terminal::TerminalKey::Character("c".into()),
                    modifiers: terminal::TerminalModifiers {
                        control: true,
                        ..Default::default()
                    },
                }),
                cx,
            );
            feature.send_input(TerminalInput::Paste("xy".into()), cx);
        });
        (entity, backend)
    });

    let settled = backend.clone();
    drain_inputs(cx, &entity, move |feature| {
        feature.message.contains("input failed")
            && !feature.resyncing_inputs
            && feature
                .inputs
                .iter()
                .all(|(_, queue)| !queue.has_in_flight())
            && settled.sent_inputs.lock().unwrap().len() == 1
    });
    assert!(
        backend.polls.load(Ordering::Relaxed) >= 1,
        "failure must resynchronize"
    );
    assert_eq!(
        *backend.sent_inputs.lock().unwrap(),
        vec![TerminalInput::Text("ab".into())],
        "accepted text is never retried"
    );

    // 再試行が止まった後でも、次の入力で未送信分を失わずに再開する。
    cx.update(|cx| entity.update(cx, |feature, cx| feature.send_text("z".into(), cx)));
    let recovered = backend.clone();
    drain_inputs(cx, &entity, move |feature| {
        feature.message.is_empty() && recovered.sent_inputs.lock().unwrap().len() == 4
    });
    assert_eq!(
        *backend.sent_inputs.lock().unwrap(),
        vec![
            TerminalInput::Text("ab".into()),
            TerminalInput::Key(terminal::TerminalKeyEvent {
                key: terminal::TerminalKey::Character("c".into()),
                modifiers: terminal::TerminalModifiers {
                    control: true,
                    ..Default::default()
                },
            }),
            TerminalInput::Paste("xy".into()),
            TerminalInput::Text("z".into()),
        ]
    );
}

#[gpui::test]
fn superseded_resync_clears_state_and_still_retries_pending_input(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext;
    let (entity, backend) = cx.update(|cx| {
        let backend = Arc::new(Backend::default());
        let mut session = TerminalSession::new(backend.clone());
        session.start(Path::new("workspace"), 80, 24).unwrap();
        let entity = cx.new(|cx| {
            TerminalFeature::test_new(
                session,
                Some("workspace".into()),
                ConversationId::new("task-a"),
                cx,
            )
        });
        entity.update(cx, |feature, cx| {
            let id = feature.active_id().unwrap();
            feature.queue_mut(&id).enqueue_text("ab".into());
            feature.resync_inputs(cx);
            assert!(feature.resyncing_inputs);
            // resize / claim / operate と同じく、再同期の応答前に action を進める。
            feature.requests.begin_action();
        });
        (entity, backend)
    });

    let sent = backend.clone();
    drain_inputs(cx, &entity, move |feature| {
        !feature.resyncing_inputs
            && !feature.input_recovery
            && sent.sent_inputs.lock().unwrap().len() == 1
            && feature.inputs.iter().all(|(_, queue)| !queue.has_pending())
    });
    assert_eq!(
        *backend.sent_inputs.lock().unwrap(),
        vec![TerminalInput::Text("ab".into())],
        "追い越されても未送信入力は失わない"
    );

    // フラグが残っていないため、次の再同期も即座に実行できる。
    let polls = backend.polls.load(Ordering::Relaxed);
    cx.update(|cx| {
        entity.update(cx, |feature, cx| {
            feature.resync_inputs(cx);
        })
    });
    drain_inputs(cx, &entity, |feature| !feature.resyncing_inputs);
    assert!(
        backend.polls.load(Ordering::Relaxed) > polls,
        "再同期が再び実行される"
    );
}

#[test]
fn workspace_switch_isolates_terminal_operations_without_terminating_previous_context() {
    let (mut sessions, backend) = feature();
    let original = sessions
        .session
        .start(Path::new("workspace"), 80, 24)
        .unwrap();
    let conversation = ConversationId::new("first");
    assert!(
        sessions
            .switch_context(Some("other".into()), conversation.clone(), &[])
            .is_empty()
    );
    backend.events.lock().unwrap().insert(
        original.as_str().to_owned(),
        vec![TerminalEvent::Delta(model_delta(
            1,
            TerminalSize::new(80, 24),
            "inactive output",
        ))],
    );
    assert!(sessions.refresh().unwrap());
    assert!(sessions.session.terminals().is_empty());
    let current = sessions.session.start(Path::new("other"), 80, 24).unwrap();
    let (result, terminals) = sessions.action(|session| {
        session.input(
            &session.terminals().last().unwrap().id,
            "new workspace input",
        )
    });
    result.unwrap();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].id, current);
    assert_eq!(terminals[0].cwd, Path::new("other"));
    assert_eq!(*backend.inputs.lock().unwrap(), vec![current.clone()]);
    assert!(
        sessions
            .switch_context(
                None,
                conversation.clone(),
                &[RestoredTerminal {
                    id: None,
                    cwd: "other".into(),
                    status: TerminalStatus::Running,
                    columns: 80,
                    rows: 24,
                }]
            )
            .is_empty()
    );
    assert!(!sessions.refresh().unwrap());
    let original_snapshot =
        sessions.switch_context(Some("workspace".into()), conversation.clone(), &[]);
    assert_eq!(original_snapshot.len(), 1);
    assert_eq!(original_snapshot[0].id, original);
    assert!(
        original_snapshot[0].screen.rows[0]
            .plain_text()
            .starts_with("inactive output")
    );
    assert_eq!(original_snapshot[0].status, TerminalStatus::Running);
    let current_snapshot = sessions.switch_context(Some("other".into()), conversation, &[]);
    assert_eq!(current_snapshot.len(), 1);
    assert_eq!(current_snapshot[0].id, current);
    assert_eq!(backend.terminated.load(Ordering::Relaxed), 0);
}

#[test]
fn workspace_change_rejects_previous_poll_and_switch_results() {
    let (mut sessions, backend) = feature();
    sessions
        .session
        .start(Path::new("workspace"), 80, 24)
        .unwrap();
    let worker = crate::client_adapter::ClientWorker::new(sessions);
    let mut requests = TerminalRequests::default();
    let poll = requests.begin_poll().unwrap();
    let old_snapshot = worker
        .try_request(|sessions| Ok(sessions.session.terminals().to_vec()))
        .unwrap();
    requests.invalidate();
    let switched = requests.begin_action();
    let next_snapshot = worker
        .try_request(|sessions| {
            Ok(sessions.switch_context(Some("other".into()), ConversationId::new("first"), &[]))
        })
        .unwrap();
    assert_eq!(
        old_snapshot
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap()
            .unwrap()
            .len(),
        1
    );
    assert!(!requests.finish_poll(poll));
    let next_poll = requests
        .begin_poll()
        .expect("polling does not wait for the switch action");
    assert!(requests.finish_poll(next_poll));
    assert!(
        next_snapshot
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap()
            .unwrap()
            .is_empty()
    );
    requests.gate.invalidate();
    let closed = requests.begin_action();
    let closed_snapshot = worker
        .try_request(|sessions| {
            Ok(sessions.switch_context(None, ConversationId::new("first"), &[]))
        })
        .unwrap();
    assert!(!requests.finish_action(switched));
    assert!(
        closed_snapshot
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap()
            .unwrap()
            .is_empty()
    );
    assert!(requests.finish_action(closed));
    assert!(requests.begin_poll().is_some());
    assert_eq!(backend.terminated.load(Ordering::Relaxed), 0);
}

#[test]
fn conversation_switch_returns_idle_and_restored_terminals_without_new_events() {
    let (mut sessions, backend) = feature();
    let terminal = sessions
        .session
        .start(Path::new("workspace"), 80, 24)
        .unwrap();
    let summaries = [RestoredTerminal {
        id: None,
        cwd: "saved".into(),
        status: TerminalStatus::Running,
        columns: 120,
        rows: 30,
    }];
    let restored = sessions.switch_context(
        Some("workspace".into()),
        ConversationId::new("second"),
        &summaries,
    );
    assert!(!sessions.refresh().unwrap());
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].cwd, Path::new("saved"));
    assert_eq!(restored[0].status, TerminalStatus::Exited);
    let resumed =
        sessions.switch_context(Some("workspace".into()), ConversationId::new("first"), &[]);
    assert!(!sessions.refresh().unwrap());
    assert_eq!(resumed.len(), 1);
    assert_eq!(resumed[0].id, terminal);
    assert_eq!(resumed[0].status, TerminalStatus::Running);
    assert_eq!(backend.terminated.load(Ordering::Relaxed), 0);
}

#[test]
fn rapid_conversation_switches_only_project_the_latest_snapshot() {
    let (mut sessions, backend) = feature();
    let terminal = sessions
        .session
        .start(Path::new("workspace"), 80, 24)
        .unwrap();
    let worker = crate::client_adapter::ClientWorker::new(sessions);
    let mut requests = TerminalRequests::default();
    requests.gate.invalidate();
    let first = requests.begin_action();
    let first_response = worker
        .try_request(|sessions| {
            Ok(sessions.switch_context(
                Some("workspace".into()),
                ConversationId::new("second"),
                &[],
            ))
        })
        .unwrap();
    requests.gate.invalidate();
    let second = requests.begin_action();
    let second_response =
        worker
            .try_request(|sessions| {
                Ok(sessions.switch_context(
                    Some("workspace".into()),
                    ConversationId::new("first"),
                    &[],
                ))
            })
            .unwrap();
    let poll = requests
        .begin_poll()
        .expect("polling does not wait for pending switches");
    assert!(requests.finish_poll(poll));
    assert!(
        first_response
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap()
            .unwrap()
            .is_empty()
    );
    assert!(!requests.finish_action(first));
    let poll = requests
        .begin_poll()
        .expect("polling does not wait for pending switches");
    assert!(requests.finish_poll(poll));
    let terminals = second_response
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap()
        .unwrap();
    assert!(requests.finish_action(second));
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].id, terminal);
    assert_eq!(terminals[0].status, TerminalStatus::Running);
    assert!(requests.begin_poll().is_some());
    assert_eq!(backend.terminated.load(Ordering::Relaxed), 0);
}

#[test]
fn switching_conversations_retains_live_terminal_identity() {
    let (mut feature, backend) = feature();
    let terminal = feature
        .session
        .start(Path::new("workspace"), 80, 24)
        .unwrap();
    feature.switch_context(Some("workspace".into()), ConversationId::new("second"), &[]);
    assert!(feature.session.terminals().is_empty());
    assert!(feature.has_running_process());
    feature.switch_context(Some("workspace".into()), ConversationId::new("first"), &[]);
    assert_eq!(feature.session.terminals()[0].id, terminal);
    assert_eq!(
        feature.session.terminals()[0].status,
        TerminalStatus::Running
    );
    assert_eq!(backend.terminated.load(Ordering::Relaxed), 0);
}

#[test]
fn inactive_terminal_output_and_exit_are_consumed() {
    let (mut feature, backend) = feature();
    let terminal = feature
        .session
        .start(Path::new("workspace"), 80, 24)
        .unwrap();
    feature.switch_context(Some("workspace".into()), ConversationId::new("second"), &[]);
    assert!(feature.live_sessions.load(Ordering::Acquire));
    backend.events.lock().unwrap().insert(
        terminal.as_str().to_owned(),
        vec![
            TerminalEvent::Delta(model_delta(1, TerminalSize::new(80, 24), "background")),
            TerminalEvent::Exited { code: Some(0) },
        ],
    );
    assert!(feature.refresh().unwrap());
    assert!(!feature.live_sessions.load(Ordering::Acquire));
    feature.switch_context(Some("workspace".into()), ConversationId::new("first"), &[]);
    assert!(
        feature.session.terminals()[0].screen.rows[0]
            .plain_text()
            .starts_with("background")
    );
    assert_eq!(
        feature.session.terminals()[0].status,
        TerminalStatus::Exited
    );
}

#[test]
fn summaries_never_replace_live_sessions_and_drop_is_not_termination() {
    let (mut feature, backend) = feature();
    let terminal = feature
        .session
        .start(Path::new("workspace"), 80, 24)
        .unwrap();
    let summaries = [RestoredTerminal {
        id: None,
        cwd: "old".into(),
        status: TerminalStatus::Running,
        columns: 40,
        rows: 10,
    }];
    feature.switch_context(
        Some("workspace".into()),
        ConversationId::new("second"),
        &summaries,
    );
    assert_eq!(
        feature.session.terminals()[0].status,
        TerminalStatus::Exited
    );
    feature.switch_context(
        Some("workspace".into()),
        ConversationId::new("first"),
        &summaries,
    );
    assert_eq!(feature.session.terminals()[0].id, terminal);
    drop(feature);
    assert_eq!(backend.terminated.load(Ordering::Relaxed), 0);
}

#[gpui::test]
fn retries_a_temporarily_unavailable_profile_catalog(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext;
    let (entity, backend) = cx.update(|cx| {
        let (_, backend) = feature();
        backend.fail_profiles.store(true, Ordering::Relaxed);
        *backend.profiles.lock().unwrap() = terminal::TerminalProfiles::new(
            vec![terminal::TerminalProfile::new(
                "cmd",
                "Command Prompt",
                "cmd.exe",
            )],
            Some(terminal::TerminalProfileId::new("cmd")),
        );
        let entity = cx.new(|cx| {
            TerminalFeature::test_new(
                TerminalSession::new(backend.clone()),
                Some("workspace".into()),
                ConversationId::new("task-a"),
                cx,
            )
        });
        entity.update(cx, |feature, cx| {
            feature.default_profile = Some(terminal::TerminalProfileId::new("cmd"));
            feature.sync_tab_presence(true, cx);
        });
        (entity, backend)
    });
    cx.run_until_parked();
    cx.update(|cx| {
        assert!(entity.read(cx).profiles_retry);
        assert!(backend.started_profiles.lock().unwrap().is_empty());
        backend.fail_profiles.store(false, Ordering::Relaxed);
        entity.update(cx, |feature, cx| feature.note_change(cx));
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let feature = entity.read(cx);
        assert!(!feature.profiles_retry);
        assert_eq!(feature.profiles.profiles[0].id.as_str(), "cmd");
    });
    assert_eq!(
        *backend.started_profiles.lock().unwrap(),
        vec![Some(terminal::TerminalProfileId::new("cmd"))]
    );
}

#[gpui::test]
fn opening_the_terminal_tab_auto_starts_the_configured_default_profile(
    cx: &mut gpui::TestAppContext,
) {
    use gpui::AppContext;
    let opened = cx.update(|cx| {
        let (_, backend) = feature();
        *backend.profiles.lock().unwrap() = terminal::TerminalProfiles::new(
            vec![
                terminal::TerminalProfile::new("powershell", "PowerShell", "powershell.exe"),
                terminal::TerminalProfile::new("cmd", "Command Prompt", "cmd.exe"),
            ],
            Some(terminal::TerminalProfileId::new("powershell")),
        );
        let entity = cx.new(|cx| {
            TerminalFeature::test_new(
                TerminalSession::new(backend.clone()),
                Some("workspace".into()),
                ConversationId::new("task-a"),
                cx,
            )
        });
        // 設定で選ばれた既定 profile を Host 公開後に解決できるようにする。
        assert!(
            backend.started_profiles.lock().unwrap().is_empty(),
            "表示前には起動しない"
        );
        entity.update(cx, |feature, cx| {
            feature.default_profile = Some(terminal::TerminalProfileId::new("cmd"));
            feature.load_profiles(cx);
            feature.sync_tab_presence(true, cx);
        });
        (entity, backend)
    });
    let (entity, backend) = opened;
    cx.run_until_parked();
    cx.update(|cx| {
        entity.update(cx, |feature, _| {
            assert_eq!(feature.terminals.len(), 1, "既定 profile で自動起動する");
        });
    });
    assert_eq!(
        *backend.started_profiles.lock().unwrap(),
        vec![Some(terminal::TerminalProfileId::new("cmd"))],
        "設定で選んだ profile を使う"
    );

    // 表示が続くだけでは新しい shell を起動しない。
    cx.update(|cx| {
        entity.update(cx, |feature, cx| {
            feature.sync_tab_presence(true, cx);
        });
    });
    cx.run_until_parked();
    assert_eq!(backend.started_profiles.lock().unwrap().len(), 1);
}

#[gpui::test]
fn auto_start_falls_back_to_the_host_default_when_the_setting_is_unknown(
    cx: &mut gpui::TestAppContext,
) {
    use gpui::AppContext;
    let (entity, backend) = cx.update(|cx| {
        let (_, backend) = feature();
        *backend.profiles.lock().unwrap() = terminal::TerminalProfiles::new(
            vec![terminal::TerminalProfile::new(
                "powershell",
                "PowerShell",
                "powershell.exe",
            )],
            Some(terminal::TerminalProfileId::new("powershell")),
        );
        let entity = cx.new(|cx| {
            TerminalFeature::test_new(
                TerminalSession::new(backend.clone()),
                Some("workspace".into()),
                ConversationId::new("task-a"),
                cx,
            )
        });
        entity.update(cx, |feature, cx| {
            feature.default_profile = Some(terminal::TerminalProfileId::new("removed"));
            feature.load_profiles(cx);
            feature.sync_tab_presence(true, cx);
        });
        (entity, backend)
    });
    cx.run_until_parked();
    assert_eq!(
        *backend.started_profiles.lock().unwrap(),
        vec![None],
        "存在しない profile は Host 既定へ倒す"
    );
    cx.update(|cx| {
        entity.update(cx, |feature, _| assert_eq!(feature.terminals.len(), 1));
    });
}

#[gpui::test]
fn two_tab_instances_own_independent_sessions_and_closing_one_keeps_the_other(
    cx: &mut gpui::TestAppContext,
) {
    use gpui::AppContext;
    // 同じ backend を共有しても、タブごとの実体は別の Session を持つ。
    let (first, second, backend) = cx.update(|cx| {
        let backend = Arc::new(Backend::default());
        let mut first_session = TerminalSession::new(backend.clone());
        first_session.start(Path::new("workspace"), 80, 24).unwrap();
        let mut second_session = TerminalSession::new(backend.clone());
        second_session
            .start(Path::new("workspace"), 80, 24)
            .unwrap();
        let first = cx.new(|cx| {
            TerminalFeature::test_new(
                first_session,
                Some("workspace".into()),
                ConversationId::new("task-a"),
                cx,
            )
        });
        let second = cx.new(|cx| {
            TerminalFeature::test_new(
                second_session,
                Some("workspace".into()),
                ConversationId::new("task-a"),
                cx,
            )
        });
        for feature in [&first, &second] {
            feature.update(cx, |feature, _| feature.auto_start = AutoStart::Finished);
        }
        (first, second, backend)
    });
    // 2つのタブは別々の Terminal を表示する。
    let ids = cx.update(|cx| {
        (
            first.read(cx).terminals[0].id.clone(),
            second.read(cx).terminals[0].id.clone(),
        )
    });
    assert_ne!(ids.0, ids.1, "タブごとに独立したセッション");

    // 片方のタブを閉じても、もう片方の PTY は終了しない。
    cx.update(|cx| second.update(cx, |feature, cx| feature.close_session(cx)));
    cx.run_until_parked();
    assert_eq!(
        backend.terminated.load(Ordering::Relaxed),
        1,
        "閉じたタブの PTY だけを終了する"
    );
    cx.update(|cx| {
        assert!(!first.read(cx).terminals.is_empty(), "他のタブは残る");
        assert!(second.read(cx).terminals.is_empty());
    });
}

#[gpui::test]
fn closing_the_terminal_tab_terminates_the_session_and_detach_does_not(
    cx: &mut gpui::TestAppContext,
) {
    use gpui::AppContext;
    let (entity, backend) = cx.update(|cx| {
        let backend = Arc::new(Backend::default());
        let mut session = TerminalSession::new(backend.clone());
        session.start(Path::new("workspace"), 80, 24).unwrap();
        let entity = cx.new(|cx| {
            TerminalFeature::test_new(
                session,
                Some("workspace".into()),
                ConversationId::new("task-a"),
                cx,
            )
        });
        entity.update(cx, |feature, _| feature.auto_start = AutoStart::Finished);
        (entity, backend)
    });
    // Workspace の detach 相当。セッションは残り、PTY も終了しない。
    cx.update(|cx| {
        entity.update(cx, |feature, cx| {
            feature.set_context(
                Some("other".into()),
                ConversationId::new("task-b"),
                Vec::new(),
                cx,
            );
        });
    });
    cx.run_until_parked();
    cx.update(|cx| {
        entity.update(cx, |feature, cx| {
            feature.set_context(
                Some("workspace".into()),
                ConversationId::new("task-a"),
                Vec::new(),
                cx,
            );
        });
    });
    cx.run_until_parked();
    assert_eq!(
        backend.terminated.load(Ordering::Relaxed),
        0,
        "コンテキスト切替（detach）では PTY を終了しない"
    );
    cx.update(|cx| {
        entity.update(cx, |feature, _| assert_eq!(feature.terminals.len(), 1));
    });
    // タブを閉じた操作だけがセッションを終了する。
    cx.update(|cx| {
        entity.update(cx, |feature, cx| feature.close_session(cx));
    });
    cx.run_until_parked();
    cx.update(|cx| {
        entity.update(cx, |feature, _| {
            assert!(feature.terminals.is_empty(), "閉じたタブは表示から消える");
        });
    });
    assert_eq!(
        backend.terminated.load(Ordering::Relaxed),
        1,
        "タブを閉じたら PTY も終了する"
    );
}

#[gpui::test]
fn reopening_the_tab_before_the_close_response_still_auto_starts(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext;
    let (entity, backend) = cx.update(|cx| {
        let backend = Arc::new(Backend::default());
        let mut session = TerminalSession::new(backend.clone());
        session.start(Path::new("workspace"), 80, 24).unwrap();
        let entity = cx.new(|cx| {
            TerminalFeature::test_new(
                session,
                Some("workspace".into()),
                ConversationId::new("task-a"),
                cx,
            )
        });
        entity.update(cx, |feature, cx| {
            feature.auto_start = AutoStart::Finished;
            feature.close_session(cx);
            // 終了の応答より前にタブを開き直す。
            // ここで pending_auto_start を消費しても、応答後にやり直す。
            feature.sync_tab_presence(true, cx);
            assert!(feature.tab_visible);
        });
        (entity, backend)
    });
    cx.run_until_parked();
    cx.update(|cx| {
        entity.update(cx, |feature, _| {
            assert_eq!(feature.terminals.len(), 1, "表示中のタブを空のまま残さない");
            assert_eq!(feature.auto_start, AutoStart::Finished);
        });
    });
    assert_eq!(
        backend.started_profiles.lock().unwrap().len(),
        2,
        "閉じた後に開き直したタブは既定 profile で起動し直す"
    );
}

#[gpui::test]
fn releasing_a_stale_resource_allows_the_visible_tab_to_auto_start(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext;
    let feature = cx.update(|cx| {
        let (_, backend) = feature();
        cx.new(|cx| {
            TerminalFeature::test_new(
                TerminalSession::new(backend),
                Some("workspace".into()),
                ConversationId::new("task-a"),
                cx,
            )
        })
    });
    cx.update(|cx| {
        feature.update(cx, |feature, _| {
            // 前の Task の Terminal を固定参照として持つ。
            feature.bind_resource(Some(terminal::TerminalId::new("terminal-a")));
            feature.tab_visible = true;
            feature.auto_start = AutoStart::Finished;
            feature.pending_auto_start = false;

            feature.release_resource();

            // 参照先が消えたら、表示中のタブで改めて自動起動を判定できる。
            assert!(feature.bound_resource.is_none());
            assert_eq!(feature.auto_start, AutoStart::Idle);
            assert!(feature.pending_auto_start);

            // 2 回目以降は状態を変えない。表示中のタブの判定を勝手に戻さない。
            feature.auto_start = AutoStart::Finished;
            feature.pending_auto_start = false;
            feature.release_resource();
            assert_eq!(feature.auto_start, AutoStart::Finished);
            assert!(!feature.pending_auto_start);
        });
    });
}

#[gpui::test]
fn long_session_list_menu_stays_within_the_window(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        crate::theme::init(&extension_ui_contract::ThemeId::new("lapis.dark"), None, cx)
    });
    let (feature, visual) = cx.add_window_view(|_, cx| {
        let (_, backend) = feature();
        TerminalFeature::test_new(
            TerminalSession::new(backend),
            Some("workspace".into()),
            ConversationId::new("task-a"),
            cx,
        )
    });
    feature.update(visual, |feature, cx| {
        // 1 行 30px では Window の高さに収まらない件数のセッション一覧を開く。
        feature.terminals = (0..40)
            .map(|index| {
                terminal::TerminalSnapshot::empty(
                    terminal::TerminalId::new(format!("terminal-{index:02}")),
                    "workspace".into(),
                    TerminalStatus::Running,
                    TerminalSize::new(80, 24),
                )
            })
            .collect();
        feature.context_menu = Some(());
        feature.context_anchor = Some(gpui::point(gpui::px(8.0), gpui::px(8.0)));
        cx.notify();
    });
    visual.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear();
    });
    let bounds = visual
        .debug_bounds("terminal-context-menu")
        .expect("context menu must be drawn");
    // 共通 wrapper の上限（450px）で高さを切り、残りはスクロールさせる。
    assert!(
        bounds.size.height <= gpui::px(450.),
        "メニューの高さは共通の上限に収める"
    );
    assert!(
        bounds.size.height < gpui::px(40. * 30.),
        "全項目を一度に描画せず、高さを制限してスクロールさせる"
    );
}
