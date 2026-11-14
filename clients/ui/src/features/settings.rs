use std::{path::PathBuf, sync::Arc};

use super::settings_coordinator::{Change, SettingsCoordinator};
use crate::client_adapter::ClientWorker;
use crate::{
    components::{Icon, IconName, ScrollAxis, ScrollState, scroll_items_viewport, scroll_viewport},
    localization::Localizer,
    theme,
};
use desktop_client::{HostDevice, HostWebStatus, WebAccessControl};
use gpui::{Context, Focusable, Render, Window, div, prelude::*, px};

#[derive(Clone)]
enum WebOperation {
    Refresh,
    Start(String),
    Stop,
    Invite,
    Revoke(String),
}

struct WebResult {
    status: Result<HostWebStatus, String>,
    devices: Result<Vec<HostDevice>, String>,
    invitation: Option<zeroize::Zeroizing<String>>,
}

struct WebState {
    worker: ClientWorker<Arc<dyn WebAccessControl>>,
    status: Option<HostWebStatus>,
    devices: Vec<HostDevice>,
    address: String,
    busy: bool,
    generation: u64,
    message: String,
}

use settings::{
    Label, OptionSource, ResolvedSettings, SettingControl, SettingDefinition, SettingKey,
    SettingOption, SettingValue, SettingsScope,
};

