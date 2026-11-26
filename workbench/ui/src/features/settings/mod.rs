use gpui::prelude::*;
use web::{WebOperation, WebState};

mod extensions;
mod fields;
mod navigation;
#[cfg(test)]
mod tests;
mod web;

use super::settings_coordinator::SettingsCoordinator;
use crate::client_adapter::ClientWorker;
use crate::components::ScrollState;
use crate::localization::Localizer;
use crate::theme;
use desktop_client::WebAccessControl;
use gpui::{Context, Render, Window, div, px};
use settings::{ResolvedSettings, SettingKey, SettingValue, SettingsScope};
use std::path::PathBuf;
use std::sync::Arc;

/// 左ナビ3層（分類 → 中間層 → ページ）と、定義データから組み立てる設定ページ。
pub(crate) struct SettingsFeature {
    coordinator: gpui::Entity<SettingsCoordinator>,
    _subscription: gpui::Subscription,
    _catalogs: Vec<gpui::Subscription>,
    values: ResolvedSettings,
    extension_terminal: bool,
    root: Option<PathBuf>,
    locale: Localizer,
    message: String,
    page: &'static str,
    search: gpui::Entity<crate::components::SearchInput>,
    _search_subscription: gpui::Subscription,
    query: String,
    open_choice: Option<SettingKey>,
    choice_scroll: ScrollState,
    content_scroll: ScrollState,
    nav_scroll: ScrollState,
    pending_scroll: bool,
    sync_page_on_scroll: bool,
    last_scroll_offset: gpui::Pixels,
    /// 候補を持たない値を編集中の項目。確定するまで保存しない。
    editing: Option<TextEdit>,
    /// Desktop からの Web 公開管理。リモート接続や Web 構成では `None`。
    web: Option<WebState>,
    /// ローカル Web の接続情報を都度集める供給元。無い構成では接続ページを出さない。
    web_access: Option<crate::app::WebAccessSource>,
    /// 接続中の Workspace ID。Web の初回接続で入力する値として表示する。
    workspace_id: Option<String>,
    /// 接続ページで選んでいる候補ホスト。
    selected_web_host: Option<String>,
    /// コピーやブラウザー起動の結果を一時的に示す。
    web_message: String,
    extension: Option<desktop_client::ExtensionContributionSession>,
    extension_worker: Option<ClientWorker<desktop_client::ExtensionContributionSession>>,
    extension_generation: u64,
    extension_edit: Option<ExtensionTextEdit>,
    #[cfg(not(target_family = "wasm"))]
    extension_manager: Option<ClientWorker<crate::app::ExtensionManager>>,
    extension_management_busy: bool,
    capability_info: Option<desktop_client::HostExtensionCapabilitiesResponse>,
    capability_generation: u64,
    capability_busy: bool,
    #[cfg(not(target_family = "wasm"))]
    capability_manager: Option<ClientWorker<crate::app::ExtensionCapabilityManager>>,
}

struct ExtensionTextEdit {
    key: String,
    kind: settings::ValueKind,
    input: gpui::Entity<crate::components::SearchInput>,
    draft: String,
    _subscription: gpui::Subscription,
}

type ExtensionRow = (String, String, String, settings::ExtensionSettingDefinition);

fn extension_value_text(value: &SettingValue) -> String {
    match value {
        SettingValue::Text(value) => value.clone(),
        SettingValue::Integer(value) => value.to_string(),
        SettingValue::Flag(value) => value.to_string(),
    }
}

/// 単一行の値編集。IME 対応は共通の入力部品に任せる。
struct TextEdit {
    key: SettingKey,
    input: gpui::Entity<crate::components::SearchInput>,
    /// 入力中の値。確定するまで保存しない。
    draft: String,
    _subscription: gpui::Subscription,
}

impl gpui::EventEmitter<ResolvedSettings> for SettingsFeature {}

