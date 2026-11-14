use std::collections::VecDeque;

use desktop_client::{SettingsSession, SettingsWatch};
use gpui::{App, AppContext, Context, Entity, Global};
use settings::{ResolvedSettings, SettingKey, SettingValue, SettingsError, SettingsScope};

use crate::{client_adapter::ClientWorker, theme};
use extension_ui_contract::ThemeId;

const CAPACITY: usize = 64;

/// 1つのユーザー操作としてまとめて保存する設定変更。
#[derive(Clone, Debug)]
pub(crate) struct Change {
    /// `None` は保存値を消して既定値へ戻す。
    values: Vec<(SettingKey, Option<SettingValue>)>,
}

impl Change {
    pub(crate) fn set(key: SettingKey, value: SettingValue) -> Self {
        Self {
            values: vec![(key, Some(value))],
        }
    }

    /// 保存値を消して既定値へ戻す。
    pub(crate) fn clear(key: SettingKey) -> Self {
        Self {
            values: vec![(key, None)],
        }
    }

    /// 複数項目を 1 回の保存にまとめる。`None` は既定値へ戻す。
    pub(crate) fn update(values: Vec<(SettingKey, Option<SettingValue>)>) -> Self {
        Self { values }
    }

    /// 表示だけ先に反映し、保存が終われば共有 state の値に置き換わる。
    fn apply(&self, values: &mut ResolvedSettings) {
        for (key, value) in &self.values {
            match value {
                Some(value) => values.overlay(SettingsScope::User, *key, value.clone()),
                None => values.clear_overlay(*key),
            }
        }
    }

    fn save(&self, session: &SettingsSession) -> Result<(), SettingsError> {
        session.update(&self.values)
    }
}

struct SettingsGlobal(Entity<SettingsCoordinator>);
impl Global for SettingsGlobal {}

pub(crate) struct SettingsCoordinator {
    worker: ClientWorker<SettingsSession>,
    /// 保存済みの実効値。購読で更新される。
    confirmed: ResolvedSettings,
    watch: SettingsWatch,
    pending: VecDeque<Change>,
    active: bool,
    quit_when_idle: bool,
    shutting_down: bool,
    pub(crate) values: ResolvedSettings,
    pub(crate) message: String,
}

impl SettingsCoordinator {
    pub(crate) fn init(session: SettingsSession, cx: &mut App) {
        let values = session.settings();
        let watch = session.subscribe();
        let entity = cx.new(|cx: &mut Context<Self>| {
            cx.on_app_quit(|this, _| this.wait_for_shutdown_saves())
                .detach();
            Self {
                worker: ClientWorker::new(session),
                confirmed: values.clone(),
                watch,
                pending: VecDeque::new(),
                active: false,
                quit_when_idle: false,
                shutting_down: false,
                values,
                message: String::new(),
            }
        });
        cx.set_global(SettingsGlobal(entity));
    }

    #[cfg(test)]
    pub(super) fn test_is_idle(&self) -> bool {
        self.pending.is_empty()
    }

    pub(crate) fn shared(cx: &App) -> Entity<Self> {
        cx.global::<SettingsGlobal>().0.clone()
    }

    /// 設定が未初期化の環境（テストなど）では `None`。
    pub(crate) fn try_shared(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<SettingsGlobal>()
            .map(|global| global.0.clone())
    }

    #[cfg(any(not(target_family = "wasm"), test))]
    pub(crate) fn quit_after_pending(&mut self, cx: &mut Context<Self>) {
        self.quit_when_idle = true;
        self.finish_quit(cx);
    }

    fn wait_for_shutdown_saves(&mut self) -> impl std::future::Future<Output = ()> + use<> {
        let pending = !self.shutting_down && !self.pending.is_empty();
        self.shutting_down = true;
        self.quit_when_idle = false;
        // 先頭は既にworkerへ渡されている。残りはforeground taskを介さず引き継ぐ。
        let remaining: Vec<_> = self
            .pending
            .iter()
            .skip(usize::from(self.active))
            .cloned()
            .collect();
        let worker = self.worker.clone();
        let receiver = pending.then(|| {
            worker.request(move |session| {
                let mut failure = None;
                for change in remaining {
                    if let Err(error) = change.save(session) {
                        failure = Some(error.to_string());
                    }
                }
                failure.map_or(Ok(()), Err)
            })
        });
        async move {
            let _worker = worker;
            let Some(receiver) = receiver else { return };
            loop {
                match receiver.try_recv() {
                    Ok(Ok(())) | Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
                    Ok(Err(error)) => {
                        eprintln!("終了時の設定保存に失敗しました: {error}");
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        crate::timer::Timer::after(std::time::Duration::from_millis(5)).await;
                    }
                }
            }
        }
    }