/// 左ナビ3層（分類 → 中間層 → ページ）と、定義データから組み立てる設定ページ。
pub(crate) struct SettingsFeature {
    coordinator: gpui::Entity<SettingsCoordinator>,
    _subscription: gpui::Subscription,
    _catalogs: Vec<gpui::Subscription>,
    values: ResolvedSettings,
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
        }
    }

    /// 候補を持たない値を編集する。確定するまで保存しない。
    fn begin_text_edit(&mut self, key: SettingKey, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.as_ref().is_some_and(|edit| edit.key == key) {
            return;
        }
        let current = self.values.text(key).unwrap_or_default().to_owned();
        let placeholder = self.t("settings.value.placeholder");
        let input = cx.new(|cx| crate::components::SearchInput::new(&placeholder, cx));
        input.update(cx, |input, cx| input.set_text(current.clone(), cx));
        crate::gpui_compat::focus(window, &input.read(cx).focus_handle(cx), cx);
        let subscription = cx.subscribe(&input, |this, _, event, cx| {
            let crate::components::SearchInputEvent::Changed(text) = event;
            if let Some(edit) = this.editing.as_mut() {
                edit.draft = text.clone();
            }
            cx.notify();
        });
        self.editing = Some(TextEdit {
            key,
            input,
            draft: current,
            _subscription: subscription,
        });
        cx.notify();
    }

    fn commit_text_edit(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = self.editing.take() else {
            return;
        };
        if edit.draft.trim().is_empty() {
            self.coordinator
                .update(cx, |coordinator, cx| coordinator.clear(edit.key, cx));
        } else {
            self.choose(edit.key, SettingValue::text(edit.draft), cx);
        }
    }

    fn cancel_text_edit(&mut self, cx: &mut Context<Self>) {
        if self.editing.take().is_some() {
            cx.notify();
        }
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

    /// 保存する値は定義データの候補から来るため、UI 側で型を組み立てない。
    ///
    /// 配布元が実際に変わったときだけ、以前の場所を同じ操作で消す。場所の意味は取得元
    /// ごとに変わるため、別の取得元へ古い path/URL を流用しない。同じ取得元の再選択では
    /// 保存済みの場所を残す。
    pub(crate) fn choose(&mut self, key: SettingKey, value: SettingValue, cx: &mut Context<Self>) {
        let source_changed = key == settings::UPDATE_SOURCE
            && self.values.text(settings::UPDATE_SOURCE) != value.as_text();
        let changes = if source_changed {
            Change::update(vec![
                (key, Some(value)),
                (settings::UPDATE_SOURCE_LOCATION, None),
            ])
        } else {
            Change::set(key, value)
        };
        self.coordinator
            .update(cx, |coordinator, cx| coordinator.change(changes, cx));
    }

    /// そのページをこの起動構成で表示できるか。
    ///
    /// 接続ページは供給元の有無ではなく取得結果で決める。供給元が `None` を返す
    /// 起動構成では、空のページをナビへ出さない。
    fn page_available(&self, page: &settings::SettingPage) -> bool {
        match page.content {
            settings::SettingPageContent::Feature => {
                page.id != settings::catalog::CONNECTION_WEB_PAGE || self.web_access().is_some()
            }
            settings::SettingPageContent::Settings => true,
        }
    }

    fn select_page(&mut self, page: &'static str, cx: &mut Context<Self>) {
        if let Some(index) = self
            .visible_pages()
            .iter()
            .position(|candidate| *candidate == page)
        {
            self.content_scroll.handle().scroll_to_top_of_item(index);
            self.pending_scroll = true;
        }
        if self.page != page {
            self.page = page;
            self.web_message.clear();
            if page == "host.web" {
                self.web_request(WebOperation::Refresh, cx);
            }
        }
        cx.notify();
    }

    fn visible_pages(&self) -> Vec<&'static str> {
        let mut pages = Vec::new();
        let catalog = settings::catalog();
        for category in catalog.categories() {
            for group in category.groups {
                for id in group.pages {
                    if let Some(page) = catalog.page(id).filter(|page| self.page_available(page))
                        && (self.matches_search(&[
                            self.t(category.label),
                            self.t(group.label),
                            self.t(page.label),
                            page.description.map(|key| self.t(key)).unwrap_or_default(),
                        ]) || catalog
                            .settings_of(page.id)
                            .any(|field| self.field_matches_search(field)))
                    {
                        pages.push(page.id);
                    }
                }
            }
        }
        if self.web.is_some()
            && self.matches_search(&[
                self.t("settings.web.publish-title"),
                self.t("settings.web.devices"),
                self.t("settings.web.address"),
                self.t("settings.web.invite"),
            ])
        {
            pages.push("host.web");
        }
        pages
    }

    fn matches_search(&self, fields: &[String]) -> bool {
        let text = fields.join(" ").to_lowercase();
        self.query
            .split_whitespace()
            .all(|term| text.contains(&term.to_lowercase()))
    }

    fn field_matches_search(&self, definition: &SettingDefinition) -> bool {
        let catalog = settings::catalog();
        let page = catalog.page(definition.page);
        let category = page.and_then(|page| {
            catalog
                .categories()
                .iter()
                .find(|category| category.id == page.category)
        });
        let group = page.and_then(|page| {
            category
                .and_then(|category| category.groups.iter().find(|group| group.id == page.group))
        });
        let mut fields = vec![
            category
                .map(|value| self.t(value.label))
                .unwrap_or_default(),
            group.map(|value| self.t(value.label)).unwrap_or_default(),
            page.map(|value| self.t(value.label)).unwrap_or_default(),
            definition.key.as_str().to_owned(),
            self.t(definition.label),
            definition
                .description
                .map(|key| self.t(key))
                .unwrap_or_default(),
        ];
        if let SettingControl::Choice(OptionSource::Values(options)) = &definition.control {
            fields.extend(options.iter().map(|option| self.option_label(option)));
        }
        self.matches_search(&fields)
    }

    fn page_heading_matches_search(&self, page: &settings::SettingPage) -> bool {
        let catalog = settings::catalog();
        let category = catalog
            .categories()
            .iter()
            .find(|category| category.id == page.category);
        let group = category
            .and_then(|category| category.groups.iter().find(|group| group.id == page.group));
        self.matches_search(&[
            category
                .map(|value| self.t(value.label))
                .unwrap_or_default(),
            group.map(|value| self.t(value.label)).unwrap_or_default(),
            self.t(page.label),
            page.description.map(|key| self.t(key)).unwrap_or_default(),
        ])
    }

    fn web_request(&mut self, operation: WebOperation, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let Some(web) = &mut self.web else { return };
        if web.busy {
            return;
        }
        web.busy = true;
        web.message.clear();
        let generation = web.generation;
        let worker = web.worker.clone();
        let admitted = worker.submit(
            cx,
            move |access| {
                let invitation = match operation {
                    WebOperation::Refresh => None,
                    WebOperation::Start(address) => {
                        access.start(&root, &address, 4747)?;
                        None
                    }
                    WebOperation::Stop => {
                        access.stop(&root)?;
                        None
                    }
                    WebOperation::Invite => Some(access.invite(&root)?),
                    WebOperation::Revoke(id) => {
                        access.revoke(&root, &id)?;
                        None
                    }
                };
                // 操作自体が成功していれば、状態と端末一覧の取得失敗で招征E��を捨てなぁE��E
                Ok(WebResult {
                    status: access.status(&root),
                    devices: access.devices(&root),
                    invitation,
                })
            },
            move |this, result, cx| {
                let copied = this.t("settings.web.copied");
                let Some(web) = &mut this.web else { return };
                if web.generation != generation {
                    return;
                }
                web.busy = false;
                match result {
                    Ok(result) => {
                        match result.status {
                            Ok(status) => {
                                if let Some(url) = &status.url
                                    && let Some(address) = status.addresses.iter().find(|address| {
                                        url.starts_with(&format!("http://{address}:"))
                                    })
                                {
                                    web.address = address.clone();
                                }
                                if !status.addresses.contains(&web.address) {
                                    web.address =
                                        status.addresses.first().cloned().unwrap_or_default();
                                }
                                web.message = status.error.clone().unwrap_or_default();
                                web.status = Some(status);
                            }
                            // 取得できなぁE��合は直前の状態を残し、停止中とはみなさなぁE��E
                            Err(error) => web.message = error,
                        }
                        match result.devices {
                            Ok(devices) => web.devices = devices,
                            Err(error) => {
                                if web.message.is_empty() {
                                    web.message = error;
                                }
                            }
                        }
                        if let Some(invitation) = result.invitation {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                invitation.to_string(),
                            ));
                            web.message = copied;
                        }
                    }
                    Err(error) => web.message = error,
                }
                cx.notify();
            },
        );
        if let Err(error) = admitted
            && let Some(web) = &mut self.web
        {
            web.busy = false;
            web.message = error;
        }
        cx.notify();
    }

    fn web_button(
        &self,
        id: &'static str,
        label: &str,
        operation: WebOperation,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(id)
            .debug_selector(|| id.into())
            .px_3()
            .py_2()
            .rounded(px(6.0))
            .bg(theme::colors(cx).background_tertiary)
            .text_color(if enabled {
                theme::colors(cx).text_primary
            } else {
                theme::colors(cx).text_tertiary
            })
            .child(self.t(label))
            .when(enabled, |button| {
                // focusable によりフォーカス中の Enter/Space が on_click を呼ぶ、E
                button.focusable().cursor_pointer().on_click(
                    cx.listener(move |this, _, _, cx| this.web_request(operation.clone(), cx)),
                )
            })
    }

    fn render_web_page(&self, cx: &mut Context<Self>) -> gpui::Div {
        let Some(web) = &self.web else { return div() };
        let available = self.root.is_some() && !web.busy;
        // 未取得・取得失敗は「停止中」と区別し、開始操作を有効にしない。
        let known = web.status.is_some();
        let url = web.status.as_ref().and_then(|status| status.url.as_ref());
        let addresses = web
            .status
            .as_ref()
            .map(|status| status.addresses.as_slice())
            .unwrap_or(&[]);
        let status_text = match &web.status {
            Some(status) => status
                .url
                .clone()
                .unwrap_or_else(|| self.t("settings.web.stopped")),
            // 未取得または取得失敗を「停止中」と混同しなぁE��E
            None => self.t("settings.web.unknown"),
        };
        let mut content = div()
            .w_full()
            .min_w(px(0.0))
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .text_size(px(16.0))
                    .font_weight(gpui::FontWeight::BOLD)
                    .child(self.t("settings.web.publish-title")),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme::colors(cx).text_tertiary)
                    .child(self.t("settings.web.description")),
            )
            .child(div().text_size(px(12.0)).child(status_text))
            .children((self.root.is_none()).then(|| {
                div()
                    .text_color(theme::colors(cx).text_tertiary)
                    .child(self.t("settings.no-workspace"))
            }))
            .child(
                div()
                    .text_size(px(12.0))
                    .child(self.t("settings.web.address")),
            )
            .children(addresses.iter().enumerate().map(|(index, address)| {
                let selected = *address == web.address;
                let address = address.clone();
                div()
                    .id(("web-address", index))
                    .px_3()
                    .py_2()
                    .rounded(px(6.0))
                    .bg(if selected {
                        theme::colors(cx).button_background_selected
                    } else {
                        theme::colors(cx).background_tertiary
                    })
                    .child(address.clone())
                    .when(available && url.is_none(), |row| {
                        row.cursor_pointer().focusable().on_click(cx.listener(
                            move |this, _, _, cx| {
                                if let Some(web) = &mut this.web {
                                    web.address = address.clone();
                                }
                                cx.notify();
                            },
                        ))
                    })
            }))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(self.web_button(
                        "web-refresh",
                        "settings.web.refresh",
                        WebOperation::Refresh,
                        available,
                        cx,
                    ))
                    .child(self.web_button(
                        "web-start",
                        "settings.web.start",
                        WebOperation::Start(web.address.clone()),
                        available && known && url.is_none() && !web.address.is_empty(),
                        cx,
                    ))
                    .child(self.web_button(
                        "web-stop",
                        "settings.web.stop",
                        WebOperation::Stop,
                        available && known && url.is_some(),
                        cx,
                    ))
                    .child(self.web_button(
                        "web-invite",
                        "settings.web.invite",
                        WebOperation::Invite,
                        available && url.is_some(),
                        cx,
                    )),
            )
            .child(div().text_size(px(12.0)).child(web.message.clone()))
            .child(
                div()
                    .text_size(px(14.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(self.t("settings.web.devices")),
            );
        for (index, device) in web.devices.iter().enumerate() {
            let id = device.id.clone();
            let label = format!("{}  ·  {}  ·  {}", device.name, device.kind, device.status);
            let can_revoke = available && !device.is_current && device.status != "revoked";
            content = content.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(div().text_size(px(12.0)).child(label))
                    .child(if device.is_current {
                        div()
                            .text_size(px(11.0))
                            .child(self.t("settings.web.this-device"))
                            .into_any_element()
                    } else {
                        div()
                            .id(("web-revoke", index))
                            .px_3()
                            .py_2()
                            .rounded(px(6.0))
                            .text_color(if can_revoke {
                                theme::colors(cx).text_primary
                            } else {
                                theme::colors(cx).text_tertiary
                            })
                            .child(self.t("settings.web.revoke"))
                            .when(can_revoke, |button| {
                                button.focusable().cursor_pointer().on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.web_request(WebOperation::Revoke(id.clone()), cx)
                                    },
                                ))
                            })
                            .into_any_element()
                    }),
            );
        }
        content
    }

    /// 現在の接続情報。供給元が無い、または取得できない場合は `None`。
    fn web_access(&self) -> Option<desktop_client::WebAccess> {
        self.web_access.as_ref()?.as_ref()()
    }

    /// 表示中の候補アドレス。選んだホストが消えていれば先頭へ戻す。
    ///
    /// 1回の描画では同じ `access` を使い、候補と URL が食い違わないようにする。
    fn selected_web_address(
        &self,
        access: &desktop_client::WebAccess,
    ) -> Option<desktop_client::WebAddress> {
        match self.selected_web_host.as_deref() {
            Some(host)
                if access
                    .addresses()
                    .iter()
                    .any(|address| address.host == host) =>
            {
                access
                    .addresses()
                    .iter()
                    .find(|address| address.host == host)
                    .cloned()
            }
            _ => access.preferred().cloned(),
        }
    }

    fn select_web_host(&mut self, host: String, cx: &mut Context<Self>) {
        if self.selected_web_host.as_deref() != Some(host.as_str()) {
            self.selected_web_host = Some(host);
            self.web_message.clear();
            cx.notify();
        }
    }

    /// 表示中のページ URL をクリップボードへ書き、画面にも知らせる。
    fn copy_web_url(&mut self, value: String, copied_key: &'static str, cx: &mut Context<Self>) {
        if value.is_empty() {
            return;
        }
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(value));
        self.web_message = self.t(copied_key);
        cx.notify();
    }

    pub(super) fn render_settings_content(&self, cx: &mut Context<Self>) -> gpui::Div {
        let workspace_info = self
            .root
            .as_deref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| self.t("settings.no-workspace"));
        let nav = self.render_nav(cx).into_any_element();
        let mut items = self
            .visible_pages()
            .into_iter()
            .map(|page| self.render_page(page, cx).flex_shrink_0())
            .collect::<Vec<_>>();
        if items.is_empty() {
            items.push(
                div()
                    .text_size(px(13.0))
                    .text_color(theme::colors(cx).text_tertiary)
                    .child(self.t("settings.search.empty")),
            );
        } else if self.query.trim().is_empty() {
            items.push(
                self.render_workspace_info(cx, workspace_info)
                    .flex_shrink_0(),
            );
        }
        let on_scroll = cx.listener(|this, _, _, cx| {
            this.sync_page_on_scroll = true;
            cx.notify();
        });
        let content = scroll_items_viewport(
            "settings-content-scroll",
            &self.content_scroll,
            items,
            on_scroll,
            cx,
        );

        div()
            .size_full()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .items_center()
            .pt_8()
            .px_4()
            .gap_4()
            .child(
                div()
                    .id("settings-search")
                    .debug_selector(|| "settings-search".into())
                    .w_full()
                    .max_w(px(920.0))
                    .child(self.search.clone()),
            )
            .child(
                div()
                    .w_full()
                    .max_w(px(920.0))
                    .flex_1()
                    .min_h(px(0.0))
                    .flex()
                    .gap_6()
                    .child(
                        div()
                            .w(px(220.0))
                            .flex_shrink_0()
                            .h_full()
                            .child(scroll_viewport(
                                "settings-nav-scroll",
                                ScrollAxis::Vertical,
                                &self.nav_scroll,
                                nav,
                                cx,
                            )),
                    )
                    .child(div().flex_1().min_w(px(0.0)).h_full().child(content)),
            )
    }

    /// 分類と中間層は分類専用の行として並べ、ページだけを選べるようにする。
    fn render_nav(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let catalog = settings::catalog();
        let visible = self.visible_pages();
        div()
            .id("main-settings-nav")
            .debug_selector(|| "main-settings-nav".into())
            .w(px(220.0))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_1()
            .children(
                catalog
                    .categories()
                    .iter()
                    .filter(|category| {
                        category
                            .groups
                            .iter()
                            .any(|group| group.pages.iter().any(|id| visible.contains(id)))
                    })
                    .map(|category| {
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .px_3()
                                    .pt_3()
                                    .pb_1()
                                    .text_size(px(11.0))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(theme::colors(cx).text_tertiary)
                                    .child(self.t(category.label)),
                            )
                            .children(
                                category
                                    .groups
                                    .iter()
                                    .filter(|group| {
                                        group.pages.iter().any(|id| visible.contains(id))
                                    })
                                    .map(|group| {
                                        div()
                                            .flex()
                                            .flex_col()
                                            .child(
                                                div()
                                                    .px_5()
                                                    .py_1()
                                                    .text_size(px(11.0))
                                                    .text_color(theme::colors(cx).text_tertiary)
                                                    .child(self.t(group.label)),
                                            )
                                            .children(
                                                group
                                                    .pages
                                                    .iter()
                                                    .filter_map(|id| catalog.page(id))
                                                    .filter(|page| visible.contains(&page.id))
                                                    .map(|page| {
                                                        let selected = page.id == self.page;
                                                        div()
                                                            .id(page.id)
                                                            .px_5()
                                                            .py_2()
                                                            .rounded(px(6.0))
                                                            .when(selected, |row| {
                                                                row.bg(theme::colors(cx)
                                                                    .button_background_selected)
                                                            })
                                                            .hover(|style| {
                                                                style.bg(theme::colors(cx)
                                                                    .button_background_hover)
                                                            })
                                                            .text_size(px(12.0))
                                                            .text_color(
                                                                theme::colors(cx).text_primary,
                                                            )
                                                            .child(self.t(page.label))
                                                            .on_click(cx.listener(
                                                                move |this, _, _, cx| {
                                                                    this.select_page(page.id, cx)
                                                                },
                                                            ))
                                                    }),
                                            )
                                    }),
                            )
                    }),
            )
            .when(visible.contains(&"host.web"), |nav| {
                let selected = self.page == "host.web";
                nav.child(
                    div()
                        .id("settings-host-web")
                        .debug_selector(|| "settings-host-web".into())
                        .px_5()
                        .py_2()
                        .rounded(px(6.0))
                        .when(selected, |row| {
                            row.bg(theme::colors(cx).button_background_selected)
                        })
                        .hover(|style| style.bg(theme::colors(cx).button_background_hover))
                        .text_size(px(12.0))
                        .text_color(theme::colors(cx).text_primary)
                        .child(self.t("settings.web.publish-title"))
                        .focusable()
                        .on_click(cx.listener(|this, _, _, cx| this.select_page("host.web", cx))),
                )
            })
    }
    fn render_page(&self, page_id: &'static str, cx: &mut Context<Self>) -> gpui::Div {
        if page_id == "host.web" {
            return self.render_web_page(cx);
        }
        let catalog = settings::catalog();
        if page_id == settings::catalog::CONNECTION_WEB_PAGE && self.web_access().is_some() {
            return self.render_web_connection_page(cx);
        }
        let page = catalog.page(page_id).or_else(|| catalog.first_page());
        let mut container = div().w_full().min_w(px(0.0)).flex().flex_col().gap_4();
        let Some(page) = page else {
            return container.child(self.t("settings.empty"));
        };
        container = container.child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_size(px(16.0))
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(theme::colors(cx).text_primary)
                        .child(self.t(page.label)),
                )
                .children(page.description.map(|description| {
                    div()
                        .text_size(px(12.0))
                        .text_color(theme::colors(cx).text_tertiary)
                        .child(self.t(description))
                })),
        );
        let show_page = self.page_heading_matches_search(page);
        let fields = catalog
            .settings_of(page.id)
            .filter(|definition| show_page || self.field_matches_search(definition))
            .map(|definition| self.render_field(definition, cx))
            .collect::<Vec<_>>();
        if !fields.is_empty() {
            container = container.child(
                div()
                    .w_full()
                    .p_4()
                    .rounded(px(8.0))
                    .bg(theme::colors(cx).background_tertiary)
                    .flex()
                    .flex_col()
                    .gap_3()
                    .children(fields),
            );
        }
        if let Some(edit) = &self.editing
            && catalog
                .settings_of(page.id)
                .any(|definition| definition.key == edit.key)
        {
            let input = edit.input.clone();
            let key = edit.key.as_str();
            container = container.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_4()
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(theme::colors(cx).border_default)
                    .bg(theme::colors(cx).background_tertiary)
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(theme::colors(cx).text_tertiary)
                            .child(self.t("settings.value.edit-description")),
                    )
                    .child(input)
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                div()
                                    .id(gpui::SharedString::from(format!(
                                        "settings-text-save-{key}"
                                    )))
                                    .px_3()
                                    .py_2()
                                    .rounded(px(6.0))
                                    .bg(theme::colors(cx).button_background_selected)
                                    .text_size(px(12.0))
                                    .text_color(theme::colors(cx).text_accent)
                                    .child(self.t("settings.value.save"))
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.commit_text_edit(cx)),
                                    ),
                            )
                            .child(
                                div()
                                    .id(gpui::SharedString::from(format!(
                                        "settings-text-cancel-{key}"
                                    )))
                                    .px_3()
                                    .py_2()
                                    .rounded(px(6.0))
                                    .border_1()
                                    .border_color(theme::colors(cx).border_default)
                                    .text_size(px(12.0))
                                    .text_color(theme::colors(cx).text_primary)
                                    .child(self.t("settings.value.cancel"))
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.cancel_text_edit(cx)),
                                    ),
                            ),
                    )
                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                        if event.keystroke.key != "enter" {
                            return;
                        }
                        cx.stop_propagation();
                        this.commit_text_edit(cx);
                    })),
            );
        }
        container
    }

    /// 現在の接続情報を示すページ。保存値を持たないため、表示のたびに集め直す。
    ///
    /// コピーと「ブラウザーで開く」は同じ候補アドレスを対象にする。
    fn render_web_connection_page(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut container = div()
            .w_full()
            .min_w(px(0.0))
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(16.0))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(theme::colors(cx).text_primary)
                            .child(self.t("settings.page.connection.web")),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(theme::colors(cx).text_tertiary)
                            .child(self.t("settings.page.connection.web.description")),
                    ),
            );
        let Some(access) = self.web_access() else {
            return container.child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme::colors(cx).text_tertiary)
                    .child(self.t("settings.web.unavailable")),
            );
        };
        let access = access.with_loopback_fallback();
        let selected = self.selected_web_address(&access);
        let page_url = access.page_url(selected.as_ref());
        let bridge_url = access.bridge_url(selected.as_ref());

        container =
            container.child(
                div()
                    .p_4()
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(theme::colors(cx).border_default)
                    .bg(theme::colors(cx).background_tertiary)
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(theme::colors(cx).text_tertiary)
                            .child(self.t("settings.web.address")),
                    )
                    .child(div().flex().flex_wrap().gap_2().children(
                        access.addresses().iter().map(|address| {
                            let host = address.host.clone();
                            let selected = selected
                                .as_ref()
                                .is_some_and(|current| current.same_host(address));
                            div()
                                .id(gpui::ElementId::Name(format!("web-address-{host}").into()))
                                .debug_selector({
                                    let host = host.clone();
                                    move || format!("web-address-{host}")
                                })
                                .px_3()
                                .py_2()
                                .rounded(px(6.0))
                                .border_1()
                                .border_color(if selected {
                                    theme::colors(cx).text_accent
                                } else {
                                    theme::colors(cx).border_default
                                })
                                .bg(if selected {
                                    theme::colors(cx).button_background_selected
                                } else {
                                    theme::colors(cx).background_secondary
                                })
                                .hover(|style| style.bg(theme::colors(cx).button_background_hover))
                                .text_size(px(12.0))
                                .text_color(theme::colors(cx).text_primary)
                                .child(format!(
                                    "{} {}",
                                    self.t(address.kind.label_key()),
                                    address.host
                                ))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.select_web_host(host.clone(), cx)
                                }))
                        }),
                    )),
            );

        container = container.child(self.render_web_value_row(
            "web-page-url",
            "settings.web.page-url",
            page_url.clone(),
            "settings.web.copy-page-url",
            "settings.web.copied",
            cx,
        ));
        container = container.child(self.render_web_value_row(
            "web-bridge-url",
            "settings.web.bridge-url",
            bridge_url.clone(),
            "settings.web.copy-bridge-url",
            "settings.web.copied",
            cx,
        ));

        container = container.child(
            div()
                .flex()
                .items_center()
                .flex_wrap()
                .gap_2()
                .child(
                    div()
                        .id("web-open-browser")
                        .debug_selector(|| "web-open-browser".into())
                        .px_3()
                        .py_2()
                        .rounded(px(6.0))
                        .border_1()
                        .border_color(theme::colors(cx).border_default)
                        .bg(theme::colors(cx).background_secondary)
                        .text_size(px(12.0))
                        .text_color(if page_url.is_some() {
                            theme::colors(cx).text_primary
                        } else {
                            theme::colors(cx).text_tertiary
                        })
                        .child(self.t("settings.web.open"))
                        .when_some(page_url, |button, url| {
                            button
                                .cursor_pointer()
                                .hover(|style| style.bg(theme::colors(cx).button_background_hover))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.open_url(&url);
                                    this.web_message = this.t("settings.web.opened");
                                    cx.notify();
                                }))
                        }),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(theme::colors(cx).text_tertiary)
                        .child(self.web_message.clone()),
                ),
        );

        // Desktop は接続中の Workspace をそのまま初回入力の候補として出す。
        if let Some(workspace_id) = access.workspace_id.or_else(|| self.workspace_id.clone()) {
            container = container.child(self.render_web_value_row(
                "web-workspace-id",
                "settings.web.workspace-id",
                Some(workspace_id),
                "settings.web.copy-bridge-url",
                "settings.web.copied",
                cx,
            ));
        }
        container.child(
            div()
                .text_size(px(11.0))
                .text_color(theme::colors(cx).text_tertiary)
                .child(self.t("settings.web.hint")),
        )
    }

    /// 値・コピー・動作の3点を1行に並べる。値が無いときはボタンを無効にする。
    fn render_web_value_row(
        &self,
        id: &'static str,
        label: &'static str,
        value: Option<String>,
        copy_label: &'static str,
        copied_label: &'static str,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let shown = value.clone().unwrap_or_else(|| "-".to_owned());
        div()
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
                    .child(self.t(label)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        div()
                            .id(id)
                            .debug_selector(move || id.into())
                            .flex_1()
                            .min_w(px(0.0))
                            .text_size(px(12.0))
                            .text_color(theme::colors(cx).text_primary)
                            .child(shown),
                    )
                    .child(
                        div()
                            .id(gpui::ElementId::Name(format!("{id}-copy").into()))
                            .debug_selector(move || format!("{id}-copy"))
                            .px_3()
                            .py_2()
                            .rounded(px(6.0))
                            .border_1()
                            .border_color(theme::colors(cx).border_default)
                            .bg(theme::colors(cx).background_secondary)
                            .text_size(px(12.0))
                            .text_color(if value.is_some() {
                                theme::colors(cx).text_primary
                            } else {
                                theme::colors(cx).text_tertiary
                            })
                            .child(self.t(copy_label))
                            .when_some(value, |button, value| {
                                button
                                    .cursor_pointer()
                                    .hover(|style| {
                                        style.bg(theme::colors(cx).button_background_hover)
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.copy_web_url(value.clone(), copied_label, cx)
                                    }))
                            }),
                    ),
            )
    }

    /// 1項目は定義データだけで決まる。値の保存キーも定義から取る。
    fn render_field(&self, definition: &SettingDefinition, cx: &mut Context<Self>) -> gpui::Div {
        let key = definition.key;
        let current = self.effective_value(definition, cx);
        let origin = self.values.origin(key).map_or_else(
            || self.t("settings.scope.default"),
            |scope| self.t(scope_label(scope)),
        );
        let label = div()
            .w(px(150.0))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_1()
            .text_size(px(13.0))
            .text_color(theme::colors(cx).text_primary)
            .child(self.t(definition.label))
            .children(definition.description.map(|description| {
                div()
                    .text_size(px(11.0))
                    .text_color(theme::colors(cx).text_tertiary)
                    .child(self.t(description))
            }));
        let control = match &definition.control {
            SettingControl::Choice(_) => {
                let options = self.options(definition, cx);
                let selected = options
                    .iter()
                    .find(|(value, _)| Some(value) == current.as_ref())
                    .map(|(_, label)| label.clone())
                    .unwrap_or_else(|| self.t("settings.value.unset"));
                let open = self.open_choice == Some(key);
                let choice_height = px((options.len() as f32 * 34.0).clamp(34.0, 200.0));
                let keyboard_options = options.clone();
                let keyboard_current = current.clone();
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .id(gpui::SharedString::from(format!(
                                "settings-choice-{}",
                                key.as_str()
                            )))
                            .debug_selector(move || format!("settings-choice-{}", key.as_str()))
                            .w_full()
                            .px_3()
                            .py_2()
                            .rounded(px(4.0))
                            .border_1()
                            .border_color(theme::colors(cx).border_default)
                            .bg(theme::colors(cx).background_secondary)
                            .hover(|style| style.bg(theme::colors(cx).button_background_hover))
                            .cursor_pointer()
                            .focusable()
                            .flex()
                            .items_center()
                            .justify_between()
                            .text_size(px(12.0))
                            .text_color(theme::colors(cx).text_primary)
                            .child(selected)
                            .child(Icon::new(IconName::ChevronDown))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_choice = if this.open_choice == Some(key) {
                                    None
                                } else {
                                    Some(key)
                                };
                                this.choice_scroll
                                    .handle()
                                    .set_offset(gpui::point(px(0.0), px(0.0)));
                                cx.notify();
                            }))
                            .on_key_down(cx.listener(
                                move |this, event: &gpui::KeyDownEvent, _, cx| {
                                    if event.keystroke.key == "enter"
                                        || event.keystroke.key == "space"
                                    {
                                        cx.stop_propagation();
                                        this.open_choice = if this.open_choice == Some(key) {
                                            None
                                        } else {
                                            Some(key)
                                        };
                                        this.choice_scroll
                                            .handle()
                                            .set_offset(gpui::point(px(0.0), px(0.0)));
                                        cx.notify();
                                    } else if event.keystroke.key == "escape"
                                        && this.open_choice == Some(key)
                                    {
                                        cx.stop_propagation();
                                        this.open_choice = None;
                                        cx.notify();
                                    } else if matches!(event.keystroke.key.as_str(), "up" | "down")
                                        && !keyboard_options.is_empty()
                                    {
                                        cx.stop_propagation();
                                        let index = keyboard_options
                                            .iter()
                                            .position(|(value, _)| {
                                                Some(value) == keyboard_current.as_ref()
                                            })
                                            .unwrap_or(0);
                                        let next = if event.keystroke.key == "down" {
                                            (index + 1).min(keyboard_options.len() - 1)
                                        } else {
                                            index.saturating_sub(1)
                                        };
                                        this.choose(key, keyboard_options[next].0.clone(), cx);
                                    }
                                },
                            )),
                    )
                    .when(open, |control| {
                        let choices = div().w_full().flex().flex_col().children(
                            options
                                .into_iter()
                                .enumerate()
                                .map(|(index, (value, label))| {
                                    let selected = current.as_ref() == Some(&value);
                                    div()
                                        .id(gpui::SharedString::from(format!(
                                            "{}-{index}",
                                            key.as_str()
                                        )))
                                        .debug_selector(move || format!("{}-{index}", key.as_str()))
                                        .px_3()
                                        .py_2()
                                        .rounded(px(4.0))
                                        .bg(if selected {
                                            theme::colors(cx).button_background_selected
                                        } else {
                                            theme::colors(cx).background_secondary
                                        })
                                        .hover(|style| {
                                            style.bg(theme::colors(cx).button_background_hover)
                                        })
                                        .cursor_pointer()
                                        .text_size(px(12.0))
                                        .text_color(theme::colors(cx).text_primary)
                                        .child(label)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.open_choice = None;
                                            this.choose(key, value.clone(), cx);
                                        }))
                                }),
                        );
                        control.child(div().h(choice_height).child(scroll_viewport(
                            gpui::SharedString::from(format!("settings-choices-{}", key.as_str())),
                            ScrollAxis::Vertical,
                            &self.choice_scroll,
                            choices,
                            cx,
                        )))
                    })
                    .into_any_element()
            }
            SettingControl::Text => {
                let value_text = current
                    .as_ref()
                    .and_then(|value| value.as_text().map(ToOwned::to_owned))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| self.t("settings.value.unset"));
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id(gpui::SharedString::from(format!(
                                "settings-value-{}",
                                key.as_str()
                            )))
                            .flex_1()
                            .min_w(px(0.0))
                            .px_3()
                            .py_2()
                            .rounded(px(4.0))
                            .border_1()
                            .border_color(theme::colors(cx).border_default)
                            .bg(theme::colors(cx).background_secondary)
                            .text_size(px(12.0))
                            .child(value_text),
                    )
                    .child(
                        div()
                            .id(gpui::SharedString::from(format!(
                                "settings-edit-{}",
                                key.as_str()
                            )))
                            .px_3()
                            .py_2()
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .text_size(px(12.0))
                            .text_color(theme::colors(cx).text_accent)
                            .child(self.t("settings.value.edit"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.begin_text_edit(key, window, cx)
                            })),
                    )
                    .into_any_element()
            }
        };
        div()
            .w_full()
            .flex()
            .items_start()
            .gap_4()
            .child(label)
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(control)
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(theme::colors(cx).text_tertiary)
                            .child(origin),
                    ),
            )
    }

    /// 未保存の項目は、供給元が示す実行時の既定値を選択状態として見せる。
    fn effective_value(
        &self,
        definition: &SettingDefinition,
        cx: &gpui::App,
    ) -> Option<SettingValue> {
        if let Some(value) = self.values.value(definition.key) {
            return Some(value.clone());
        }
        // 実行時の既定値を持つキーは、その機能が登録した供給元から取る。
        if let Some(value) =
            super::settings_sources::SettingsSources::default_value(cx, definition.key)
        {
            return Some(value);
        }
        if definition.key == settings::TERMINAL_CARET_SHAPE {
            let default = settings::TerminalCaretShape::default();
            return Some(SettingValue::text(default.as_id()));
        }
        None
    }

    /// 候補は定義データが指す供給元から取る。項目固有の分岐はここだけに閉じる。
    fn options(
        &self,
        definition: &SettingDefinition,
        cx: &mut Context<Self>,
    ) -> Vec<(SettingValue, String)> {
        let SettingControl::Choice(source) = &definition.control else {
            return Vec::new();
        };
        match source {
            OptionSource::Values(options) => options
                .iter()
                .map(|option| (option.value.clone(), self.option_label(option)))
                .collect(),
            OptionSource::Themes => theme::available(cx)
                .into_iter()
                .map(|(id, name)| (SettingValue::text(id.as_str()), name))
                .collect(),
            OptionSource::Locales => vec![
                (SettingValue::text("ja-JP"), "日本語 (Japanese)".to_owned()),
                (SettingValue::text("en-US"), "English (英語)".to_owned()),
            ],
            // 候補が実行時の状態に依存するキーは、機能が登録した供給元から取る。
            // 設定 UI は供給元の型を知らない。
            OptionSource::Provided => {
                super::settings_sources::SettingsSources::options(cx, definition.key)
            }
        }
    }

    fn option_label(&self, option: &SettingOption) -> String {
        match option.label {
            Some(Label::Key(key)) => self.t(key),
            Some(Label::Literal(text)) => text.to_owned(),
            None => value_text(&option.value),
        }
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use extension_ui_contract::ThemeId;
    use gpui::AppContext;
    use settings::{SettingsDocument, SettingsError, SettingsRepository, SyntaxTheme};
    use std::sync::Arc;

    struct MemorySettings;

    #[gpui::test]
    fn search_filters_settings_and_restores_them_when_cleared(cx: &mut gpui::TestAppContext) {
        let feature = cx.update(|cx| {
            theme::init(&ThemeId::new("lapis.dark"), None, cx);
            let settings = desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
            SettingsCoordinator::init(settings, cx);
            cx.new(SettingsFeature::new)
        });
        cx.update(|cx| {
            let search = feature.read(cx).search.clone();
            search.update(cx, |search, cx| {
                search.set_text("terminal font size".into(), cx)
            });
        });
        cx.run_until_parked();
        cx.update(|cx| {
            let feature = feature.read(cx);
            assert_eq!(feature.visible_pages(), vec!["terminal.font"]);
            let catalog = settings::catalog();
            let size = catalog
                .settings_of("terminal.font")
                .find(|field| field.key == settings::TERMINAL_FONT_SIZE)
                .unwrap();
            let family = catalog
                .settings_of("terminal.font")
                .find(|field| field.key == settings::TERMINAL_FONT_FAMILY)
                .unwrap();
            assert!(feature.field_matches_search(size));
            assert!(!feature.field_matches_search(family));
        });
        cx.update(|cx| {
            let search = feature.read(cx).search.clone();
            search.update(cx, |search, cx| {
                search.set_text("no-such-setting-xyz".into(), cx)
            });
        });
        cx.run_until_parked();
        cx.update(|cx| assert!(feature.read(cx).visible_pages().is_empty()));
        cx.update(|cx| {
            let search = feature.read(cx).search.clone();
            search.update(cx, |search, cx| search.set_text(String::new(), cx));
        });
        cx.run_until_parked();
        cx.update(|cx| assert!(feature.read(cx).visible_pages().len() > 1));
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn custom_choice_opens_and_saves_a_value(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            theme::init(&ThemeId::new("lapis.dark"), None, cx);
            let settings = desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
            SettingsCoordinator::init(settings, cx);
        });
        let (feature, visual) = cx.add_window_view(|_, cx| SettingsFeature::new(cx));
        let expected = visual.update(|_, cx| {
            feature.update(cx, |feature, cx| {
                let definition = settings::catalog()
                    .settings_of("appearance.theme")
                    .next()
                    .unwrap();
                feature.options(definition, cx)[1].0.clone()
            })
        });
        visual.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear();
        });
        assert!(visual.debug_bounds("settings-search").is_some());
        let choice = visual
            .debug_bounds("settings-choice-ui.theme")
            .expect("theme choice");
        visual.simulate_click(choice.center(), gpui::Modifiers::none());
        visual.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear();
        });
        let option = visual.debug_bounds("ui.theme-1").expect("second theme");
        visual.simulate_click(option.center(), gpui::Modifiers::none());
        feature.read_with(visual, |feature, _| assert!(feature.open_choice.is_none()));
        wait_for_settings(cx);
        cx.update(|cx| {
            assert_eq!(
                feature.read(cx).values.value(settings::UI_THEME),
                Some(&expected)
            )
        });
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn navigation_scrolls_to_sections_without_moving_navigation(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            theme::init(&ThemeId::new("lapis.dark"), None, cx);
            let settings = desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
            SettingsCoordinator::init(settings, cx);
        });
        let (feature, visual) = cx.add_window_view(|_, cx| SettingsFeature::new(cx));
        visual.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear();
        });
        let nav = visual
            .debug_bounds("main-settings-nav")
            .expect("navigation");
        let target = feature.read_with(visual, |feature, _| {
            *feature.visible_pages().last().unwrap()
        });
        visual.update(|_, cx| feature.update(cx, |feature, cx| feature.select_page(target, cx)));
        visual.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear();
        });
        feature.read_with(visual, |feature, _| {
            assert!(feature.content_scroll.handle().offset().y < px(0.0));
            assert_eq!(feature.page, target);
        });
        visual.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear();
        });
        feature.read_with(visual, |feature, _| assert_eq!(feature.page, target));
        assert_eq!(visual.debug_bounds("main-settings-nav"), Some(nav));
        let position = feature.read_with(visual, |feature, _| {
            feature.content_scroll.handle().bounds().center()
        });
        visual.simulate_event(gpui::ScrollWheelEvent {
            position,
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.0), px(10000.0))),
            modifiers: gpui::Modifiers::none(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        visual.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear();
        });
        feature.read_with(visual, |feature, _| {
            assert_eq!(feature.page, feature.visible_pages()[0]);
        });
        // スクロールバーのドラッグは wheel event を発行しない。
        visual.update(|window, cx| {
            let handle = feature.read(cx).content_scroll.handle();
            let max = crate::gpui_compat::max_offset(&handle).height;
            handle.set_offset(gpui::point(px(0.0), -max / 2.0));
            window.refresh();
            window.draw(cx).clear();
        });
        feature.read_with(visual, |feature, _| {
            let pages = feature.visible_pages();
            let index = feature
                .content_scroll
                .handle()
                .top_item()
                .min(pages.len() - 1);
            assert_eq!(feature.page, pages[index]);
        });
    }
    impl SettingsRepository for MemorySettings {
        fn load(&self) -> Result<SettingsDocument, SettingsError> {
            Ok(SettingsDocument::empty(SettingsScope::User))
        }
        fn save(&self, _: &SettingsDocument) -> Result<(), SettingsError> {
            Ok(())
        }
    }

    fn wait_for_settings(cx: &mut gpui::TestAppContext) {
        let coordinator = cx.update(|cx| SettingsCoordinator::shared(cx));
        let deadline = crate::time::Instant::now() + std::time::Duration::from_secs(3);
        while cx.update(|cx| !coordinator.read(cx).test_is_idle()) {
            assert!(
                crate::time::Instant::now() < deadline,
                "settings queue did not drain"
            );
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(20));
            cx.run_until_parked();
            std::thread::yield_now();
        }
    }

    #[gpui::test]
    fn changes_from_different_windows_keep_the_other_theme_selection(
        cx: &mut gpui::TestAppContext,
    ) {
        let (first, second) = cx.update(|cx| {
            theme::init(&ThemeId::new("lapis.dark"), None, cx);
            let settings = desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
            SettingsCoordinator::init(settings, cx);
            (cx.new(SettingsFeature::new), cx.new(SettingsFeature::new))
        });
        cx.run_until_parked();
        cx.update(|cx| {
            first.update(cx, |feature, cx| {
                feature.choose(settings::UI_THEME, SettingValue::text("lapis.white"), cx)
            });
            second.update(cx, |feature, cx| {
                feature.choose(
                    settings::UI_SYNTAX_THEME,
                    SettingValue::text(SyntaxTheme::Zed.as_id()),
                    cx,
                )
            });
        });
        wait_for_settings(cx);
        cx.update(|cx| {
            assert_eq!(theme::active_id(cx).as_str(), "lapis.white");
            assert_eq!(theme::syntax::active_family(cx), SyntaxTheme::Zed);
        });
    }

    #[gpui::test]
    fn two_features_observe_shared_theme_syntax_and_locale(cx: &mut gpui::TestAppContext) {
        let (first, second) = cx.update(|cx| {
            theme::init(&ThemeId::new("lapis.dark"), None, cx);
            let settings = desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
            SettingsCoordinator::init(settings, cx);
            (cx.new(SettingsFeature::new), cx.new(SettingsFeature::new))
        });
        cx.run_until_parked();
        cx.update(|cx| {
            first.update(cx, |feature, cx| {
                feature.choose(settings::UI_THEME, SettingValue::text("lapis.white"), cx)
            });
            second.update(cx, |feature, cx| {
                feature.choose(
                    settings::UI_SYNTAX_THEME,
                    SettingValue::text(SyntaxTheme::Zed.as_id()),
                    cx,
                )
            });
            first.update(cx, |feature, cx| {
                feature.choose(settings::UI_LOCALE, SettingValue::text("ja-JP"), cx)
            });
        });
        wait_for_settings(cx);
        cx.update(|cx| {
            for feature in [&first, &second] {
                let values = &feature.read(cx).values;
                assert_eq!(values.theme(), "lapis.white");
                assert_eq!(values.syntax_theme(), Some(SyntaxTheme::Zed));
                assert_eq!(values.locale().as_str(), "ja-JP");
            }
        });
    }

    #[gpui::test]
    fn settings_navigation_renders_only_leaf_pages(cx: &mut gpui::TestAppContext) {
        let catalog = catalog_for_tests();
        let leaf = catalog
            .categories()
            .iter()
            .flat_map(|category| category.groups.iter())
            .flat_map(|group| group.pages.iter())
            .count();
        assert_eq!(leaf, catalog.pages().len());
        assert!(catalog.first_page().is_some());
        let _ = cx;
    }

    /// Host が公開した profile が Settings の候補になることを、描画と同じ経路で確かめる。
    #[gpui::test]
    fn default_terminal_profile_options_come_from_the_published_catalog(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            theme::init(&ThemeId::new("lapis.dark"), None, cx);
            let settings = desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
            SettingsCoordinator::init(settings, cx);
            crate::features::settings_sources::SettingsSources::init(cx);
            // Terminal は Host の profile 一覧を設定の供給元として公開する。
            let catalog = crate::features::terminal::register_settings_source(cx);
            crate::features::terminal::publish_test_profile_catalog(
                &terminal::TerminalProfiles::new(
                    vec![
                        terminal::TerminalProfile::new(
                            "powershell",
                            "PowerShell",
                            "powershell.exe",
                        ),
                        terminal::TerminalProfile::new("cmd", "Command Prompt", "cmd.exe"),
                    ],
                    Some(terminal::TerminalProfileId::new("powershell")),
                ),
                &catalog,
                cx,
            );
        });
        let feature = cx.update(|cx| cx.new(SettingsFeature::new));
        let definition = settings::catalog()
            .definition(settings::TERMINAL_DEFAULT_PROFILE)
            .expect("定義がある");
        feature.update(cx, |feature, cx| {
            let options = feature.options(definition, cx);
            let values = options
                .iter()
                .map(|(value, label)| {
                    (
                        value.as_text().unwrap_or_default().to_owned(),
                        label.clone(),
                    )
                })
                .collect::<Vec<_>>();
            assert_eq!(
                values,
                vec![
                    ("powershell".to_owned(), "PowerShell".to_owned()),
                    ("cmd".to_owned(), "Command Prompt".to_owned()),
                ]
            );
            // 未保存の既定値は Host の既定 profile を選択状態にする。
            assert_eq!(
                feature
                    .effective_value(definition, cx)
                    .and_then(|value| value.as_text().map(ToOwned::to_owned)),
                Some("powershell".to_owned())
            );
        });
    }

    #[derive(Default)]
    struct TestWeb(std::sync::Mutex<Option<String>>);
    impl WebAccessControl for TestWeb {
        fn status(&self, _: &std::path::Path) -> Result<HostWebStatus, String> {
            Ok(HostWebStatus {
                url: self.0.lock().unwrap().clone(),
                addresses: vec!["127.0.0.1".into()],
                error: None,
            })
        }
        fn start(&self, _: &std::path::Path, _: &str, _: u16) -> Result<HostWebStatus, String> {
            *self.0.lock().unwrap() = Some("http://127.0.0.1:1234/".into());
            self.status(std::path::Path::new("."))
        }
        fn stop(&self, _: &std::path::Path) -> Result<HostWebStatus, String> {
            *self.0.lock().unwrap() = None;
            self.status(std::path::Path::new("."))
        }
        fn invite(&self, _: &std::path::Path) -> Result<zeroize::Zeroizing<String>, String> {
            Err("not used".into())
        }
        fn devices(&self, _: &std::path::Path) -> Result<Vec<HostDevice>, String> {
            Ok(Vec::new())
        }
        fn revoke(&self, _: &std::path::Path, _: &str) -> Result<(), String> {
            Err("not used".into())
        }
    }

    #[gpui::test]
    fn web_controls_refresh_after_start_and_stop(cx: &mut gpui::TestAppContext) {
        let feature = cx.update(|cx| {
            theme::init(&ThemeId::new("lapis.dark"), None, cx);
            SettingsCoordinator::init(
                desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap(),
                cx,
            );
            let control: Arc<dyn WebAccessControl> = Arc::new(TestWeb::default());
            let feature = cx.new(|cx| SettingsFeature::with_web_access(cx, None, Some(control)));
            feature.update(cx, |feature, cx| {
                feature.set_workspace(Some(PathBuf::from("workspace")), None, cx);
                feature.select_page("host.web", cx);
            });
            feature
        });
        let wait = |cx: &mut gpui::TestAppContext| {
            let deadline = crate::time::Instant::now() + std::time::Duration::from_secs(5);
            while cx.update(|cx| feature.read(cx).web.as_ref().unwrap().busy) {
                assert!(crate::time::Instant::now() < deadline);
                cx.run_until_parked();
                std::thread::yield_now();
            }
        };
        wait(cx);
        cx.update(|cx| {
            feature.update(cx, |feature, cx| {
                feature.web_request(WebOperation::Start("127.0.0.1".into()), cx)
            })
        });
        wait(cx);
        cx.update(|cx| {
            assert_eq!(
                feature
                    .read(cx)
                    .web
                    .as_ref()
                    .unwrap()
                    .status
                    .as_ref()
                    .unwrap()
                    .url
                    .as_deref(),
                Some("http://127.0.0.1:1234/")
            )
        });
        cx.update(|cx| {
            feature.update(cx, |feature, cx| {
                feature.web_request(WebOperation::Stop, cx)
            })
        });
        wait(cx);
        cx.update(|cx| {
            assert_eq!(
                feature
                    .read(cx)
                    .web
                    .as_ref()
                    .unwrap()
                    .status
                    .as_ref()
                    .unwrap()
                    .url,
                None
            )
        });
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn web_management_page_can_be_opened_from_settings_navigation(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            theme::init(&ThemeId::new("lapis.dark"), None, cx);
            SettingsCoordinator::init(
                desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap(),
                cx,
            );
        });
        let (feature, visual) = {
            let control: Arc<dyn WebAccessControl> = Arc::new(TestWeb::default());
            cx.add_window_view(|_, cx| SettingsFeature::with_web_access(cx, None, Some(control)))
        };
        feature.read_with(visual, |feature, _| {
            assert!(feature.web.is_some(), "Web service missing")
        });
        visual.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear();
        });
        assert!(
            visual.debug_bounds("main-settings-nav").is_some(),
            "settings navigation missing"
        );
        visual.update(|window, cx| {
            feature.read(cx).nav_scroll.handle().scroll_to_bottom();
            window.refresh();
            window.draw(cx).clear();
        });
        let button = visual
            .debug_bounds("settings-host-web")
            .expect("Web navigation");
        visual.simulate_click(button.center(), gpui::Modifiers::none());
        feature.read_with(visual, |feature, _| assert_eq!(feature.page, "host.web"));
        visual.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear();
        });
        assert!(visual.debug_bounds("web-start").is_some());
        assert!(visual.debug_bounds("web-invite").is_some());
    }

    fn web_addresses() -> Vec<desktop_client::WebAddress> {
        vec![
            desktop_client::WebAddress::new(desktop_client::WebAddressKind::Local, "127.0.0.1"),
            desktop_client::WebAddress::new(
                desktop_client::WebAddressKind::Network,
                "192.168.1.20",
            ),
            desktop_client::WebAddress::new(
                desktop_client::WebAddressKind::Tailscale,
                "100.64.1.23",
            ),
        ]
    }

    fn selected_web_address(feature: &SettingsFeature) -> Option<desktop_client::WebAddress> {
        let access = feature.web_access()?.with_loopback_fallback();
        feature.selected_web_address(&access)
    }

    fn web_feature(
        cx: &mut gpui::TestAppContext,
        addresses: Vec<desktop_client::WebAddress>,
        workspace_id: Option<String>,
    ) -> gpui::Entity<SettingsFeature> {
        cx.update(|cx| {
            theme::init(&ThemeId::new("lapis.dark"), None, cx);
            let settings = desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
            SettingsCoordinator::init(settings, cx);
            let source: crate::app::WebAccessSource = Arc::new(move || {
                Some(desktop_client::WebAccess::new(
                    4747,
                    4757,
                    addresses.clone(),
                    workspace_id.clone(),
                ))
            });
            cx.new(|cx| SettingsFeature::with_web_access(cx, Some(source), None))
        })
    }

    /// 更新の項目が定義データだけで描画され、配布元の場所を編集して保存できることを確かめる。
    #[gpui::test]
    fn update_settings_render_from_the_catalog_and_save_the_location(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            theme::init(&ThemeId::new("lapis.dark"), None, cx);
            let settings = desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
            SettingsCoordinator::init(settings, cx);
        });
        let feature = cx.update(|cx| cx.new(SettingsFeature::new));
        feature.update(cx, |feature, cx| {
            // 更新ページには定義済みの 3 項目だけが並ぶ。
            let keys = catalog_for_tests()
                .settings_of("update.channel")
                .map(|definition| definition.key)
                .collect::<Vec<_>>();
            assert_eq!(
                keys,
                vec![
                    settings::UPDATE_MODE,
                    settings::UPDATE_SOURCE,
                    settings::UPDATE_SOURCE_LOCATION
                ]
            );
            // 候補を持つ項目は定義の候補から、持たない項目は入力を出す。
            let location = settings::catalog()
                .definition(settings::UPDATE_SOURCE_LOCATION)
                .expect("定義がある");
            assert!(feature.options(location, cx).is_empty());
            assert_eq!(location.value_kind, settings::ValueKind::Text);
            assert_eq!(
                feature.values.update_mode(),
                settings::UpdateMode::Auto,
                "既定は自動"
            );
        });
    }

    /// 取得元を変えたら以前の location を流用しない。場所の意味は取得元ごとに変わる。
    /// 同じ取得元の再選択では保存済みの場所を残す。
    #[gpui::test]
    fn changing_the_update_source_clears_the_previous_location(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            theme::init(&ThemeId::new("lapis.dark"), None, cx);
            let settings = desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
            SettingsCoordinator::init(settings, cx);
        });
        let feature = cx.update(|cx| cx.new(SettingsFeature::new));

        // まず Local と場所を保存し、保存が反映されるまで待つ。
        feature.update(cx, |feature, cx| {
            feature.choose(settings::UPDATE_SOURCE, SettingValue::text("local"), cx);
        });
        wait_for_settings(cx);
        feature.update(cx, |feature, cx| {
            feature.choose(
                settings::UPDATE_SOURCE_LOCATION,
                SettingValue::text("C:/releases"),
                cx,
            );
        });
        wait_for_settings(cx);
        cx.update(|cx| {
            let values = &feature.read(cx).values;
            assert_eq!(values.update_source(), settings::UpdateSource::Local);
            assert_eq!(
                values.update_source_location().as_deref(),
                Some("C:/releases")
            );
        });

        // 同じ Local を選び直しても場所は消さない。
        feature.update(cx, |feature, cx| {
            feature.choose(settings::UPDATE_SOURCE, SettingValue::text("local"), cx);
        });
        wait_for_settings(cx);
        cx.update(|cx| {
            assert_eq!(
                feature.read(cx).values.update_source_location().as_deref(),
                Some("C:/releases"),
                "取得元が変わっていなければ場所を残す"
            );
        });

        // Server へ変えたら場所を消し、再入力を促す。
        feature.update(cx, |feature, cx| {
            feature.choose(settings::UPDATE_SOURCE, SettingValue::text("server"), cx);
        });
        wait_for_settings(cx);
        cx.update(|cx| {
            let values = &feature.read(cx).values;
            assert_eq!(values.update_source(), settings::UpdateSource::Server);
            assert_eq!(values.update_source_location(), None);
        });
    }

    /// 接続ページは保存値ではなく、いま集めた候補を表示する。
    #[gpui::test]
    fn connection_page_shows_the_current_web_addresses(cx: &mut gpui::TestAppContext) {
        let feature = web_feature(cx, web_addresses(), Some("workspace-1".to_owned()));
        cx.update(|cx| {
            feature.update(cx, |feature, cx| {
                feature.select_page(settings::catalog::CONNECTION_WEB_PAGE, cx);
            });
            let access = feature.read(cx).web_access().expect("接続情報がある");
            assert_eq!(access.addresses().len(), 3);
            let selected = selected_web_address(feature.read(cx)).expect("候補がある");
            let access = feature.read(cx).web_access().expect("接続情報がある");
            assert_eq!(selected.host, "127.0.0.1");
            assert_eq!(
                access.page_url(Some(&selected)).as_deref(),
                Some("http://127.0.0.1:4747/")
            );
            assert_eq!(
                access.bridge_url(Some(&selected)).as_deref(),
                Some("ws://127.0.0.1:4757/remote")
            );
        });
    }

    /// 候補を選ぶと、コピーと「ブラウザーで開く」の対象がその候補へ変わる。
    #[gpui::test]
    fn choosing_a_web_address_changes_the_urls_to_open_and_copy(cx: &mut gpui::TestAppContext) {
        let feature = web_feature(cx, web_addresses(), None);
        cx.update(|cx| {
            feature.update(cx, |feature, cx| {
                feature.select_page(settings::catalog::CONNECTION_WEB_PAGE, cx);
                feature.select_web_host("100.64.1.23".to_owned(), cx);
            });
            let selected = selected_web_address(feature.read(cx)).expect("候補がある");
            let access = feature.read(cx).web_access().expect("接続情報がある");
            assert_eq!(selected.host, "100.64.1.23");
            assert_eq!(
                access.page_url(Some(&selected)).as_deref(),
                Some("http://100.64.1.23:4747/")
            );
            assert_eq!(
                access.bridge_url(Some(&selected)).as_deref(),
                Some("ws://100.64.1.23:4757/remote")
            );
        });
    }

    /// Tailscale の IP が変わっても、表示は供給元の現在値へ追随する。
    #[gpui::test]
    fn web_addresses_follow_the_current_network(cx: &mut gpui::TestAppContext) {
        let current = std::sync::Arc::new(std::sync::Mutex::new("100.64.1.23".to_owned()));
        let observed = current.clone();
        let feature = cx.update(|cx| {
            theme::init(&ThemeId::new("lapis.dark"), None, cx);
            let settings = desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
            SettingsCoordinator::init(settings, cx);
            let source: crate::app::WebAccessSource = Arc::new(move || {
                let host = observed.lock().unwrap().clone();
                Some(desktop_client::WebAccess::new(
                    4747,
                    4757,
                    vec![
                        desktop_client::WebAddress::new(
                            desktop_client::WebAddressKind::Local,
                            "127.0.0.1",
                        ),
                        desktop_client::WebAddress::new(
                            desktop_client::WebAddressKind::Tailscale,
                            host,
                        ),
                    ],
                    None,
                ))
            });
            cx.new(|cx| SettingsFeature::with_web_access(cx, Some(source), None))
        });
        cx.update(|cx| {
            feature.update(cx, |feature, cx| {
                feature.select_page(settings::catalog::CONNECTION_WEB_PAGE, cx);
                feature.select_web_host("100.64.1.23".to_owned(), cx);
            });
            *current.lock().unwrap() = "100.64.9.9".to_owned();
            // 保存値ではないため、選んだホストが消えていれば現在の候補へ戻る。
            let selected = selected_web_address(feature.read(cx)).expect("候補がある");
            assert_eq!(selected.host, "127.0.0.1");
            let access = feature.read(cx).web_access().expect("接続情報がある");
            assert!(
                access
                    .addresses()
                    .iter()
                    .any(|address| address.host == "100.64.9.9")
            );
        });
    }

    /// Desktop は接続中の Workspace ID を初回入力の候補として表示する。
    #[gpui::test]
    fn connection_page_shows_the_connected_workspace_id(cx: &mut gpui::TestAppContext) {
        let feature = web_feature(cx, web_addresses(), None);
        cx.update(|cx| {
            feature.update(cx, |feature, cx| {
                feature.set_workspace(None, Some("workspace-9".to_owned()), cx);
                feature.select_page(settings::catalog::CONNECTION_WEB_PAGE, cx);
            });
            assert_eq!(
                feature
                    .read(cx)
                    .web_access()
                    .and_then(|access| access.workspace_id)
                    .or_else(|| feature.read(cx).workspace_id.clone()),
                Some("workspace-9".to_owned())
            );
        });
    }

    /// 接続情報を出せない構成では、空のページをナビへ出さない。
    #[gpui::test]
    fn connection_page_is_hidden_without_a_web_source(cx: &mut gpui::TestAppContext) {
        let feature = cx.update(|cx| {
            theme::init(&ThemeId::new("lapis.dark"), None, cx);
            let settings = desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
            SettingsCoordinator::init(settings, cx);
            cx.new(SettingsFeature::new)
        });
        let page = settings::catalog()
            .page(settings::catalog::CONNECTION_WEB_PAGE)
            .expect("接続ページの定義がある");
        cx.update(|cx| {
            assert!(!feature.read(cx).page_available(page));
        });
        let available = web_feature(cx, web_addresses(), None);
        cx.update(|cx| {
            assert!(available.read(cx).page_available(page));
        });
    }

    /// 供給元はあっても取得結果が `None` なら、空のページをナビへ出さない。
    #[gpui::test]
    fn connection_page_is_hidden_when_the_source_returns_none(cx: &mut gpui::TestAppContext) {
        let feature = cx.update(|cx| {
            theme::init(&ThemeId::new("lapis.dark"), None, cx);
            let settings = desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
            SettingsCoordinator::init(settings, cx);
            let source: crate::app::WebAccessSource = Arc::new(|| None);
            cx.new(|cx| SettingsFeature::with_web_access(cx, Some(source), None))
        });
        let page = settings::catalog()
            .page(settings::catalog::CONNECTION_WEB_PAGE)
            .expect("接続ページの定義がある");
        cx.update(|cx| {
            assert!(feature.read(cx).web_access().is_none());
            assert!(!feature.read(cx).page_available(page));
        });
    }

    /// 描画した接続ページのボタンが、実際にコピーとブラウザー起動を行う。
    #[gpui::test]
    fn connection_page_buttons_copy_and_open_the_selected_address(cx: &mut gpui::TestAppContext) {
        use gpui::Modifiers;

        let (feature, visual) = cx.add_window_view(|_, cx| {
            theme::init(&ThemeId::new("lapis.dark"), None, cx);
            let settings = desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
            SettingsCoordinator::init(settings, cx);
            let source: crate::app::WebAccessSource = std::sync::Arc::new(|| {
                Some(desktop_client::WebAccess::new(
                    4747,
                    4757,
                    vec![
                        desktop_client::WebAddress::new(
                            desktop_client::WebAddressKind::Local,
                            "127.0.0.1",
                        ),
                        desktop_client::WebAddress::new(
                            desktop_client::WebAddressKind::Tailscale,
                            "100.64.1.23",
                        ),
                    ],
                    None,
                ))
            });
            SettingsFeature::with_web_access(cx, Some(source), None)
        });
        feature.update(visual, |feature, cx| {
            feature.select_page(settings::catalog::CONNECTION_WEB_PAGE, cx)
        });
        visual.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear();
        });

        // Tailscale の候補を選ぶと、コピーと起動の対象がその候補へ変わる。
        let tailscale = visual
            .debug_bounds("web-address-100.64.1.23")
            .expect("候補が出ている");
        visual.simulate_click(tailscale.center(), Modifiers::none());
        visual.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear();
        });
        let copy = visual
            .debug_bounds("web-page-url-copy")
            .expect("ページ URL のコピー操作が出ている");
        visual.simulate_click(copy.center(), Modifiers::none());
        assert_eq!(
            visual.read_from_clipboard().and_then(|item| item.text()),
            Some("http://100.64.1.23:4747/".to_owned())
        );

        let open = visual
            .debug_bounds("web-open-browser")
            .expect("ブラウザーで開く操作が出ている");
        visual.simulate_click(open.center(), Modifiers::none());
        assert_eq!(
            visual.opened_url().as_deref(),
            Some("http://100.64.1.23:4747/")
        );
    }
}