impl SettingsFeature {
    #[cfg(test)]
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        Self::with_web_access(cx, None, None)
    }

    /// 接続情報の供給元と Web 公開管理を受け取って作る。テストは同じ入口を使う。
    pub(crate) fn with_web_access(
        cx: &mut Context<Self>,
        web_access: Option<crate::app::WebAccessSource>,
        web_control: Option<Arc<dyn WebAccessControl>>,
    ) -> Self {
        let coordinator = SettingsCoordinator::shared(cx);
        let values = coordinator.read(cx).values.clone();
        let mut locale = Localizer::bundled();
        let _ = locale.set_active(&values.locale());
        let placeholder =
            locale.resolve(&localization::MessageId::new("settings.search.placeholder"));
        let search = cx.new(|cx| crate::components::SearchInput::new(&placeholder, cx));
        let search_subscription = cx.subscribe(&search, |this, _, event, cx| {
            let crate::components::SearchInputEvent::Changed(text) = event;
            this.query = text.clone();
            this.open_choice = None;
            this.content_scroll
                .handle()
                .set_offset(gpui::point(px(0.0), px(0.0)));
            if let Some(page) = this.visible_pages().first() {
                this.page = page;
            }
            cx.notify();
        });
        let subscription = cx.observe(&coordinator, |this, coordinator, cx| {
            let state = coordinator.read(cx);
            this.values = state.values.clone();
            this.message = state.message.clone();
            let _ = this.locale.set_active(&this.values.locale());
            let placeholder = this.t("settings.search.placeholder");
            this.search
                .update(cx, |search, cx| search.set_placeholder(placeholder, cx));
            cx.emit(this.values.clone());
            cx.notify();
        });
        // 設定の供給元が更新されたら選択肢を作り直す。
        let catalogs = vec![
            cx.observe_global::<super::settings_sources::SettingsSources>(|_, cx| cx.notify()),
        ];
        let web = web_control.map(|control| WebState {
            worker: ClientWorker::new(control),
            status: None,
            devices: Vec::new(),
            address: "127.0.0.1".into(),
            busy: false,
            generation: 0,
            message: String::new(),
        });
        let content_scroll = ScrollState::new();
        Self {
            coordinator,
            _subscription: subscription,
            _catalogs: catalogs,
            values,
            extension_terminal: false,
            root: None,
            locale,
            message: String::new(),
            page: default_page(),
            search,
            _search_subscription: search_subscription,
            query: String::new(),
            open_choice: None,
            choice_scroll: ScrollState::new(),
            content_scroll,
            nav_scroll: ScrollState::new(),
            pending_scroll: false,
            sync_page_on_scroll: false,
            last_scroll_offset: px(0.0),
            editing: None,
            web,
            web_access,
            workspace_id: None,
            selected_web_host: None,
            web_message: String::new(),
            extension: None,
            extension_worker: None,
            extension_generation: 0,
            extension_edit: None,
            #[cfg(not(target_family = "wasm"))]
            extension_manager: None,
            extension_management_busy: false,
            capability_info: None,
            capability_generation: 0,
            capability_busy: false,
            #[cfg(not(target_family = "wasm"))]
            capability_manager: None,
        }
    }

    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn set_extension_manager(&mut self, manager: Option<crate::app::ExtensionManager>) {
        self.extension_manager = manager.map(ClientWorker::new);
    }

    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn set_extension_capability_manager(
        &mut self,
        manager: Option<crate::app::ExtensionCapabilityManager>,
    ) {
        self.capability_manager = manager.map(ClientWorker::new);
    }

    pub(crate) fn set_workspace(
        &mut self,
        root: Option<PathBuf>,
        workspace_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.root != root || self.workspace_id != workspace_id {
            self.root = root;
            self.workspace_id = workspace_id;
            self.capability_generation = self.capability_generation.wrapping_add(1);
            self.capability_busy = false;
            self.capability_info = None;
            if let Some(web) = &mut self.web {
                web.generation = web.generation.wrapping_add(1);
                web.status = None;
                web.devices.clear();
                web.busy = false;
            }
            if self.web.is_some() && self.root.is_some() {
                self.web_request(WebOperation::Refresh, cx);
            }
            cx.notify();
        }
    }

    fn t(&self, key: &str) -> String {
        self.locale.resolve(&localization::MessageId::new(key))
    }
}

impl SettingsFeature {
    fn render_workspace_info(&self, cx: &mut Context<Self>, workspace_info: String) -> gpui::Div {
        div()
            .w(px(600.0))
            .max_w_full()
            .p_4()
            .rounded(px(8.0))
            .border_1()
            .border_color(theme::colors(cx).border_default)
            .bg(theme::colors(cx).background_tertiary)
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme::colors(cx).text_tertiary)
                    .child(self.t("settings.workspace-info")),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme::colors(cx).text_primary)
                    .child(workspace_info),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme::colors(cx).text_tertiary)
                    .child("Version: 0.1.0"),
            )
    }
}

fn default_page() -> &'static str {
    settings::catalog()
        .first_page()
        .map_or("appearance.theme", |page| page.id)
}

fn scope_label(scope: &SettingsScope) -> &'static str {
    match scope {
        SettingsScope::User => "settings.scope.user",
        SettingsScope::Host { .. } => "settings.scope.host",
        SettingsScope::ProjectShared { .. } => "settings.scope.project-shared",
        SettingsScope::ProjectLocal { .. } => "settings.scope.project-local",
    }
}

fn value_text(value: &SettingValue) -> String {
    match value {
        SettingValue::Text(text) => text.clone(),
        SettingValue::Integer(number) => number.to_string(),
        SettingValue::Flag(flag) => flag.to_string(),
    }
}

/// 設定ページの描画に使う定義データ。テストからも同じものを参照する。
#[cfg(test)]
pub(super) fn catalog_for_tests() -> &'static settings::DefinitionCatalog {
    settings::catalog()
}

impl Render for SettingsFeature {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let handle = self.content_scroll.handle();
        let offset = handle.offset().y;
        if self.pending_scroll {
            self.pending_scroll = false;
        } else if (self.sync_page_on_scroll || offset != self.last_scroll_offset)
            && handle.bounds().size.height > px(0.0)
        {
            let pages = self.visible_pages();
            let max = crate::gpui_compat::max_offset(&handle).height;
            let index = if max > px(0.0) && -offset >= max - px(1.0) {
                pages.len().saturating_sub(1)
            } else {
                handle.top_item().min(pages.len().saturating_sub(1))
            };
            if let Some(page) = pages.get(index) {
                self.page = page;
            }
            self.sync_page_on_scroll = false;
        }
        self.last_scroll_offset = offset;
        div()
            .size_full()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .child(self.message.clone())
            .child(self.render_settings_content(cx))
    }
}
