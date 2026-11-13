use super::*;

/// ターミナルタブを作るときのコンテキスト。Task 切替で更新する。
#[derive(Clone, Debug)]
pub(crate) struct TerminalContext {
    pub(crate) root: Option<std::path::PathBuf>,
    pub(crate) conversation: editor_core::ConversationId,
}

/// ターミナルタブの実体を作るための情報。
///
/// タブごとに独立した Session を持つが、作業ディレクトリと Task は
/// ウィンドウで共有する。factory と `set_terminal_context` が同じ値を
/// 参照するため、Task 切替後に開いたタブも現在のコンテキストで起動する。
#[derive(Clone)]
pub(crate) struct TerminalTabSource {
    pub(crate) backend: std::sync::Arc<dyn terminal::TerminalBackend>,
    pub(crate) context: std::rc::Rc<std::cell::RefCell<TerminalContext>>,
}

impl Shell {
    /// ターミナルタブの生成元を設定し、個体ごとの実体を作る factory を登録する。
    ///
    /// Window ごとに1回だけ呼ぶ。factory は個体IDだけを受け取り、タブごとに
    /// 独立した `TerminalFeature` を作る。ライフサイクルは Layout が決める。
    pub(crate) fn configure_terminal_tabs(
        &mut self,
        source: TerminalTabSource,
        cx: &mut Context<Self>,
    ) {
        self.terminal_source = Some(source.clone());
        self.feature_registry.register_view_factory(
            ViewId::new(id::VIEW_TERMINAL),
            std::rc::Rc::new(move |_instance, cx| {
                let context = source.context.borrow().clone();
                let feature = cx.new(|cx| {
                    crate::features::terminal::TerminalFeature::new(
                        desktop_client::TerminalSession::new(source.backend.clone()),
                        context.root.clone(),
                        context.conversation.clone(),
                        cx,
                    )
                });
                feature.into()
            }),
        );
        // 既に復元済みのタブがあれば、この場で実体を作る。
        self.synchronize_terminal_tabs(cx);
    }