    fn finish_quit(&mut self, cx: &mut Context<Self>) {
        if self.quit_when_idle && self.pending.is_empty() {
            self.quit_when_idle = false;
            if cx.windows().is_empty() {
                cx.quit();
            }
        }
    }

    pub(crate) fn change(&mut self, change: Change, cx: &mut Context<Self>) {
        if self.shutting_down {
            return;
        }
        if self.pending.len() >= CAPACITY {
            self.message = "設定の処理待ちが上限に達しました".into();
            cx.notify();
            return;
        }
        self.pending.push_back(change);
        if !self.active && !self.start_next(cx) {
            return;
        }
        self.message.clear();
        self.publish(cx);
    }

    fn start_next(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(change) = self.pending.front().cloned() else {
            self.active = false;
            return true;
        };
        let admitted = self.worker.submit(
            cx,
            move |session| {
                let result = change.save(session).map_err(|error| error.to_string());
                Ok((result, session.settings()))
            },
            |this, result, cx| {
                this.pending.pop_front();
                match result {
                    Ok((outcome, values)) => {
                        this.confirmed = values;
                        this.message = outcome.err().unwrap_or_default();
                    }
                    Err(error) => this.message = error,
                }
                this.active = false;
                if !this.shutting_down {
                    this.start_next(cx);
                }
                this.publish(cx);
            },
        );
        match admitted {
            Ok(()) => {
                self.active = true;
                true
            }
            Err(error) => {
                self.message = error;
                self.pending.clear();
                self.active = false;
                self.publish(cx);
                false
            }
        }
    }

    /// 複数の設定を1回の保存でまとめて更新する。`None` は既定値へ戻す。
    pub(crate) fn clear(&mut self, key: SettingKey, cx: &mut Context<Self>) {
        self.change(Change::clear(key), cx)
    }

