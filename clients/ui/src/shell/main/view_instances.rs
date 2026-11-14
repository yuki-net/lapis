//! 宣言された View の個体を Layout と一致させる Shell 側の処理。
//!
//! View の種類ごとの分岐はここに置かない。個体を所有する機能は
//! [`crate::extension_ui::ViewLifecycle`] を実装してレジストリへ登録し、
//! Shell は「Layout 上にタブがある／ない」と「表示中か」だけを見て、
//! 生成・表示の反映・終了を機能へ委ねる。機能からの要求（タブ追加・
//! Resource 差し替え）は要求として受け取り、Layout への適用だけを行う。

use std::cell::RefCell;
use std::rc::Rc;

use super::*;
use crate::extension_ui::{ViewLifecycle, ViewRequest};

/// タブの表示状態を1つの View について解決した結果。
struct TabState {
    view: ViewId,
    instance: ViewInstanceId,
    resource: Option<String>,
    visible: bool,
}

impl Shell {
    /// 個体を所有する機能をレジストリへ登録し、タブとの配線を始める。
    ///
    /// Window ごとに1回だけ呼ぶ。以降のタブの生成・表示・終了は、登録した
    /// 機能が自分の個体に対して行う。機能固有の操作（コンテキスト切替・
    /// 永続化）は呼び出し側が受け取ったハンドルから行う。
    pub(crate) fn configure_view_lifecycles(
        &mut self,
        lifecycle: Rc<RefCell<dyn ViewLifecycle>>,
        view: ViewId,
        cx: &mut Context<Self>,
    ) {
        self.feature_registry
            .register_view_lifecycle(view.clone(), lifecycle);
        // 機能の外部変更通知を、Shell の再同期へ繋ぐ。変更は画面全体ではなく
        // 該当 View の個体だけに配る。
        let shell = cx.entity().downgrade();
        let notify: Rc<dyn Fn(&mut App) -> bool> = Rc::new(move |cx: &mut App| {
            shell
                .update(cx, |shell, cx| {
                    shell.feature_registry.changed(cx);
                    shell.synchronize_view_instances(cx);
                    cx.notify();
                })
                .is_ok()
        });
        self.feature_registry.ensure_started(&view, notify, cx);
        self.synchronize_view_instances(cx);
    }

    /// タブの表示状態を各機能へ伝え、機能からの要求を Layout に適用する。
    ///
    /// 機能の登録は View の種類に依存しない。以後に個体所有者を増やす場合も
    /// 同じ経路を使う。
    pub(crate) fn synchronize_owned_instances(&mut self, cx: &mut Context<Self>) {
        let states = self.tab_states();
        let views = self
            .feature_registry
            .lifecycle_views()
            .into_iter()
            .collect::<Vec<_>>();
        for view in views {
            let owned = states.iter().filter(|state| state.view == view);
            let wanted = owned
                .clone()
                .map(|state| state.instance.clone())
                .collect::<Vec<_>>();
            // タブがあるのに実体が無ければ、個体所有者に作らせる。
            for state in owned.clone() {
                if !self
                    .feature_registry
                    .ensure_view_instance(&state.view, &state.instance, cx)
                {
                    continue;
                }
                self.feature_registry.activate_instance(
                    &state.view,
                    &state.instance,
                    state.resource.as_deref(),
                    state.visible,
                    cx,
                );
            }
            // タブが消えた個体は配線だけを外す。Session は終了しない（close 経路だけが終了する）。
            let stale = self
                .feature_registry
                .view_instances(&view)
                .into_iter()
                .filter(|instance| !wanted.contains(instance))
                .collect::<Vec<_>>();
            for instance in stale {
                self.feature_registry.release_instance(&view, &instance, cx);
            }
        }
        self.apply_view_requests(cx);
    }

    /// 機能からの要求を Layout に適用する。
    fn apply_view_requests(&mut self, cx: &mut Context<Self>) {
        for (_, request) in self.feature_registry.take_view_requests() {
            match request {
                ViewRequest::BindResource { instance, resource } => {
                    self.rebind_tab_resource(&instance, resource);
                }
                ViewRequest::OpenTab(view) => {
                    self.open_new_tab_for_view(view, cx);
                }
            }
        }
    }