    /// 変更通知を1本だけ購読し、全てのターミナルタブへ配る。
    ///
    /// backend の `changes()` は1度しか receiver を返さないため、タブごとに
    /// 購読すると2つ目以降が更新されない。ここで集約する。
    pub(crate) fn start_terminal_changes(&mut self, cx: &mut Context<Self>) {
        if self.terminal_changes_started {
            return;
        }
        let Some(source) = self.terminal_source.clone() else {
            return;
        };
        let Some(mut changes) = source.backend.changes() else {
            return;
        };
        self.terminal_changes_started = true;
        cx.spawn(async move |shell, cx| {
            use futures_util::StreamExt;
            while changes.next().await.is_some() {
                if shell
                    .update(cx, |shell, cx| shell.deliver_terminal_changes(cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn deliver_terminal_changes(&mut self, cx: &mut Context<Self>) {
        for tab in self.terminal_entities() {
            tab.update(cx, |tab, cx| tab.note_change(cx));
        }
    }

    /// 現在 Layout にあるターミナルタブの実体。
    fn terminal_entities(&self) -> Vec<gpui::Entity<crate::features::terminal::TerminalFeature>> {
        self.shell
            .layout
            .placements()
            .into_iter()
            .filter(|(tab, _)| tab.is_terminal())
            .filter_map(|(tab, _)| {
                self.feature_registry
                    .instance_entity::<crate::features::terminal::TerminalFeature>(
                        &tab.instance_id(),
                    )
            })
            .collect()
    }

    /// Layout 上のターミナルタブと実体を一致させる。
    ///
    /// タブがあるのに実体が無ければ factory で作り、購読と表示状態を結ぶ。
    /// タブが消えた実体は購読を外してレジストリから落とす。Session の終了は
    /// タブを閉じる操作 `close_terminal_tab` だけが行う。
    pub(crate) fn synchronize_terminal_tabs(&mut self, cx: &mut Context<Self>) {
        let placements = self.shell.layout.placements();
        let wanted = placements
            .iter()
            .filter(|(tab, _)| tab.is_terminal())
            .map(|(tab, _)| tab.instance_id())
            .collect::<Vec<_>>();
        let view = ViewId::new(id::VIEW_TERMINAL);
        for instance in &wanted {
            if self
                .feature_registry
                .instance_entity::<crate::features::terminal::TerminalFeature>(instance)
                .is_none()
                && !self
                    .feature_registry
                    .ensure_view_instance(&view, instance, cx)
            {
                continue;
            }
            let Some(entity) = self
                .feature_registry
                .instance_entity::<crate::features::terminal::TerminalFeature>(instance)
            else {
                continue;
            };
            if !self.terminal_subscriptions.contains_key(instance) {
                let subscribed = instance.clone();
                let subscription = cx.subscribe(&entity, move |shell, _, event, cx| {
                    shell.terminal_tab_event(&subscribed, event, cx);
                });
                self.terminal_subscriptions
                    .insert(instance.clone(), subscription);
            }
            // タブの参照先 Resource を実体へ結ぶ。未確定なら何もしない。
            let resource = self
                .shell
                .layout
                .placements()
                .into_iter()
                .find(|(tab, _)| tab.instance_id() == *instance)
                .and_then(|(tab, _)| tab.terminal_id());
            if let Some(resource) = resource {
                entity.update(cx, |entity, cx| entity.show_resource(resource, cx));
            }
            let visible = self.terminal_tab_visible(instance);
            entity.update(cx, |entity, cx| entity.sync_tab_presence(visible, cx));
        }
        let stale = self
            .terminal_subscriptions
            .keys()
            .filter(|instance| !wanted.contains(instance))
            .cloned()
            .collect::<Vec<_>>();
        for instance in stale {
            self.terminal_subscriptions.remove(&instance);
            self.feature_registry.close_view_instance(&instance);
        }
    }

    /// タブ1つの実体を返す。タブが無ければ None。
    pub(crate) fn terminal_entity(
        &self,
        instance: &ViewInstanceId,
    ) -> Option<gpui::Entity<crate::features::terminal::TerminalFeature>> {
        self.feature_registry
            .instance_entity::<crate::features::terminal::TerminalFeature>(instance)
    }

    fn terminal_tab_visible(&self, instance: &ViewInstanceId) -> bool {
        self.shell.layout.visible_panels().any(|panel| {
            self.shell
                .visible_active(panel)
                .is_some_and(|tab| tab.instance_id() == *instance && tab.is_terminal())
        })
    }

    /// タブの表示状態を実体へ伝える。表示中だけ自動起動する。
    pub(crate) fn sync_terminal_tab_presence(&mut self, cx: &mut Context<Self>) {
        let visibility = self
            .shell
            .layout
            .placements()
            .into_iter()
            .filter(|(tab, _)| tab.is_terminal())
            .map(|(tab, _)| {
                let instance = tab.instance_id();
                let visible = self.terminal_tab_visible(&instance);
                (instance, visible)
            })
            .collect::<Vec<_>>();
        for (instance, visible) in visibility {
            if let Some(entity) = self.terminal_entity(&instance) {
                entity.update(cx, |entity, cx| entity.sync_tab_presence(visible, cx));
            }
        }
    }

    fn terminal_tab_event(
        &mut self,
        instance: &ViewInstanceId,
        event: &crate::features::terminal::TerminalFeatureEvent,
        cx: &mut Context<Self>,
    ) {
        use crate::features::terminal::TerminalFeatureEvent;
        match event {
            TerminalFeatureEvent::Started(resource) => {
                self.bind_terminal_tab_resource(instance, Some(resource.clone()));
                self.refresh_feature_activation(cx);
                cx.notify();
            }
            TerminalFeatureEvent::NewTabRequested => {
                self.open_new_terminal_tab(cx);
            }
        }
    }

    /// タブの参照先 Resource を差し替える。個体IDは変えない。
    pub(crate) fn bind_terminal_tab_resource(
        &mut self,
        instance: &ViewInstanceId,
        resource: Option<terminal::TerminalId>,
    ) {
        let Some(current) = self
            .shell
            .layout
            .placements()
            .into_iter()
            .find(|(tab, _)| tab.instance_id() == *instance)
            .map(|(tab, _)| tab)
        else {
            return;
        };
        let next = current.with_terminal_resource(resource);
        if next != current {
            self.shell.layout.rebind_tab(&current, next);
        }
    }

    /// 新しいターミナルタブを、現在のパネルと同じ位置に1つ増やす。
    /// タブは1セッションなので、2つ目は新しいタブと新しい Session になる。
    pub(crate) fn open_new_terminal_tab(&mut self, cx: &mut Context<Self>) {
        let position = self
            .shell
            .focused_panel
            .as_ref()
            .and_then(|panel| self.shell.layout.zone_for_panel(panel))
            .filter(|position| *position != ZonePosition::Main)
            .unwrap_or(ZonePosition::Bottom);
        let Ok(instance) = self
            .shell
            .next_tool_instance(&ViewId::new(id::VIEW_TERMINAL))
        else {
            return;
        };
        let tab = PanelTab::terminal_instance(instance);
        if let Ok(panel) = self.shell.layout.open_tab(position, tab) {
            self.shell.layout.set_panel_open(&panel, true);
            self.shell.layout.set_zone_open(position, true);
            self.shell.focused_panel = Some(panel);
        }
        self.refresh_feature_activation(cx);
        cx.notify();
    }

    /// タブを閉じる。そのタブの Session だけを終了する。
    pub(crate) fn close_terminal_tab(
        &mut self,
        tab: &PanelTab,
        panel: &PanelId,
        cx: &mut Context<Self>,
    ) {
        let instance = tab.instance_id();
        // 終了はこのタブの実体だけ。他タブの PTY は残す。
        if let Some(entity) = self.terminal_entity(&instance) {
            entity.update(cx, |entity, cx| entity.close_session(cx));
        }
        self.terminal_subscriptions.remove(&instance);
        self.feature_registry.close_view_instance(&instance);
        self.shell
            .layout
            .remove_tab(panel, tab, crate::time::Instant::now());
        self.schedule_empty_close(panel.clone(), cx);
        self.refresh_feature_activation(cx);
        cx.notify();
    }

    /// 各ターミナルタブの永続化用サマリー。
    pub(crate) fn terminal_summaries(&self, cx: &gpui::App) -> Vec<terminal::TerminalSnapshot> {
        self.terminal_tab_instances()
            .into_iter()
            .filter_map(|instance| self.terminal_entity(&instance))
            .flat_map(|tab| tab.read(cx).summaries())
            .collect()
    }

    /// 各ターミナルタブの永続化用メタデータ。
    pub(crate) fn terminal_persistence_summaries(
        &self,
        cx: &gpui::App,
    ) -> Vec<desktop_client::RestoredTerminal> {
        self.terminal_tab_instances()
            .into_iter()
            .filter_map(|instance| self.terminal_entity(&instance))
            .flat_map(|tab| tab.read(cx).persistence_summaries())
            .collect()
    }

    /// ターミナルタブの所有 Task。全タブが同じコンテキストを共有する。
    pub(crate) fn terminal_owner(&self, cx: &gpui::App) -> Option<editor_core::ConversationId> {
        self.terminal_tab_instances()
            .into_iter()
            .filter_map(|instance| self.terminal_entity(&instance))
            .find_map(|tab| tab.read(cx).persistence_owner())
    }

    /// Layout 上のターミナルタブの個体ID。
    fn terminal_tab_instances(&self) -> Vec<ViewInstanceId> {
        self.shell
            .layout
            .placements()
            .into_iter()
            .filter(|(tab, _)| tab.is_terminal())
            .map(|(tab, _)| tab.instance_id())
            .collect()
    }

    /// 全ターミナルタブのコンテキストを切り替える。以降に開くタブにも同じ値を使う。
    pub(crate) fn set_terminal_context(
        &mut self,
        root: Option<std::path::PathBuf>,
        conversation: editor_core::ConversationId,
        summaries: Vec<desktop_client::RestoredTerminal>,
        cx: &mut Context<Self>,
    ) {
        if let Some(source) = &self.terminal_source {
            *source.context.borrow_mut() = TerminalContext {
                root: root.clone(),
                conversation: conversation.clone(),
            };
        }
        for instance in self.terminal_tab_instances() {
            if let Some(entity) = self.terminal_entity(&instance) {
                entity.update(cx, |entity, cx| {
                    entity.set_context(root.clone(), conversation.clone(), summaries.clone(), cx)
                });
            }
        }
    }

    /// 復元したタブの参照先 Resource を実体へ結ぶ。
    pub(crate) fn bind_terminal_resources(&mut self, cx: &mut Context<Self>) {
        self.synchronize_terminal_tabs(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct DocumentFixture;
    impl Render for DocumentFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

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

    fn shell_with_terminals(cx: &mut gpui::App) -> gpui::Entity<Shell> {
        cx.new(|cx| {
            let document = cx.new(|_| DocumentFixture);
            let mut shell = Shell::new(document.into(), HashMap::new(), cx);
            shell.configure_terminal_tabs(
                TerminalTabSource {
                    backend: Arc::new(Backend::default()),
                    context: std::rc::Rc::new(std::cell::RefCell::new(TerminalContext {
                        root: Some("workspace".into()),
                        conversation: editor_core::ConversationId::new("task-a"),
                    })),
                },
                cx,
            );
            shell
        })
    }

    /// 2つ目の Terminal タブは、1つ目とは別のタブ・別のセッションになる。
    #[gpui::test]
    fn opening_a_second_terminal_tab_creates_an_independent_session(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let shell = shell_with_terminals(cx);
            shell.update(cx, |shell, cx| {
                let view = ViewId::new(id::VIEW_TERMINAL);
                shell.open_view_in_zone(ZonePosition::Bottom, view.clone(), cx);
                shell.open_view_in_zone(ZonePosition::Bottom, view, cx);
                let terminals = shell.terminal_tab_instances();
                assert_eq!(terminals.len(), 2, "開くたびにタブが増える");
                assert_ne!(terminals[0], terminals[1], "タブは別の個体");
            });
        });
    }

    /// 同じ文書を開き直しても複製せず、既存タブを選ぶ。
    #[gpui::test]
    fn opening_the_same_document_focuses_the_existing_tab(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let shell = shell_with_terminals(cx);
            shell.update(cx, |shell, _| {
                let document = PanelTab::Document(editor_core::DocumentId::new("doc-1"));
                let panel = shell
                    .shell
                    .layout
                    .open_tab(ZonePosition::Main, document.clone())
                    .unwrap();
                shell
                    .shell
                    .layout
                    .open_tab(ZonePosition::Bottom, PanelTab::tool(id::VIEW_FILES))
                    .unwrap();
                // 同じ文書を開き直すと、移動ではなく既存タブの選択になる。
                shell
                    .shell
                    .layout
                    .open_tab(ZonePosition::Main, document.clone())
                    .unwrap();
                let duplicates = shell
                    .shell
                    .layout
                    .placements()
                    .into_iter()
                    .filter(|(tab, _)| tab == &document)
                    .count();
                assert_eq!(duplicates, 1, "同じ文書のタブは複製しない");
                assert_eq!(
                    shell.shell.layout.panel(&panel).unwrap().active,
                    Some(document)
                );
            });
        });
    }

    /// Terminal タブを閉じても、他のタブのセッションは終了しない。
    #[gpui::test]
    fn closing_one_terminal_tab_keeps_other_tabs_running(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let shell = shell_with_terminals(cx);
            shell.update(cx, |shell, cx| {
                let view = ViewId::new(id::VIEW_TERMINAL);
                shell.open_view_in_zone(ZonePosition::Bottom, view.clone(), cx);
                shell.open_view_in_zone(ZonePosition::Bottom, view, cx);
                let terminals = shell.terminal_tab_instances();
                let (first, second) = (terminals[0].clone(), terminals[1].clone());
                let tab = shell
                    .shell
                    .layout
                    .placements()
                    .into_iter()
                    .find(|(tab, _)| tab.instance_id() == first)
                    .map(|(tab, _)| tab)
                    .unwrap();
                let panel = shell.shell.layout.panel_for_tab(&tab).unwrap();
                assert!(shell.terminal_entity(&second).is_some(), "close前");
                shell.close_terminal_tab(&tab, &panel, cx);
                let after = shell.terminal_tab_instances();
                assert!(shell.terminal_entity(&first).is_none());
                assert!(
                    shell.terminal_entity(&second).is_some(),
                    "after={after:?} second={second:?}"
                );
                assert_eq!(after, vec![second]);
            });
        });
    }
}
