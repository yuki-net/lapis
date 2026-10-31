use std::time::SystemTime;

use crate::extension_ui::CommandId;
use gpui::{App, Context as GpuiContext, Render, Subscription, Window, div, prelude::*};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Context {
    pub origin: Option<String>,
    pub host: Option<String>,
    pub workspace: Option<String>,
    pub task: Option<String>,
    pub worktree: Option<String>,
    pub session: Option<String>,
    pub resource: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action {
    pub label: String,
    pub command: CommandId,
}

#[derive(Clone, Debug)]
pub struct Event {
    pub id: u64,
    pub kind: String,
    pub severity: Severity,
    pub title: String,
    pub message: String,
    pub context: Context,
    pub actions: Vec<Action>,
    pub occurred_at: SystemTime,
    pub read: bool,
}

#[derive(Default)]
pub struct Store {
    next_id: u64,
    events: Vec<Event>,
    popups: Vec<(u64, gpui::WindowId)>,
}

impl gpui::Global for Store {}

const MAX_ACTIONS: usize = 2;

impl Severity {
    pub(crate) fn label(self, locale: &crate::localization::Localizer) -> String {
        let key = match self {
            Self::Info => "notification.severity.info",
            Self::Warning => "notification.severity.warning",
            Self::Error => "notification.severity.error",
        };
        locale.resolve(&lapis_localization::MessageId::new(key))
    }
}

impl Store {
    fn publish(&mut self, mut event: Event) -> u64 {
        event.actions.truncate(MAX_ACTIONS);
        self.next_id = self
            .next_id
            .checked_add(1)
            .expect("notification ID exhausted");
        event.id = self.next_id;
        event.read = false;
        self.events.push(event);
        self.next_id
    }

    pub fn events(&self) -> &[Event] {
        &self.events
    }

    pub fn set_read(&mut self, id: u64, read: bool) {
        if let Some(event) = self.events.iter_mut().find(|event| event.id == id) {
            event.read = read;
        }
    }

    pub fn delete(&mut self, id: u64) {
        self.events.retain(|event| event.id != id);
        self.dismiss_popup(id);
    }

    pub fn popups_for(&self, window: gpui::WindowId) -> Vec<&Event> {
        self.popups
            .iter()
            .rev()
            .filter(|(_, target)| *target == window)
            .take(3)
            .filter_map(|(id, _)| self.events.iter().find(|event| event.id == *id))
            .collect()
    }

    pub fn dismiss_popup(&mut self, id: u64) {
        self.popups.retain(|(current, _)| *current != id);
    }
}

pub fn popup_by_default(kind: &str, severity: Severity) -> bool {
    matches!(
        kind,
        "host.connection_launcher_failed"
            | "window.creation_failed"
            | "workspace.persistence_failed"
    ) || severity == Severity::Error
}

pub fn publish(cx: &mut App, event: Event) {
    let target = cx.active_window().map(|window| window.window_id());
    let show_popup = popup_by_default(&event.kind, event.severity);
    let store = cx.default_global::<Store>();
    let id = store.publish(event);
    if show_popup && let Some(window) = target {
        store.popups.push((id, window));
        cx.spawn(async move |cx| {
            gpui::Timer::after(std::time::Duration::from_secs(6)).await;
            let _ = cx.update(|cx| {
                if cx
                    .global::<Store>()
                    .popups
                    .iter()
                    .any(|(current, _)| *current == id)
                {
                    cx.global_mut::<Store>().dismiss_popup(id);
                }
            });
        })
        .detach();
    }
}

pub fn publish_error(cx: &mut App, kind: &str, title: &str, message: String) {
    publish(
        cx,
        Event {
            id: 0,
            kind: kind.into(),
            severity: Severity::Error,
            title: title.into(),
            message,
            context: Context::default(),
            actions: Vec::new(),
            occurred_at: SystemTime::now(),
            read: false,
        },
    );
}

type CommandExecutor = std::rc::Rc<dyn Fn(CommandId, &mut Window, &mut App)>;

pub(crate) struct Center {
    _subscription: Subscription,
    execute: CommandExecutor,
    locale: crate::localization::Localizer,
}

impl Center {
    pub(crate) fn new(execute: CommandExecutor, cx: &mut GpuiContext<Self>) -> Self {
        cx.default_global::<Store>();
        Self {
            _subscription: cx.observe_global::<Store>(|_, cx| cx.notify()),
            execute,
            locale: crate::localization::Localizer::bundled(),
        }
    }

    pub(crate) fn set_locale(
        &mut self,
        locale: &lapis_localization::LocaleId,
        cx: &mut GpuiContext<Self>,
    ) {
        self.locale.set_active(locale);
        cx.notify();
    }
}

impl Render for Center {
    fn render(&mut self, _: &mut Window, cx: &mut GpuiContext<Self>) -> impl IntoElement {
        let events = cx
            .global::<Store>()
            .events()
            .iter()
            .rev()
            .cloned()
            .collect::<Vec<_>>();
        let execute = self.execute.clone();
        let empty = events.is_empty();
        let resolve = |key: &str| {
            self.locale
                .resolve(&lapis_localization::MessageId::new(key))
        };
        let empty_label = resolve("notification.empty");
        let unread = resolve("notification.unread");
        let mark_read = resolve("notification.mark-read");
        let mark_unread = resolve("notification.mark-unread");
        let delete = resolve("notification.delete");
        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_2()
            .when(empty, |view| view.child(empty_label))
            .children(events.into_iter().map(|event| {
                let id = event.id;
                let time = local_time(event.occurred_at);
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .border_1()
                    .border_color(crate::theme::colors().border_default)
                    .child(format!(
                        "{} · {}{}",
                        time,
                        event.severity.label(&self.locale),
                        if event.read {
                            String::new()
                        } else {
                            format!(" · {unread}")
                        }
                    ))
                    .child(event.title)
                    .child(event.message)
                    .children(
                        event
                            .actions
                            .into_iter()
                            .enumerate()
                            .map(|(index, action)| {
                                let execute = execute.clone();
                                div()
                                    .id(gpui::SharedString::from(format!(
                                        "notification-action-{id}-{index}"
                                    )))
                                    .cursor_pointer()
                                    .child(action.label)
                                    .on_click(move |_, window, cx| {
                                        execute(action.command.clone(), window, cx)
                                    })
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                div()
                                    .id(("notification-read", id))
                                    .cursor_pointer()
                                    .child(if event.read {
                                        mark_unread.clone()
                                    } else {
                                        mark_read.clone()
                                    })
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.global_mut::<Store>().set_read(id, !event.read);
                                    })),
                            )
                            .child(
                                div()
                                    .id(("notification-delete", id))
                                    .cursor_pointer()
                                    .child(delete.clone())
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.global_mut::<Store>().delete(id);
                                    })),
                            ),
                    )
            }))
    }
}