    fn publish(&mut self, cx: &mut Context<Self>) {
        // coordinator を経由しない保存（別 Session）も購読から取り込む。
        while let Ok(snapshot) = self.watch.try_recv() {
            self.confirmed = snapshot;
        }
        self.values = self.confirmed.clone();
        for change in &self.pending {
            change.apply(&mut self.values);
        }
        theme::set_active(&ThemeId::new(self.values.theme().to_owned()), cx);
        theme::syntax::set_active(self.values.syntax_theme(), cx);
        cx.refresh_windows();
        cx.notify();
        self.finish_quit(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::Instant;
    use settings::{SettingsDocument, SettingsRepository, UI_SYNTAX_THEME, UI_THEME};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[derive(Default)]
    struct Repository {
        saved: Mutex<Vec<SettingsDocument>>,
        fail_at: usize,
    }

    impl SettingsRepository for Repository {
        fn load(&self) -> Result<SettingsDocument, SettingsError> {
            Ok(SettingsDocument::empty(SettingsScope::User))
        }

        fn save(&self, value: &SettingsDocument) -> Result<(), SettingsError> {
            let mut saved = self.saved.lock().unwrap();
            saved.push(value.clone());
            if self.fail_at == saved.len() {
                return Err(SettingsError::new("save failed"));
            }
            Ok(())
        }
    }

    fn theme_change(theme: &str) -> Change {
        Change::set(UI_THEME, SettingValue::text(theme))
    }

    fn syntax_change(theme: settings::SyntaxTheme) -> Change {
        Change::set(UI_SYNTAX_THEME, SettingValue::text(theme.as_id()))
    }

    fn setup(
        cx: &mut gpui::TestAppContext,
        repository: Arc<Repository>,
    ) -> Entity<SettingsCoordinator> {
        cx.update(|cx| {
            theme::init(&ThemeId::new("lapis.dark"), None, cx);
            SettingsCoordinator::init(SettingsSession::load(repository).unwrap(), cx);
            SettingsCoordinator::shared(cx)
        })
    }

    fn drain(cx: &mut gpui::TestAppContext, coordinator: &Entity<SettingsCoordinator>) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while cx.update(|cx| !coordinator.read(cx).pending.is_empty()) {
            assert!(Instant::now() < deadline, "settings queue did not drain");
            cx.executor().advance_clock(Duration::from_millis(20));
            cx.run_until_parked();
            std::thread::yield_now();
        }
    }

    fn stored_theme(document: &SettingsDocument) -> Option<&str> {
        document.get(UI_THEME).and_then(SettingValue::as_text)
    }

    #[gpui::test]
    fn reverse_and_repeated_changes_keep_request_order(cx: &mut gpui::TestAppContext) {
        let repository = Arc::new(Repository::default());
        let coordinator = setup(cx, repository.clone());
        cx.update(|cx| {
            coordinator.update(cx, |state, cx| {
                state.change(syntax_change(settings::SyntaxTheme::Zed), cx);
                state.change(theme_change("lapis.white"), cx);
                state.change(theme_change("lapis.dark"), cx);
                state.change(theme_change("lapis.white"), cx);
            })
        });
        drain(cx, &coordinator);
        cx.update(|cx| {
            let state = coordinator.read(cx);
            assert_eq!(state.values.theme(), "lapis.white");
            assert_eq!(
                state.values.syntax_theme(),
                Some(settings::SyntaxTheme::Zed)
            );
        });
        let saved = repository.saved.lock().unwrap();
        assert_eq!(saved.len(), 4);
        assert_eq!(stored_theme(saved.last().unwrap()), Some("lapis.white"));
    }

    #[gpui::test]
    fn failure_replays_later_pending_changes(cx: &mut gpui::TestAppContext) {
        let repository = Arc::new(Repository {
            fail_at: 1,
            ..Default::default()
        });
        let coordinator = setup(cx, repository.clone());
        cx.update(|cx| {
            coordinator.update(cx, |state, cx| {
                state.change(theme_change("lapis.white"), cx);
                state.change(syntax_change(settings::SyntaxTheme::Zed), cx);
            })
        });
        drain(cx, &coordinator);
        cx.update(|cx| {
            let state = coordinator.read(cx);
            assert_eq!(state.values.theme(), "lapis.dark");
            assert_eq!(
                state.values.syntax_theme(),
                Some(settings::SyntaxTheme::Zed)
            );
        });
        assert_eq!(repository.saved.lock().unwrap().len(), 2);
    }

    #[gpui::test]
    fn full_queue_rejects_without_changing_optimistic_state(cx: &mut gpui::TestAppContext) {
        let coordinator = setup(cx, Arc::new(Repository::default()));
        cx.update(|cx| {
            coordinator.update(cx, |state, cx| {
                for _ in 0..CAPACITY {
                    state.change(syntax_change(settings::SyntaxTheme::Zed), cx);
                }
                state.change(theme_change("lapis.white"), cx);
                assert_eq!(state.values.theme(), "lapis.dark");
                assert!(!state.message.is_empty());
                assert_eq!(state.pending.len(), CAPACITY);
            })
        });
        drain(cx, &coordinator);
    }

    #[gpui::test]
    fn pending_save_survives_feature_release(cx: &mut gpui::TestAppContext) {
        let repository = Arc::new(Repository::default());
        let coordinator = setup(cx, repository.clone());
        cx.update(|cx| {
            let feature = cx.new(crate::features::settings::SettingsFeature::new);
            feature.update(cx, |feature, cx| {
                feature.choose(
                    settings::UI_THEME,
                    settings::SettingValue::text("lapis.white"),
                    cx,
                );
            });
            drop(feature);
        });
        drain(cx, &coordinator);
        assert_eq!(
            stored_theme(repository.saved.lock().unwrap().last().unwrap()),
            Some("lapis.white")
        );
        cx.update(|cx| assert_eq!(coordinator.read(cx).confirmed.theme(), "lapis.white"));
    }

    #[gpui::test]
    fn last_window_quit_waits_for_accepted_saves(cx: &mut gpui::TestAppContext) {
        let repository = Arc::new(Repository::default());
        let coordinator = setup(cx, repository.clone());
        cx.update(|cx| {
            coordinator.update(cx, |state, cx| {
                state.change(theme_change("lapis.white"), cx);
                state.change(syntax_change(settings::SyntaxTheme::Zed), cx);
                state.quit_after_pending(cx);
                assert!(state.quit_when_idle);
                assert_eq!(state.pending.len(), 2);
            })
        });
        drain(cx, &coordinator);
        cx.update(|cx| {
            let state = coordinator.read(cx);
            assert!(!state.quit_when_idle);
            assert_eq!(state.confirmed.theme(), "lapis.white");
            assert_eq!(
                state.confirmed.syntax_theme(),
                Some(settings::SyntaxTheme::Zed)
            );
        });
        assert_eq!(repository.saved.lock().unwrap().len(), 2);
    }

    #[gpui::test]
    fn application_quit_waits_for_in_flight_and_queued_saves(cx: &mut gpui::TestAppContext) {
        use std::future::Future;
        use std::sync::mpsc;
        use std::task::{Context as TaskContext, Poll, Waker};

        struct GatedRepository {
            saved: Mutex<Vec<SettingsDocument>>,
            started: mpsc::Sender<()>,
            release: Mutex<mpsc::Receiver<()>>,
        }
        impl SettingsRepository for GatedRepository {
            fn load(&self) -> Result<SettingsDocument, SettingsError> {
                Ok(SettingsDocument::empty(SettingsScope::User))
            }
            fn save(&self, value: &SettingsDocument) -> Result<(), SettingsError> {
                if self.saved.lock().unwrap().is_empty() {
                    self.started.send(()).unwrap();
                    self.release.lock().unwrap().recv().unwrap();
                }
                self.saved.lock().unwrap().push(value.clone());
                Ok(())
            }
        }

        let (started, started_rx) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let repository = Arc::new(GatedRepository {
            saved: Mutex::new(Vec::new()),
            started,
            release: Mutex::new(release_rx),
        });
        let coordinator = cx.update(|cx| {
            theme::init(&ThemeId::new("lapis.dark"), None, cx);
            SettingsCoordinator::init(SettingsSession::load(repository.clone()).unwrap(), cx);
            SettingsCoordinator::shared(cx)
        });
        let mut shutdown = Box::pin(cx.update(|cx| {
            coordinator.update(cx, |state, cx| {
                state.change(theme_change("lapis.white"), cx);
                state.change(syntax_change(settings::SyntaxTheme::Zed), cx);
                state.wait_for_shutdown_saves()
            })
        }));
        started_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let mut task_cx = TaskContext::from_waker(Waker::noop());
        assert!(matches!(
            shutdown.as_mut().poll(&mut task_cx),
            Poll::Pending
        ));
        assert!(repository.saved.lock().unwrap().is_empty());
        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            cx.executor().advance_clock(Duration::from_millis(10));
            cx.run_until_parked();
            if shutdown.as_mut().poll(&mut task_cx).is_ready() {
                break;
            }
            assert!(Instant::now() < deadline, "shutdown did not wait for saves");
            std::thread::yield_now();
        }
        let saved = repository.saved.lock().unwrap();
        assert_eq!(saved.len(), 2);
        assert_eq!(stored_theme(saved.last().unwrap()), Some("lapis.white"));
        assert_eq!(
            saved
                .last()
                .unwrap()
                .get(UI_SYNTAX_THEME)
                .and_then(SettingValue::as_text),
            Some("zed")
        );
    }

    #[gpui::test]
    fn failed_change_does_not_undo_prior_success_or_newer_pending(cx: &mut gpui::TestAppContext) {
        let repository = Arc::new(Repository {
            fail_at: 2,
            ..Default::default()
        });
        let coordinator = setup(cx, repository);
        cx.update(|cx| {
            coordinator.update(cx, |state, cx| {
                state.change(syntax_change(settings::SyntaxTheme::Zed), cx);
                state.change(theme_change("lapis.white"), cx);
                state.change(
                    Change::set(settings::UI_LOCALE, SettingValue::text("ja-JP")),
                    cx,
                );
            })
        });
        drain(cx, &coordinator);
        cx.update(|cx| {
            let values = &coordinator.read(cx).values;
            assert_eq!(values.theme(), "lapis.dark");
            assert_eq!(values.syntax_theme(), Some(settings::SyntaxTheme::Zed));
            assert_eq!(values.locale().as_str(), "ja-JP");
        });
    }
}
