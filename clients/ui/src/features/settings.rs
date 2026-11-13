use std::{path::PathBuf, sync::Arc};

use super::settings_coordinator::{Change, SettingsCoordinator};
use crate::client_adapter::ClientWorker;
use crate::{
    components::{Icon, IconName},
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
        let subscription = cx.observe(&coordinator, |this, coordinator, cx| {
            let state = coordinator.read(cx);
            this.values = state.values.clone();
            this.message = state.message.clone();
            let _ = this.locale.set_active(&this.values.locale());
            cx.emit(this.values.clone());
            cx.notify();
        });
        // Terminal Feature が Host の profile 一覧を公開したら選択肢を作り直す。
        let catalogs =
            vec![cx.observe_global::<super::terminal::TerminalProfileCatalog>(|_, cx| cx.notify())];
        let web = web_control.map(|control| WebState {
            worker: ClientWorker::new(control),
            status: None,
            devices: Vec::new(),
            address: "127.0.0.1".into(),
            busy: false,
            generation: 0,
            message: String::new(),
        });
        Self {
            coordinator,
            _subscription: subscription,
            _catalogs: catalogs,
            values,
            root: None,
            locale,
            message: String::new(),
            page: default_page(),
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
            if self.page == "host.web" && self.root.is_some() {
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
        if self.page != page {
            self.page = page;
            self.web_message.clear();
            if page == "host.web" {
                self.web_request(WebOperation::Refresh, cx);
            }
            cx.notify();
        }
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
            .flex_1()
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

        div()
            .w_full()
            .flex()
            .flex_col()
            .py_8()
            .gap_6()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(Icon::new(IconName::Settings))
                    .child(
                        div()
                            .text_size(px(22.0))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(theme::colors(cx).text_primary)
                            .child(self.t("settings.title")),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .max_w(px(920.0))
                    .flex()
                    .gap_6()
                    .child(self.render_nav(cx))
                    .child(self.render_page(cx)),
            )
            .child(self.render_workspace_info(cx, workspace_info))
    }

    /// 分類と中間層は分類専用の行として並べ、ページだけを選べるようにする。
    fn render_nav(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let catalog = settings::catalog();
        div()
            .id("main-settings-nav")
            .debug_selector(|| "main-settings-nav".into())
            .w(px(220.0))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_1()
            .when(self.web.is_some(), |nav| {
                let selected = self.page == "host.web";
                nav.child(
                    div()
                        .id("settings-host-web")
                        .debug_selector(|| "settings-host-web".into())
                        .px_5()
                        .py_2()
                        .rounded(px(6.0))
                        .bg(if selected {
                            theme::colors(cx).button_background_selected
                        } else {
                            theme::colors(cx).background_primary
                        })
                        .hover(|style| style.bg(theme::colors(cx).button_background_hover))
                        .text_size(px(12.0))
                        .text_color(if selected {
                            theme::colors(cx).text_accent
                        } else {
                            theme::colors(cx).text_primary
                        })
                        .child(self.t("settings.web.publish-title"))
                        .focusable()
                        .on_click(cx.listener(|this, _, _, cx| this.select_page("host.web", cx))),
                )
            })
            .children(catalog.categories().iter().map(|category| {
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
                    .children(category.groups.iter().map(|group| {
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
                                    .filter(|page| self.page_available(page))
                                    .map(|page| {
                                        let selected = page.id == self.page;
                                        div()
                                            .id(page.id)
                                            .px_5()
                                            .py_2()
                                            .rounded(px(6.0))
                                            .bg(if selected {
                                                theme::colors(cx).button_background_selected
                                            } else {
                                                theme::colors(cx).background_primary
                                            })
                                            .hover(|style| {
                                                style.bg(theme::colors(cx).button_background_hover)
                                            })
                                            .text_size(px(12.0))
                                            .text_color(if selected {
                                                theme::colors(cx).text_accent
                                            } else {
                                                theme::colors(cx).text_primary
                                            })
                                            .child(self.t(page.label))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.select_page(page.id, cx)
                                            }))
                                    }),
                            )
                    }))
            }))
    }
    fn render_page(&self, cx: &mut Context<Self>) -> gpui::Div {
        if self.page == "host.web" {
            return self.render_web_page(cx);
        }
        let catalog = settings::catalog();
        if self.page == settings::catalog::CONNECTION_WEB_PAGE && self.web_access().is_some() {
            return self.render_web_connection_page(cx);
        }
        let page = catalog.page(self.page).or_else(|| catalog.first_page());
        let mut container = div().flex_1().min_w(px(0.0)).flex().flex_col().gap_4();
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
        for definition in catalog.settings_of(page.id) {
            container = container.child(self.render_field(definition, cx));
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
            .flex_1()
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
        let origin = match self.values.origin(key) {
            Some(scope) => self.t(scope_label(scope)),
            None => self.t("settings.scope.default"),
        };

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
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(theme::colors(cx).text_primary)
                            .child(self.t(definition.label)),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(theme::colors(cx).text_tertiary)
                            .child(origin),
                    ),
            )
            .children(definition.description.map(|description| {
                div()
                    .text_size(px(12.0))
                    .text_color(theme::colors(cx).text_tertiary)
                    .child(self.t(description))
            }))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .children(match &definition.control {
                        SettingControl::Choice(_) => self
                            .options(definition, cx)
                            .into_iter()
                            .enumerate()
                            .map(|(index, (value, label))| {
                                let selected = current.as_ref() == Some(&value);
                                div()
                                    .id(gpui::SharedString::from(format!(
                                        "{}-{index}",
                                        key.as_str()
                                    )))
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
                                    .hover(|style| {
                                        style.bg(theme::colors(cx).button_background_hover)
                                    })
                                    .text_size(px(12.0))
                                    .text_color(theme::colors(cx).text_primary)
                                    .child(format!("{} {label}", if selected { "✓" } else { "" }))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.choose(key, value.clone(), cx)
                                    }))
                                    .into_any_element()
                            })
                            .collect::<Vec<_>>(),
                        // 候補を持たない値は、保存済みの値を表示し、明示的な編集入口から保存する。
                        SettingControl::Text => {
                            let value_text = current
                                .as_ref()
                                .and_then(|value| value.as_text().map(ToOwned::to_owned))
                                .filter(|value| !value.is_empty())
                                .unwrap_or_else(|| self.t("settings.value.unset"));
                            vec![
                                div()
                                    .id(gpui::SharedString::from(format!(
                                        "settings-value-{}",
                                        key.as_str()
                                    )))
                                    .px_3()
                                    .py_2()
                                    .rounded(px(6.0))
                                    .border_1()
                                    .border_color(theme::colors(cx).border_default)
                                    .bg(theme::colors(cx).background_secondary)
                                    .text_size(px(12.0))
                                    .text_color(theme::colors(cx).text_primary)
                                    .child(value_text)
                                    .into_any_element(),
                                div()
                                    .id(gpui::SharedString::from(format!(
                                        "settings-edit-{}",
                                        key.as_str()
                                    )))
                                    .px_3()
                                    .py_2()
                                    .rounded(px(6.0))
                                    .border_1()
                                    .border_color(theme::colors(cx).border_default)
                                    .bg(theme::colors(cx).background_secondary)
                                    .hover(|style| {
                                        style.bg(theme::colors(cx).button_background_hover)
                                    })
                                    .text_size(px(12.0))
                                    .text_color(theme::colors(cx).text_accent)
                                    .child(self.t("settings.value.edit"))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.begin_text_edit(key, window, cx)
                                    }))
                                    .into_any_element(),
                            ]
                        }
                    }),
            )
    }

    /// 未保存の項目は、UI が持つ実行時の既定値を選択状態として見せる。
    fn effective_value(
        &self,
        definition: &SettingDefinition,
        cx: &gpui::App,
    ) -> Option<SettingValue> {
        if let Some(value) = self.values.value(definition.key) {
            return Some(value.clone());
        }
        if definition.key == settings::TERMINAL_FONT_FAMILY {
            return Some(SettingValue::text(super::terminal::default_font_family()));
        }
        if definition.key == settings::TERMINAL_FONT_SIZE {
            return Some(SettingValue::integer(i64::from(
                super::terminal::default_font_size(),
            )));
        }
        if definition.key == settings::TERMINAL_LINE_HEIGHT {
            return Some(SettingValue::integer(i64::from(
                super::terminal::default_line_height_percent(),
            )));
        }
        if definition.key == settings::TERMINAL_CARET_SHAPE {
            let default = settings::TerminalCaretShape::default();
            return Some(SettingValue::text(default.as_id()));
        }
        if definition.key == settings::TERMINAL_DEFAULT_PROFILE {
            // 未保存なら Host の既定 profile を選択状態として見せる。
            return host_terminal_default_profile(cx).map(SettingValue::text);
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
            OptionSource::TerminalFonts => super::terminal::font_choices()
                .iter()
                .map(|family| (SettingValue::text(*family), (*family).to_owned()))
                .collect(),
            OptionSource::TerminalFontSizes => super::terminal::font_size_choices()
                .iter()
                .map(|size| (SettingValue::integer(i64::from(*size)), size.to_string()))
                .collect(),
            OptionSource::TerminalLineHeights => super::terminal::line_height_choices()
                .iter()
                .map(|percent| {
                    (
                        SettingValue::integer(i64::from(*percent)),
                        format!("{percent}%"),
                    )
                })
                .collect(),
            OptionSource::TerminalProfiles => {
                let catalog = super::terminal::profile_catalog(cx);
                catalog
                    .profiles
                    .into_iter()
                    .map(|(id, name)| (SettingValue::text(id), name))
                    .collect()
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

/// Host が既定として公開している Terminal profile ID。未取得なら `None`。
fn host_terminal_default_profile(cx: &gpui::App) -> Option<String> {
    super::terminal::profile_catalog(cx).default
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
        div()
            .w_full()
            .flex()
            .flex_col()
            .child(self.message.clone())
            .child(self.render_settings_content(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension_ui::ThemeId;
    use gpui::AppContext;
    use settings::{SettingsDocument, SettingsError, SettingsRepository, SyntaxTheme};
    use std::sync::Arc;

    struct MemorySettings;
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
            crate::features::terminal::publish_profile_catalog(
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