fn local_time(time: SystemTime) -> String {
    let local: chrono::DateTime<chrono::Local> = time.into();
    local.format("%H:%M").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_events_have_distinct_ids_and_can_be_read_and_deleted() {
        let mut store = Store::default();
        let event = Event {
            id: 0,
            kind: "host.connection_lost".into(),
            severity: Severity::Error,
            title: "接続失敗".into(),
            message: "接続できません".into(),
            context: Context::default(),
            actions: vec![],
            occurred_at: SystemTime::now(),
            read: false,
        };
        let ids: Vec<_> = (0..5).map(|_| store.publish(event.clone())).collect();
        assert_eq!(ids, [1, 2, 3, 4, 5]);
        assert_eq!(store.events().len(), 5);
        store.set_read(ids[0], true);
        assert!(store.events()[0].read);
        store.delete(ids[0]);
        assert_eq!(store.events().len(), 4);
    }

    #[test]
    fn publish_limits_actions_to_two() {
        let mut store = Store::default();
        let event = Event {
            id: 0,
            kind: "agent.input_required".into(),
            severity: Severity::Info,
            title: "入力待ち".into(),
            message: "確認が必要です".into(),
            context: Context::default(),
            actions: (0..3)
                .map(|index| Action {
                    label: format!("action-{index}"),
                    command: CommandId::new(format!("command-{index}")),
                })
                .collect(),
            occurred_at: SystemTime::now(),
            read: false,
        };

        let id = store.publish(event);
        let stored = store.events().iter().find(|event| event.id == id).unwrap();
        assert_eq!(stored.actions.len(), MAX_ACTIONS);
        assert_eq!(
            stored
                .actions
                .iter()
                .map(|action| action.command.as_str())
                .collect::<Vec<_>>(),
            ["command-0", "command-1"]
        );
    }

    #[test]
    fn default_policy_uses_kind_not_severity() {
        assert!(popup_by_default(
            "host.connection_launcher_failed",
            Severity::Info
        ));
        assert!(!popup_by_default("agent.task_completed", Severity::Info));
        assert!(!popup_by_default("unknown", Severity::Warning));
        assert!(popup_by_default("unknown", Severity::Error));
    }

    struct Empty;
    impl Render for Empty {
        fn render(&mut self, _: &mut Window, _: &mut GpuiContext<Self>) -> impl IntoElement {
            div()
        }
    }

    #[gpui::test]
    fn history_is_shared_and_popup_targets_only_the_active_window(cx: &mut gpui::TestAppContext) {
        let first = cx.add_window(|_, _| Empty);
        let second = cx.add_window(|_, _| Empty);
        let first_id = first
            .update(cx, |_, window, _| {
                window.activate_window();
                window.window_handle().window_id()
            })
            .unwrap();
        let second_id = second
            .update(cx, |_, window, _| window.window_handle().window_id())
            .unwrap();
        cx.update(|cx| {
            for _ in 0..5 {
                publish_error(
                    cx,
                    "host.connection_launcher_failed",
                    "失敗",
                    "接続できません".into(),
                );
            }
            let store = cx.global::<Store>();
            assert_eq!(store.events().len(), 5);
            assert_eq!(
                store
                    .popups_for(first_id)
                    .iter()
                    .map(|event| event.id)
                    .collect::<Vec<_>>(),
                [5, 4, 3]
            );
            assert!(store.popups_for(second_id).is_empty());
        });
        first
            .update(cx, |_, _, cx| {
                assert_eq!(cx.global::<Store>().events().len(), 5)
            })
            .unwrap();
        second
            .update(cx, |_, _, cx| {
                assert_eq!(cx.global::<Store>().events().len(), 5);
                let id = cx.global::<Store>().events()[4].id;
                cx.global_mut::<Store>().dismiss_popup(id);
                assert_eq!(cx.global::<Store>().events().len(), 5);
                assert_eq!(
                    cx.global::<Store>()
                        .popups_for(first_id)
                        .iter()
                        .map(|event| event.id)
                        .collect::<Vec<_>>(),
                    [4, 3, 2]
                );
            })
            .unwrap();
    }

    #[test]
    fn notification_labels_exist_in_both_locales_and_time_is_local() {
        let mut locale = crate::localization::Localizer::bundled();
        for locale_id in ["en-US", "ja-JP"] {
            assert!(locale.set_active(&lapis_localization::LocaleId::new(locale_id)));
            for key in [
                "notification.empty",
                "notification.unread",
                "notification.mark-read",
                "notification.mark-unread",
                "notification.delete",
                "notification.close",
                "notification.severity.info",
                "notification.severity.warning",
                "notification.severity.error",
                "notification.error.window",
                "notification.error.host-launch",
                "notification.error.persistence",
            ] {
                assert_ne!(
                    locale.resolve(&lapis_localization::MessageId::new(key)),
                    key
                );
            }
            let empty = locale.resolve(&lapis_localization::MessageId::new("notification.empty"));
            assert_eq!(
                empty,
                if locale_id == "ja-JP" {
                    "通知はありません"
                } else {
                    "No notifications"
                }
            );
        }
        let now = SystemTime::now();
        let expected = chrono::DateTime::<chrono::Local>::from(now)
            .format("%H:%M")
            .to_string();
        assert_eq!(local_time(now), expected);
        assert!(!local_time(now).contains("UTC"));
    }
}
