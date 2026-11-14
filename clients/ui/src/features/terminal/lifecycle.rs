//! Terminal のタブ個体を所有する機能側の実装。
//!
//! Shell はターミナル固有の分岐を持たない。`UiContribution::references_external_resource`
//! を宣言した View は、この [`ViewLifecycle`] をレジストリへ登録して
//! 「個体の生成・Session の開始と終了・表示状態の反映」を自分で行う。
//! タブの追加や Resource の差し替えは [`ViewRequest`] として Shell へ返す。

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use extension_ui_contract::{ViewId, ViewInstanceId};
use gpui::{AnyView, App, Subscription};

use super::{TerminalFeature, TerminalFeatureEvent, TerminalProfileCatalog};
use crate::extension_ui::{ViewLifecycle, ViewRequest};
use crate::features::id;

/// ターミナルタブを作るときのコンテキスト。Task 切替で更新する。
#[derive(Clone, Debug)]
pub(crate) struct TerminalContext {
    pub(crate) root: Option<std::path::PathBuf>,
    pub(crate) conversation: editor_core::ConversationId,
}

/// ターミナルタブの実体を作るための情報。
///
/// タブごとに独立した Session を持つが、作業ディレクトリと Task は
/// ウィンドウで共有する。個体の生成と `set_context` が同じ値を参照するため、
/// Task 切替後に開いたタブも現在のコンテキストで起動する。
#[derive(Clone)]
pub(crate) struct TerminalTabSource {
    pub(crate) backend: std::sync::Arc<dyn terminal::TerminalBackend>,
    pub(crate) context: Rc<RefCell<TerminalContext>>,
    /// Host から取得した profile 一覧を設定の供給元へ公開するため。
    pub(crate) settings: Rc<RefCell<TerminalProfileCatalog>>,
}

/// Terminal Feature がタブ個体と Session を所有する実装。
pub(crate) struct TerminalLifecycle {
    source: TerminalTabSource,
    /// この機能が作ったタブ実体。個体 ID ごとに1つだけ持つ。
    instances: BTreeMap<ViewInstanceId, gpui::Entity<TerminalFeature>>,
    subscriptions: BTreeMap<ViewInstanceId, Subscription>,
    /// 個体のイベントから受け取った Shell への要求。
    requests: Rc<RefCell<Vec<ViewRequest>>>,
    /// 要求が溜まったことを Shell へ知らせる入口。`start` で受け取る。
    notify: Option<Rc<dyn Fn(&mut App) -> bool>>,
    /// 変更通知を購読済みか。backend は receiver を1度しか返さない。
    changes_started: bool,
}

impl TerminalLifecycle {
    fn new(source: TerminalTabSource) -> Self {
        Self {
            source,
            instances: BTreeMap::new(),
            subscriptions: BTreeMap::new(),
            requests: Rc::new(RefCell::new(Vec::new())),
            notify: None,
            changes_started: false,
        }
    }

    /// この機能が今持っている全タブ実体。永続化と代表個体の取得に使う。
    pub(crate) fn entities(&self) -> Vec<gpui::Entity<TerminalFeature>> {
        self.instances.values().cloned().collect()
    }

    /// 共有コンテキストを差し替える。以降に開くタブにも同じ値を使う。
    pub(crate) fn set_context(&self, context: TerminalContext) {
        *self.source.context.borrow_mut() = context;
    }

    /// タブ実体のイベントを購読する。生成直後に1回だけ呼ぶ。
    fn subscribe(
        &mut self,
        instance: &ViewInstanceId,
        entity: &gpui::Entity<TerminalFeature>,
        cx: &mut App,
    ) {
        if self.subscriptions.contains_key(instance) {
            return;
        }
        let subscribed = instance.clone();
        let requests = self.requests.clone();
        let notify = self.notify.clone();
        let view = ViewId::new(id::VIEW_TERMINAL);
        let subscription = cx.subscribe(entity, move |_, event: &TerminalFeatureEvent, cx| {
            let mut requests = requests.borrow_mut();
            match event {
                TerminalFeatureEvent::Started(resource) => {
                    requests.push(ViewRequest::BindResource {
                        instance: subscribed.clone(),
                        resource: Some(resource.as_str().to_owned()),
                    });
                }
                TerminalFeatureEvent::NewTabRequested => {
                    requests.push(ViewRequest::OpenTab(view.clone()));
                }
            }
            drop(requests);
            // 要求を Shell が受け取れるよう、その場で再同期を促す。
            if let Some(notify) = &notify {
                notify(cx);
            }
        });
        self.subscriptions.insert(instance.clone(), subscription);
    }