    /// タブの表示条件を、Layout の現在の配置から解決する。
    fn tab_states(&self) -> Vec<TabState> {
        self.shell
            .layout
            .placements()
            .into_iter()
            .filter_map(|(tab, _)| {
                let view = tab.view_id()?.clone();
                Some(TabState {
                    resource: tab.resource().map(ToOwned::to_owned),
                    visible: self.tab_visible(&tab),
                    instance: tab.instance_id(),
                    view,
                })
            })
            .collect()
    }

    /// タブが画面に出ているか。表示中パネルで選択されているタブだけを見る。
    fn tab_visible(&self, tab: &PanelTab) -> bool {
        self.shell.layout.visible_panels().any(|panel| {
            self.shell
                .visible_active(panel)
                .is_some_and(|active| active == tab)
        })
    }

    /// 外部 Resource を参照するタブの Resource を差し替える。個体 ID は変えない。
    fn rebind_tab_resource(&mut self, instance: &ViewInstanceId, resource: Option<String>) {
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
        let next = current.with_resource(resource);
        if next != current {
            self.shell.layout.rebind_tab(&current, next);
        }
    }

    /// 同じ View のタブを、現在のパネルと同じ位置に1つ増やす。
    ///
    /// 個体化する View はタブごとに独立した実体を持つため、2つ目は新しい個体になる。
    fn open_new_tab_for_view(&mut self, view: ViewId, cx: &mut Context<Self>) {
        let position = self
            .shell
            .focused_panel
            .as_ref()
            .and_then(|panel| self.shell.layout.zone_for_panel(panel))
            .filter(|position| *position != ZonePosition::Main)
            .unwrap_or(ZonePosition::Bottom);
        let Ok(instance) = self.shell.next_tool_instance(&view) else {
            return;
        };
        let tab = self.new_tab_for(&view, instance, None);
        if let Ok(panel) = self.shell.layout.open_tab(position, tab) {
            self.shell.layout.set_panel_open(&panel, true);
            self.shell.layout.set_zone_open(position, true);
            self.shell.focused_panel = Some(panel);
        }
        self.refresh_feature_activation(cx);
        cx.notify();
    }

    /// 宣言に従ってタブを作る。外部 Resource を参照する宣言なら Resource を渡す。
    pub(super) fn new_tab_for(
        &self,
        view: &ViewId,
        instance: ViewInstanceId,
        resource: Option<String>,
    ) -> PanelTab {
        if self.tab_references_resource(view) {
            PanelTab::external(view.clone(), instance, resource)
        } else {
            PanelTab::tool_instance(view.clone(), instance)
        }
    }

