use super::*;
use crate::client_adapter::ClientWorker;
use crate::theme;
use desktop_client::{HostDevice, HostWebStatus, WebAccessControl};
use gpui::{Context, div, px};
use std::sync::Arc;

#[derive(Clone)]
pub(super) enum WebOperation {
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

pub(super) struct WebState {
    pub(super) worker: ClientWorker<Arc<dyn WebAccessControl>>,
    pub(super) status: Option<HostWebStatus>,
    pub(super) devices: Vec<HostDevice>,
    pub(super) address: String,
    pub(super) busy: bool,
    pub(super) generation: u64,
    pub(super) message: String,
}

impl SettingsFeature {
    pub(super) fn web_request(&mut self, operation: WebOperation, cx: &mut Context<Self>) {
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

    pub(super) fn render_web_page(&self, cx: &mut Context<Self>) -> gpui::Div {
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
    pub(super) fn web_access(&self) -> Option<desktop_client::WebAccess> {
        self.web_access.as_ref()?.as_ref()()
    }

    /// 表示中の候補アドレス。選んだホストが消えていれば先頭へ戻す。
    ///
    /// 1回の描画では同じ `access` を使い、候補と URL が食い違わないようにする。
    pub(super) fn selected_web_address(
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

    pub(super) fn select_web_host(&mut self, host: String, cx: &mut Context<Self>) {
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

    /// 現在の接続情報を示すページ。保存値を持たないため、表示のたびに集め直す。
    ///
    /// コピーと「ブラウザーで開く」は同じ候補アドレスを対象にする。
    pub(super) fn render_web_connection_page(&self, cx: &mut Context<Self>) -> gpui::Div {
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
}