    /// backend の変更通知を表示中の全タブへ配る。
    ///
    /// `changes()` は1度しか receiver を返さないため、ここで1本に集約する。
    fn start_changes(&mut self, notify: Rc<dyn Fn(&mut App) -> bool>, cx: &mut App) {
        if self.changes_started {
            return;
        }
        let Some(mut changes) = self.source.backend.changes() else {
            return;
        };
        self.changes_started = true;
        cx.spawn(async move |cx| {
            use futures_util::StreamExt;
            while changes.next().await.is_some() {
                let alive = cx.update(|cx| notify(cx));
                #[cfg(not(target_family = "wasm"))]
                if alive.is_err() {
                    break;
                }
                let _ = alive;
            }
        })
        .detach();
    }
}

impl ViewLifecycle for TerminalLifecycle {
    fn create(&mut self, instance: &ViewInstanceId, cx: &mut App) -> Option<AnyView> {
        use gpui::AppContext;
        let context = self.source.context.borrow().clone();
        let settings = self.source.settings.clone();
        let backend = self.source.backend.clone();
        let feature = cx.new(|cx| {
            TerminalFeature::new(
                desktop_client::TerminalSession::new(backend),
                context.root.clone(),
                context.conversation.clone(),
                settings,
                cx,
            )
        });
        self.subscribe(instance, &feature, cx);
        self.instances.insert(instance.clone(), feature.clone());
        Some(feature.into())
    }

    fn start(&mut self, notify: Rc<dyn Fn(&mut App) -> bool>, cx: &mut App) {
        self.notify = Some(notify.clone());
        self.start_changes(notify, cx);
    }

    fn changed(&mut self, cx: &mut App) {
        for entity in self.instances.values() {
            entity.update(cx, |tab, cx| tab.note_change(cx));
        }
    }

    fn activate(
        &mut self,
        instance: &ViewInstanceId,
        resource: Option<&str>,
        visible: bool,
        cx: &mut App,
    ) {
        let Some(view) = self.instances.get(instance).cloned() else {
            return;
        };
        if let Some(resource) = resource {
            let resource = terminal::TerminalId::new(resource);
            view.update(cx, |tab, cx| tab.show_resource(resource, cx));
        }
        view.update(cx, |tab, cx| tab.sync_tab_presence(visible, cx));
    }

    fn release(&mut self, instance: &ViewInstanceId) {
        // タブが Layout から消えただけ。Session は終了せず、配線だけを外す。
        self.subscriptions.remove(instance);
        self.instances.remove(instance);
    }

    fn deactivate(&mut self, instance: &ViewInstanceId, cx: &mut App) {
        // 終了はこのタブの実体だけ。他タブの PTY は残す。
        if let Some(view) = self.instances.get(instance).cloned() {
            view.update(cx, |tab, cx| tab.close_session(cx));
        }
        self.release(instance);
    }

    fn take_requests(&mut self) -> Vec<ViewRequest> {
        std::mem::take(&mut *self.requests.borrow_mut())
    }

    fn persisted_resources(&self, cx: &gpui::App) -> Vec<desktop_client::RestoredTerminal> {
        self.instances
            .values()
            .flat_map(|tab| tab.read(cx).persistence_summaries())
            .collect()
    }

    fn resource_owner(&self, cx: &gpui::App) -> Option<editor_core::ConversationId> {
        self.instances
            .values()
            .find_map(|tab| tab.read(cx).persistence_owner())
    }
}