    /// 宣言が外部 Resource の参照を求める View か。
    pub(super) fn tab_references_resource(&self, view: &ViewId) -> bool {
        self.feature_registry
            .declaration(view)
            .is_some_and(|declaration| {
                declaration.tab_reference == extension_ui_contract::TabReference::ExternalResource
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Empty;
    impl Render for Empty {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    /// 個体を作る機能の最小実装。Resource の反映と終了を記録する。
    #[derive(Default)]
    struct RecordingLifecycle {
        created: Vec<ViewInstanceId>,
        bound: Vec<(ViewInstanceId, Option<String>, bool)>,
        released: Vec<ViewInstanceId>,
        closed: Vec<ViewInstanceId>,
    }

    impl ViewLifecycle for RecordingLifecycle {
        fn create(&mut self, instance: &ViewInstanceId, cx: &mut App) -> Option<AnyView> {
            use gpui::AppContext;
            self.created.push(instance.clone());
            Some(cx.new(|_| Empty).into())
        }

        fn activate(
            &mut self,
            instance: &ViewInstanceId,
            resource: Option<&str>,
            visible: bool,
            _: &mut App,
        ) {
            self.bound
                .push((instance.clone(), resource.map(ToOwned::to_owned), visible));
        }

        fn deactivate(&mut self, instance: &ViewInstanceId, _: &mut App) {
            self.closed.push(instance.clone());
        }

        fn release(&mut self, instance: &ViewInstanceId) {
            self.released.push(instance.clone());
        }
    }

    #[gpui::test]
    fn a_declared_tab_gets_its_instance_from_the_owning_feature(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            crate::theme::init(&extension_ui_contract::ThemeId::new("lapis.dark"), None, cx)
        });
        let lifecycle = Rc::new(RefCell::new(RecordingLifecycle::default()));
        let shell = cx.new(|cx| {
            let document = cx.new(|_| Empty);
            let mut shell = Shell::new(document.into(), HashMap::new(), cx);
            shell.configure_view_lifecycles(lifecycle.clone(), ViewId::new(id::VIEW_TERMINAL), cx);
            shell
        });
        shell.update(cx, |shell, cx| {
            shell.open_view_in_zone(ZonePosition::Bottom, ViewId::new(id::VIEW_TERMINAL), cx);
            let tab = shell
                .shell
                .layout
                .placements()
                .into_iter()
                .find(|(tab, _)| {
                    tab.view_id()
                        .is_some_and(|view| view.as_str() == id::VIEW_TERMINAL)
                })
                .map(|(tab, _)| tab)
                .expect("タブが増える");
            assert!(
                tab.resource().is_none(),
                "未起動のタブは Resource を持たない"
            );
            assert_eq!(lifecycle.borrow().created, vec![tab.instance_id()]);
            assert!(
                shell
                    .feature_registry
                    .instance_view(&tab.instance_id())
                    .is_some(),
                "個体所有者が作った実体を Shell が表示する"
            );
        });
    }

    /// 2つ目のタブは独立した個体になり、1つ目と同じセッションに畳まれない。
    #[gpui::test]
    fn opening_the_same_declared_view_twice_creates_two_instances(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            crate::theme::init(&extension_ui_contract::ThemeId::new("lapis.dark"), None, cx)
        });
        let lifecycle = Rc::new(RefCell::new(RecordingLifecycle::default()));
        let shell = cx.new(|cx| {
            let document = cx.new(|_| Empty);
            let mut shell = Shell::new(document.into(), HashMap::new(), cx);
            shell.configure_view_lifecycles(lifecycle.clone(), ViewId::new(id::VIEW_TERMINAL), cx);
            shell
        });
        shell.update(cx, |shell, cx| {
            let view = ViewId::new(id::VIEW_TERMINAL);
            shell.open_view_in_zone(ZonePosition::Bottom, view.clone(), cx);
            shell.open_view_in_zone(ZonePosition::Bottom, view, cx);
            let instances = shell
                .shell
                .layout
                .placements()
                .into_iter()
                .filter(|(tab, _)| {
                    tab.view_id()
                        .is_some_and(|view| view.as_str() == id::VIEW_TERMINAL)
                })
                .map(|(tab, _)| tab.instance_id())
                .collect::<Vec<_>>();
            assert_eq!(instances.len(), 2, "開くたびにタブが増える");
            assert_ne!(instances[0], instances[1], "タブは別の個体");
            assert_eq!(lifecycle.borrow().created.len(), 2);
        });
    }

