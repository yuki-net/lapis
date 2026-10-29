use std::collections::VecDeque;

use gpui::{App, AppContext, Context, Entity, Global};
use lapis_desktop_client::SettingsSession;
use lapis_localization::LocaleId;
use lapis_settings::{GlobalSettings, SyntaxTheme};

use crate::{client_adapter::ClientWorker, extension_ui::ThemeId, theme};

const CAPACITY: usize = 64;

#[derive(Clone, Debug)]
pub(crate) enum Change {
    Theme(String),
    Syntax(SyntaxTheme),
    Locale(LocaleId),
}

impl Change {
    fn apply(&self, values: &mut GlobalSettings) {
        match self {
            Self::Theme(id) => values.theme = id.clone(),
            Self::Syntax(id) => values.syntax_theme = Some(*id),
            Self::Locale(id) => values.locale = id.clone(),
        }
    }

    fn save(&self, session: &SettingsSession) -> Result<(), lapis_settings::SettingsError> {
        match self {
            Self::Theme(id) => session.set_theme(id.clone()),
            Self::Syntax(id) => session.set_syntax_theme(*id),
            Self::Locale(id) => session.set_locale(id.clone()),
        }
    }
}

struct SettingsGlobal(Entity<SettingsCoordinator>);
impl Global for SettingsGlobal {}

pub(crate) struct SettingsCoordinator {
    worker: ClientWorker<SettingsSession>,
    confirmed: GlobalSettings,
    pending: VecDeque<Change>,
    active: bool,
    quit_when_idle: bool,
    pub(crate) values: GlobalSettings,
    pub(crate) message: String,
}

impl SettingsCoordinator {
    pub(crate) fn init(session: SettingsSession, cx: &mut App) {
        let values = session.settings();
        let entity = cx.new(|_| Self {
            worker: ClientWorker::new(session),
            confirmed: values.clone(),
            pending: VecDeque::new(),
            active: false,
            quit_when_idle: false,
            values,
            message: String::new(),
        });
        cx.set_global(SettingsGlobal(entity));
    }

    pub(crate) fn shared(cx: &App) -> Entity<Self> {
        cx.global::<SettingsGlobal>().0.clone()
    }

    pub(crate) fn quit_after_pending(&mut self, cx: &mut Context<Self>) {
        self.quit_when_idle = true;
        self.finish_quit(cx);
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
                this.start_next(cx);
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

    fn publish(&mut self, cx: &mut Context<Self>) {
        self.values = self.confirmed.clone();
        for change in &self.pending {
            change.apply(&mut self.values);
        }
        theme::set_active(&ThemeId::new(self.values.theme.clone()), cx);
        theme::syntax::set_active(self.values.syntax_theme, cx);
        cx.refresh_windows();
        cx.notify();
        self.finish_quit(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lapis_settings::{GlobalSettingsRepository, SettingsError};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    #[derive(Default)]
    struct Repository {
        saved: Mutex<Vec<GlobalSettings>>,
        fail_at: usize,
    }
    impl GlobalSettingsRepository for Repository {
        fn load(&self) -> Result<GlobalSettings, SettingsError> {
            Ok(GlobalSettings::default())
        }
        fn save(&self, value: &GlobalSettings) -> Result<(), SettingsError> {
            let mut saved = self.saved.lock().unwrap();
            saved.push(value.clone());
            if self.fail_at == saved.len() {
                return Err(SettingsError::new("save failed"));
            }
            Ok(())
        }
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

    #[gpui::test]
    fn reverse_and_repeated_changes_keep_request_order(cx: &mut gpui::TestAppContext) {
        let repository = Arc::new(Repository::default());
        let coordinator = setup(cx, repository.clone());
        cx.update(|cx| {
            coordinator.update(cx, |state, cx| {
                state.change(Change::Syntax(SyntaxTheme::Zed), cx);
                state.change(Change::Theme("lapis.white".into()), cx);
                state.change(Change::Theme("lapis.dark".into()), cx);
                state.change(Change::Theme("lapis.white".into()), cx);
            })
        });
        drain(cx, &coordinator);
        cx.update(|cx| {
            let state = coordinator.read(cx);
            assert_eq!(state.values.theme, "lapis.white");
            assert_eq!(state.values.syntax_theme, Some(SyntaxTheme::Zed));
        });
        let saved = repository.saved.lock().unwrap();
        assert_eq!(saved.len(), 4);
        assert_eq!(saved.last().unwrap().theme, "lapis.white");
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
                state.change(Change::Theme("lapis.white".into()), cx);
                state.change(Change::Syntax(SyntaxTheme::Zed), cx);
            })
        });
        drain(cx, &coordinator);
        cx.update(|cx| {
            let state = coordinator.read(cx);
            assert_eq!(state.values.theme, "lapis.dark");
            assert_eq!(state.values.syntax_theme, Some(SyntaxTheme::Zed));
        });
        assert_eq!(repository.saved.lock().unwrap().len(), 2);
    }

    #[gpui::test]
    fn full_queue_rejects_without_changing_optimistic_state(cx: &mut gpui::TestAppContext) {
        let coordinator = setup(cx, Arc::new(Repository::default()));
        cx.update(|cx| {
            coordinator.update(cx, |state, cx| {
                for _ in 0..CAPACITY {
                    state.change(Change::Syntax(SyntaxTheme::Zed), cx);
                }
                state.change(Change::Theme("lapis.white".into()), cx);
                assert_eq!(state.values.theme, "lapis.dark");
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
                feature.select_theme(ThemeId::new("lapis.white"), cx);
            });
            drop(feature);
        });
        drain(cx, &coordinator);
        assert_eq!(
            repository.saved.lock().unwrap().last().unwrap().theme,
            "lapis.white"
        );
        cx.update(|cx| assert_eq!(coordinator.read(cx).confirmed.theme, "lapis.white"));
    }

    #[gpui::test]
    fn last_window_quit_waits_for_accepted_saves(cx: &mut gpui::TestAppContext) {
        let repository = Arc::new(Repository::default());
        let coordinator = setup(cx, repository.clone());
        cx.update(|cx| {
            coordinator.update(cx, |state, cx| {
                state.change(Change::Theme("lapis.white".into()), cx);
                state.change(Change::Syntax(SyntaxTheme::Zed), cx);
                state.quit_after_pending(cx);
                assert!(state.quit_when_idle);
                assert_eq!(state.pending.len(), 2);
            })
        });
        drain(cx, &coordinator);
        cx.update(|cx| {
            let state = coordinator.read(cx);
            assert!(!state.quit_when_idle);
            assert_eq!(state.confirmed.theme, "lapis.white");
            assert_eq!(state.confirmed.syntax_theme, Some(SyntaxTheme::Zed));
        });
        assert_eq!(repository.saved.lock().unwrap().len(), 2);
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
                state.change(Change::Syntax(SyntaxTheme::Zed), cx);
                state.change(Change::Theme("lapis.white".into()), cx);
                state.change(Change::Locale(LocaleId::new("ja-JP")), cx);
            })
        });
        drain(cx, &coordinator);
        cx.update(|cx| {
            let values = &coordinator.read(cx).values;
            assert_eq!(values.theme, "lapis.dark");
            assert_eq!(values.syntax_theme, Some(SyntaxTheme::Zed));
            assert_eq!(values.locale.as_str(), "ja-JP");
        });
    }
}