/// Terminal のタブ個体を所有する実装を作る。
pub(crate) fn terminal_lifecycle(source: TerminalTabSource) -> Rc<RefCell<TerminalLifecycle>> {
    Rc::new(RefCell::new(TerminalLifecycle::new(source)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use extension_ui_contract::ViewInstanceId;
    use std::sync::Arc;

    #[derive(Default)]
    struct Backend {
        next: std::sync::atomic::AtomicUsize,
        terminated: std::sync::Mutex<Vec<String>>,
    }

    impl terminal::TerminalBackend for Backend {
        fn start(
            &self,
            cwd: &std::path::Path,
            columns: u16,
            rows: u16,
        ) -> Result<terminal::TerminalId, terminal::TerminalError> {
            self.start_profile(cwd, columns, rows, None)
        }
        fn start_profile(
            &self,
            _: &std::path::Path,
            _: u16,
            _: u16,
            _: Option<&terminal::TerminalProfileId>,
        ) -> Result<terminal::TerminalId, terminal::TerminalError> {
            Ok(terminal::TerminalId::new(format!(
                "terminal-{}",
                self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            )))
        }
        fn input(
            &self,
            _: &terminal::TerminalId,
            _: &terminal::TerminalInput,
        ) -> Result<(), terminal::TerminalError> {
            Ok(())
        }
        fn resize(
            &self,
            _: &terminal::TerminalId,
            _: u16,
            _: u16,
        ) -> Result<(), terminal::TerminalError> {
            Ok(())
        }
        fn poll(
            &self,
            _: &terminal::TerminalId,
        ) -> Result<Vec<terminal::TerminalEvent>, terminal::TerminalError> {
            Ok(Vec::new())
        }
        fn terminate(&self, id: &terminal::TerminalId) -> Result<(), terminal::TerminalError> {
            self.terminated.lock().unwrap().push(id.as_str().to_owned());
            Ok(())
        }
    }

    fn lifecycle() -> Rc<RefCell<TerminalLifecycle>> {
        terminal_lifecycle(TerminalTabSource {
            backend: Arc::new(Backend::default()),
            context: Rc::new(RefCell::new(TerminalContext {
                root: Some("workspace".into()),
                conversation: editor_core::ConversationId::new("task-a"),
            })),
            settings: Rc::new(RefCell::new(TerminalProfileCatalog::default())),
        })
    }

    /// 2つ目のタブは、1つ目とは別の個体・別のセッションになる。
    #[gpui::test]
    fn each_tab_owns_an_independent_instance(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let lifecycle = lifecycle();
            let first = ViewInstanceId::new("terminal:one");
            let second = ViewInstanceId::new("terminal:two");
            let first_view = lifecycle
                .borrow_mut()
                .create(&first, cx)
                .expect("個体を作れる");
            let second_view = lifecycle
                .borrow_mut()
                .create(&second, cx)
                .expect("個体を作れる");
            assert_ne!(first_view.entity_id(), second_view.entity_id());
            assert_eq!(lifecycle.borrow().instances.len(), 2);
        });
    }

    /// タブを閉じたら、そのタブの Session だけを終了する。
    #[gpui::test]
    fn closing_one_tab_keeps_other_tabs_running(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let lifecycle = lifecycle();
            let first = ViewInstanceId::new("terminal:one");
            let second = ViewInstanceId::new("terminal:two");
            lifecycle.borrow_mut().create(&first, cx).unwrap();
            lifecycle.borrow_mut().create(&second, cx).unwrap();
            lifecycle.borrow_mut().deactivate(&first, cx);
            assert!(lifecycle.borrow().instances.contains_key(&second));
            assert!(!lifecycle.borrow().instances.contains_key(&first));
        });
    }

    /// 閉じた直後に同じ個体を作り直せる。配線は個体ごとに独立する。
    #[gpui::test]
    fn reopening_after_close_recreates_the_instance(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let lifecycle = lifecycle();
            let instance = ViewInstanceId::new("terminal:one");
            lifecycle.borrow_mut().create(&instance, cx).unwrap();
            lifecycle.borrow_mut().deactivate(&instance, cx);
            assert!(lifecycle.borrow_mut().create(&instance, cx).is_some());
            assert!(lifecycle.borrow().instances.contains_key(&instance));
        });
    }

    /// 新しいタブの要求は Layout ではなく Shell が適用する。
    #[gpui::test]
    fn new_tab_event_becomes_a_request(cx: &mut gpui::TestAppContext) {
        let (lifecycle, instance) = cx.update(|cx| {
            let lifecycle = lifecycle();
            let instance = ViewInstanceId::new("terminal:one");
            let view = lifecycle.borrow_mut().create(&instance, cx).unwrap();
            let entity = view.downcast::<TerminalFeature>().expect("TerminalFeature");
            entity.update(cx, |feature, cx| feature.request_new_tab(cx));
            (lifecycle, instance)
        });
        // イベントの配送は effect として遅延するため、反映後に確認する。
        cx.run_until_parked();
        let requests = lifecycle.borrow_mut().take_requests();
        assert_eq!(instance, ViewInstanceId::new("terminal:one"));
        assert!(matches!(
            requests.as_slice(),
            [ViewRequest::OpenTab(view)] if view.as_str() == id::VIEW_TERMINAL
        ));
    }
}