    /// タブを閉じると、その個体だけを所有者へ終了として伝える。
    #[gpui::test]
    fn closing_one_tab_only_deactivates_that_instance(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            crate::theme::init(&extension_ui_contract::ThemeId::new("lapis.dark"), None, cx)
        });
        let lifecycle = Rc::new(RefCell::new(RecordingLifecycle::default()));
        let shell = cx.new(|cx| {
            let document = cx.new(|_| Empty);
            let mut shell = Shell::new(document.into(), HashMap::new(), cx);
            shell.configure_view_lifecycles(lifecycle.clone(), ViewId::new(id::VIEW_TERMINAL), cx);
            shell
        });
        shell.update(cx, |shell, cx| {
            let view = ViewId::new(id::VIEW_TERMINAL);
            shell.open_view_in_zone(ZonePosition::Bottom, view.clone(), cx);
            shell.open_view_in_zone(ZonePosition::Bottom, view, cx);
            let tabs = shell
                .shell
                .layout
                .placements()
                .into_iter()
                .filter(|(tab, _)| {
                    tab.view_id()
                        .is_some_and(|view| view.as_str() == id::VIEW_TERMINAL)
                })
                .map(|(tab, _)| tab)
                .collect::<Vec<_>>();
            let (first, second) = (tabs[0].instance_id(), tabs[1].instance_id());
            let panel = shell.shell.layout.panel_for_tab(&tabs[0]).unwrap();
            shell.close_tool_tab(&panel, &tabs[0], cx);
            assert_eq!(lifecycle.borrow().closed, vec![first.clone()]);
            assert!(
                shell.feature_registry.instance_view(&second).is_some(),
                "閉じていないタブの個体は残る"
            );
        });
    }

    /// 機能からの Resource 差し替え要求は、個体IDを変えずにタブへ反映する。
    #[gpui::test]
    fn a_resource_request_rebinds_the_tab_without_changing_the_instance(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            crate::theme::init(&extension_ui_contract::ThemeId::new("lapis.dark"), None, cx)
        });
        let shell = cx.new(|cx| {
            let document = cx.new(|_| Empty);
            Shell::new(document.into(), HashMap::new(), cx)
        });
        shell.update(cx, |shell, _| {
            let view = ViewId::new(id::VIEW_TERMINAL);
            let instance = ViewInstanceId::new("terminal:one");
            let tab = shell.new_tab_for(&view, instance.clone(), None);
            shell
                .shell
                .layout
                .open_tab(ZonePosition::Bottom, tab.clone())
                .unwrap();
            let before = tab.instance_id();
            shell.rebind_tab_resource(&instance, Some("terminal-a".to_owned()));
            let bound = shell
                .shell
                .layout
                .placements()
                .into_iter()
                .find(|(tab, _)| tab.instance_id() == instance)
                .map(|(tab, _)| tab)
                .unwrap();
            assert_eq!(bound.instance_id(), before);
            assert_eq!(bound.resource(), Some("terminal-a"));
        });
    }

    /// タブが Layout から消えただけでは Session を終了しない。
    ///
    /// Task 切替や Layout の置き換えは detach であり、終了は明示的な close だけが行う。
    #[gpui::test]
    fn a_disappearing_tab_is_released_without_terminating_the_session(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            crate::theme::init(&extension_ui_contract::ThemeId::new("lapis.dark"), None, cx)
        });
        let lifecycle = Rc::new(RefCell::new(RecordingLifecycle::default()));
        let shell = cx.new(|cx| {
            let document = cx.new(|_| Empty);
            let mut shell = Shell::new(document.into(), HashMap::new(), cx);
            shell.configure_view_lifecycles(lifecycle.clone(), ViewId::new(id::VIEW_TERMINAL), cx);
            shell
        });
        shell.update(cx, |shell, cx| {
            let view = ViewId::new(id::VIEW_TERMINAL);
            shell.open_view_in_zone(ZonePosition::Bottom, view, cx);
            let tab = shell
                .shell
                .layout
                .placements()
                .into_iter()
                .find(|(tab, _)| {
                    tab.view_id()
                        .is_some_and(|view| view.as_str() == id::VIEW_TERMINAL)
                })
                .map(|(tab, _)| tab)
                .unwrap();
            let instance = tab.instance_id();
            let panel = shell.shell.layout.panel_for_tab(&tab).unwrap();
            shell
                .shell
                .layout
                .remove_tab(&panel, &tab, crate::time::Instant::now());
            shell.refresh_feature_activation(cx);
            assert_eq!(lifecycle.borrow().closed, Vec::<ViewInstanceId>::new());
            assert_eq!(lifecycle.borrow().released, vec![instance]);
        });
    }
}
