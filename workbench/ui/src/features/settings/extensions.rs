use super::*;
use crate::client_adapter::ClientWorker;
use crate::theme;
use gpui::{Context, Focusable, Window, div, px};
use settings::{SettingValue, SettingsScope};

impl SettingsFeature {
    #[cfg(not(target_family = "wasm"))]
    pub(super) fn manage_capabilities(
        &mut self,
        id: String,
        action: Option<desktop_client::HostExtensionSetCapabilityRequest>,
        cx: &mut Context<Self>,
    ) {
        if action.is_none() {
            self.capability_info = None;
        }
        let Some(workspace) = self
            .workspace_id
            .as_ref()
            .map(editor_core::WorkspaceId::new)
        else {
            self.message = self.t("settings.extensions.workspace-required");
            cx.notify();
            return;
        };
        let Some(worker) = &self.capability_manager else {
            self.message = self.t("settings.extensions.read-only");
            cx.notify();
            return;
        };
        self.capability_generation = self.capability_generation.wrapping_add(1);
        let generation = self.capability_generation;
        let extension_generation = self.extension_generation;
        let issued_workspace = self.workspace_id.clone();
        let requested_id = id.clone();
        self.capability_busy = true;
        if let Err(error) = worker.submit(
            cx,
            move |manager| manager(workspace, id, action),
            move |this, result, cx| {
                if this.capability_generation != generation
                    || this.extension_generation != extension_generation
                    || this.workspace_id != issued_workspace
                {
                    return;
                }
                this.capability_busy = false;
                if !this.extension_is_current(&requested_id) {
                    this.capability_info = None;
                    cx.notify();
                    return;
                }
                match result {
                    Ok(info) => {
                        this.capability_info = Some(info);
                        this.message.clear();
                    }
                    Err(error) => this.message = error,
                }
                cx.notify();
            },
        ) {
            self.capability_busy = false;
            self.message = error;
        }
        cx.notify();
    }

    #[cfg(target_family = "wasm")]
    pub(super) fn manage_capabilities(
        &mut self,
        id: String,
        action: Option<desktop_client::HostExtensionSetCapabilityRequest>,
        cx: &mut Context<Self>,
    ) {
        if action.is_none() {
            self.capability_info = None;
        }
        if action.is_some() && !desktop_client::web_host::can_manage_extensions() {
            self.message = self.t("settings.extensions.read-only");
            cx.notify();
            return;
        }
        self.capability_generation = self.capability_generation.wrapping_add(1);
        let generation = self.capability_generation;
        let extension_generation = self.extension_generation;
        let requested_id = id.clone();
        self.capability_busy = true;
        cx.spawn(async move |this, cx| {
            let result = async {
                if let Some(action) = action {
                    desktop_client::web_host::set_extension_capability(action).await?;
                }
                desktop_client::web_host::extension_capabilities(id).await
            }
            .await;
            let _ = this.update(cx, |this, cx| {
                if this.capability_generation != generation
                    || this.extension_generation != extension_generation
                {
                    return;
                }
                this.capability_busy = false;
                if !this.extension_is_current(&requested_id) {
                    this.capability_info = None;
                    cx.notify();
                    return;
                }
                match result {
                    Ok(info) => {
                        this.capability_info = Some(info);
                        this.message.clear();
                    }
                    Err(error) => this.message = error,
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn extension_is_current(&self, id: &str) -> bool {
        self.extension
            .as_ref()
            .and_then(|session| session.snapshot())
            .is_some_and(|snapshot| {
                snapshot
                    .extensions()
                    .iter()
                    .any(|entry| entry.id == id && entry.installed)
            })
    }

    #[cfg(not(target_family = "wasm"))]
    pub(super) fn set_extension_enabled(
        &mut self,
        id: String,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        if self.extension_management_busy {
            return;
        }
        let Some(workspace) = self
            .workspace_id
            .as_ref()
            .map(editor_core::WorkspaceId::new)
        else {
            self.message = self.t("settings.extensions.workspace-required");
            cx.notify();
            return;
        };
        let Some(worker) = &self.extension_manager else {
            self.message = self.t("settings.extensions.read-only");
            cx.notify();
            return;
        };
        self.extension_management_busy = true;
        let generation = self.extension_generation;
        let issued_workspace = self.workspace_id.clone();
        if let Err(error) = worker.submit(
            cx,
            move |manager| manager(workspace, id, enabled),
            move |this, result, cx| {
                if this.extension_generation != generation || this.workspace_id != issued_workspace
                {
                    this.extension_management_busy = false;
                    cx.notify();
                    return;
                }
                this.extension_management_busy = false;
                this.message = result
                    .err()
                    .unwrap_or_else(|| this.t("settings.extensions.saved"));
                cx.notify();
            },
        ) {
            self.extension_management_busy = false;
            self.message = error;
        }
        cx.notify();
    }

    #[cfg(target_family = "wasm")]
    pub(super) fn set_extension_enabled(
        &mut self,
        id: String,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        if self.extension_management_busy {
            return;
        }
        if !desktop_client::web_host::can_manage_extensions() {
            self.message = self.t("settings.extensions.read-only");
            cx.notify();
            return;
        }
        self.extension_management_busy = true;
        let generation = self.extension_generation;
        cx.spawn(async move |this, cx| {
            let result = desktop_client::web_host::set_extension_enabled(id, enabled).await;
            let _ = this.update(cx, |this, cx| {
                if this.extension_generation != generation {
                    return;
                }
                this.extension_management_busy = false;
                this.message = result
                    .err()
                    .unwrap_or_else(|| this.t("settings.extensions.saved"));
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// 接続単位の宣言を設定画面に反映する。古い接続の通知は無視する。
    pub(crate) fn set_extension_contributions(
        &mut self,
        session: Option<desktop_client::ExtensionContributionSession>,
        cx: &mut Context<Self>,
    ) {
        self.extension_generation = self.extension_generation.wrapping_add(1);
        let generation = self.extension_generation;
        self.extension_edit = None;
        self.extension_management_busy = false;
        self.capability_generation = self.capability_generation.wrapping_add(1);
        self.capability_busy = false;
        self.capability_info = None;
        self.extension_worker = session.clone().map(ClientWorker::new);
        self.extension = session;
        self.refresh_extension_locales(cx);
        if let Some(session) = &self.extension {
            let receiver = session.subscribe();
            cx.spawn(async move |this, cx| {
                loop {
                    // Web には blocking receiver を待つ thread がない。
                    crate::timer::Timer::after(std::time::Duration::from_millis(200)).await;
                    let mut changed = false;
                    while receiver.try_recv().is_ok() {
                        changed = true;
                    }
                    let alive = this
                        .update(cx, |this, cx| {
                            if this.extension_generation != generation {
                                return false;
                            }
                            if changed {
                                let snapshot = this
                                    .extension
                                    .as_ref()
                                    .and_then(|session| session.snapshot());
                                if this.capability_info.as_ref().is_some_and(|info| {
                                    !snapshot.as_ref().is_some_and(|snapshot| {
                                        snapshot
                                            .extensions()
                                            .iter()
                                            .any(|entry| entry.id == info.id && entry.installed)
                                    })
                                }) || snapshot.is_none()
                                {
                                    this.capability_generation =
                                        this.capability_generation.wrapping_add(1);
                                    this.capability_busy = false;
                                    this.capability_info = None;
                                }
                                this.refresh_extension_locales(cx);
                            }
                            true
                        })
                        .unwrap_or(false);
                    if !alive {
                        break;
                    }
                }
            })
            .detach();
        }
        cx.notify();
    }

    pub(super) fn refresh_extension_locales(&mut self, cx: &mut Context<Self>) {
        let snapshot = self
            .extension
            .as_ref()
            .and_then(|session| session.snapshot());
        let packs = snapshot
            .as_ref()
            .map(|snapshot| snapshot.core_locales())
            .unwrap_or_default();
        if let Err(error) = self.locale.replace_external(packs) {
            tracing::warn!("拡張の言語を反映できません: {error:?}");
        }
        self.locale.set_active(&self.values.locale());
        let placeholder = self.t("settings.search.placeholder");
        self.search
            .update(cx, |search, cx| search.set_placeholder(placeholder, cx));
        cx.notify();
    }

    pub(super) fn extension_rows(&self) -> Vec<ExtensionRow> {
        let Some(snapshot) = self
            .extension
            .as_ref()
            .and_then(|session| session.snapshot())
        else {
            return Vec::new();
        };
        let locale = self.values.locale();
        snapshot
            .contributions()
            .iter()
            .flat_map(|contribution| {
                contribution
                    .declaration
                    .settings
                    .iter()
                    .map(|setting| {
                        let resolve = |id: &str| {
                            snapshot.resolve_text(
                                &contribution.extension_id,
                                &locale,
                                &localization::MessageId::new(id),
                            )
                        };
                        (
                            format!(
                                "extensions/{}/{}",
                                contribution.extension_id, setting.definition.name
                            ),
                            resolve(&setting.label),
                            setting
                                .description
                                .as_deref()
                                .map(resolve)
                                .unwrap_or_default(),
                            setting.definition.clone(),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    pub(super) fn save_extension(
        &mut self,
        key: String,
        value: Option<SettingValue>,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = &self.extension else {
            return;
        };
        let Some(definition) = self
            .extension_rows()
            .into_iter()
            .find(|row| row.0 == key)
            .map(|row| row.3)
        else {
            return;
        };
        let Some(values) = session.resolved() else {
            return;
        };
        let Some(scope) = values.context().scopes().into_iter().rev().find(|scope| {
            definition.allows_scope(scope) && !matches!(scope, SettingsScope::ProjectShared { .. })
        }) else {
            self.message = self.t("settings.extensions.no-scope");
            cx.notify();
            return;
        };
        let Some(worker) = &self.extension_worker else {
            return;
        };
        let generation = self.extension_generation;
        if let Err(error) = worker.submit(
            cx,
            move |session| {
                session
                    .update(scope, &[(key.as_str(), value)])
                    .map_err(|e| e.to_string())
            },
            move |this, result, cx| {
                if this.extension_generation != generation {
                    return;
                }
                this.message = result.err().unwrap_or_default();
                cx.notify();
            },
        ) {
            self.message = error;
            cx.notify();
        }
    }

    fn edit_extension(
        &mut self,
        key: String,
        kind: settings::ValueKind,
        value: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| crate::components::SearchInput::new("", cx));
        input.update(cx, |input, cx| input.set_text(value.clone(), cx));
        crate::gpui_compat::focus(window, &input.read(cx).focus_handle(cx), cx);
        let subscription = cx.subscribe(&input, |this, _, event, cx| {
            let crate::components::SearchInputEvent::Changed(text) = event;
            if let Some(edit) = &mut this.extension_edit {
                edit.draft = text.clone();
            }
            cx.notify();
        });
        self.extension_edit = Some(ExtensionTextEdit {
            key,
            kind,
            input,
            draft: value,
            _subscription: subscription,
        });
        cx.notify();
    }

    fn commit_extension_edit(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = self.extension_edit.take() else {
            return;
        };
        let value = match edit.kind {
            settings::ValueKind::Text => Some(SettingValue::text(edit.draft)),
            settings::ValueKind::Integer => match edit.draft.parse::<i64>() {
                Ok(value) => Some(SettingValue::integer(value)),
                Err(_) => {
                    self.message = self.t("settings.extensions.invalid-integer");
                    self.extension_edit = Some(edit);
                    cx.notify();
                    return;
                }
            },
            settings::ValueKind::Flag => return,
        };
        self.save_extension(edit.key, value, cx);
    }

    pub(super) fn render_extensions_page(&self, cx: &mut Context<Self>) -> gpui::Div {
        let resolved = self
            .extension
            .as_ref()
            .and_then(|session| session.resolved());
        let rows = self
            .extension_rows()
            .into_iter()
            .filter(|row| self.matches_search(&[row.0.clone(), row.1.clone(), row.2.clone()]))
            .map(|(key, label, description, definition)| {
                let current = resolved
                    .as_ref()
                    .and_then(|values| values.extension_setting(&key))
                    .and_then(|value| value.value())
                    .cloned();
                let mut options = definition.allowed_values.clone();
                if options.is_none() && definition.value_kind == settings::ValueKind::Flag {
                    options = Some(vec![SettingValue::flag(false), SettingValue::flag(true)]);
                }
                let control = if let Some(options) = options {
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_1()
                        .children(options.into_iter().enumerate().map(|(index, value)| {
                            let selected = current.as_ref() == Some(&value);
                            let text = extension_value_text(&value);
                            let key = key.clone();
                            div()
                                .id(gpui::SharedString::from(format!("extension-{key}-{index}")))
                                .px_3()
                                .py_2()
                                .rounded(px(4.0))
                                .bg(if selected {
                                    theme::colors(cx).button_background_selected
                                } else {
                                    theme::colors(cx).background_secondary
                                })
                                .text_size(px(12.0))
                                .text_color(theme::colors(cx).text_primary)
                                .child(text)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.save_extension(key.clone(), Some(value.clone()), cx)
                                }))
                        }))
                        .into_any_element()
                } else {
                    let value = current
                        .as_ref()
                        .map(extension_value_text)
                        .unwrap_or_default();
                    let display = if value.is_empty() {
                        self.t("settings.value.unset")
                    } else {
                        value.clone()
                    };
                    let key = key.clone();
                    let kind = definition.value_kind;
                    div()
                        .id(gpui::SharedString::from(format!("extension-edit-{key}")))
                        .px_3()
                        .py_2()
                        .rounded(px(4.0))
                        .bg(theme::colors(cx).background_secondary)
                        .text_size(px(12.0))
                        .text_color(theme::colors(cx).text_primary)
                        .child(display)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.edit_extension(key.clone(), kind, value.clone(), window, cx)
                        }))
                        .into_any_element()
                };
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(theme::colors(cx).text_primary)
                            .child(label),
                    )
                    .when(!description.is_empty(), |row| {
                        row.child(
                            div()
                                .text_size(px(11.0))
                                .text_color(theme::colors(cx).text_tertiary)
                                .child(description),
                        )
                    })
                    .child(
                        div().flex().items_center().gap_2().child(control).child(
                            div()
                                .id(gpui::SharedString::from(format!("extension-reset-{key}")))
                                .px_3()
                                .py_2()
                                .text_size(px(12.0))
                                .text_color(theme::colors(cx).text_tertiary)
                                .child(self.t("settings.extensions.reset"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.save_extension(key.clone(), None, cx)
                                })),
                        ),
                    )
            })
            .collect::<Vec<_>>();
        let mut page = div()
            .w_full()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .text_size(px(16.0))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(theme::colors(cx).text_primary)
                    .child(self.t("settings.extensions.title")),
            )
            .child(
                div()
                    .p_4()
                    .rounded(px(8.0))
                    .bg(theme::colors(cx).background_tertiary)
                    .flex()
                    .flex_col()
                    .gap_3()
                    .children(
                        self.extension
                            .as_ref()
                            .and_then(|session| session.snapshot())
                            .into_iter()
                            .flat_map(|snapshot| snapshot.extensions().to_vec())
                            .filter(|entry| {
                                self.matches_search(&[
                                    entry.id.clone(),
                                    entry.version.clone(),
                                    self.t("settings.extensions.title"),
                                ])
                            })
                            .map(|entry| {
                                let capability_id = entry.id.clone();
                                let current = match entry.active {
                                    Some(true) => self.t("settings.extensions.active"),
                                    Some(false) => self.t("settings.extensions.inactive"),
                                    None => self.t("settings.extensions.unknown"),
                                };
                                let next = if entry.enabled {
                                    self.t("settings.extensions.next-on")
                                } else {
                                    self.t("settings.extensions.next-off")
                                };
                                let action = self.render_extension_action(
                                    entry.id.clone(),
                                    entry.enabled,
                                    entry.installed && !entry.incompatible,
                                    cx,
                                );
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_size(px(13.0))
                                            .text_color(theme::colors(cx).text_primary)
                                            .child(format!("{}  {}", entry.id, entry.version)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(theme::colors(cx).text_tertiary)
                                            .child(format!("{current} · {next}")),
                                    )
                                    .child(action)
                                    .when(entry.installed && !entry.incompatible, |row| {
                                        row.child(
                                            div()
                                                .id(gpui::SharedString::from(format!(
                                                    "capabilities-{}",
                                                    capability_id
                                                )))
                                                .text_size(px(12.0))
                                                .text_color(theme::colors(cx).text_accent)
                                                .child(self.t("settings.extensions.permissions"))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.manage_capabilities(
                                                        capability_id.clone(),
                                                        None,
                                                        cx,
                                                    )
                                                })),
                                        )
                                    })
                            }),
                    ),
            )
            .child(
                div()
                    .p_4()
                    .rounded(px(8.0))
                    .bg(theme::colors(cx).background_tertiary)
                    .flex()
                    .flex_col()
                    .gap_3()
                    .children(rows),
            );
        if let Some(info) = &self.capability_info {
            let mut panel = div()
                .p_4()
                .rounded(px(8.0))
                .bg(theme::colors(cx).background_tertiary)
                .flex()
                .flex_col()
                .gap_2()
                .child(format!(
                    "{}: {}",
                    self.t("settings.extensions.permissions"),
                    info.id
                ));
            if info.capabilities.is_empty() {
                panel = panel.child(self.t("settings.extensions.permissions-empty"));
            }
            for capability in &info.capabilities {
                let description = if capability.id == "process.pty" {
                    self.t("settings.extensions.pty-description")
                } else {
                    format!("{} {}", capability.id, capability.version)
                };
                let status = if capability.approved {
                    self.t("settings.extensions.permission-granted")
                } else {
                    self.t("settings.extensions.permission-denied")
                };
                let mut row = div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(description)
                    .child(status);
                if capability.supported || capability.approved {
                    let request = desktop_client::HostExtensionSetCapabilityRequest {
                        id: info.id.clone(),
                        manifest_digest: info.manifest_digest.clone(),
                        capability: capability.id.clone(),
                        version: capability.version.clone(),
                        approved: !capability.approved,
                    };
                    let text = if capability.approved {
                        self.t("settings.extensions.permission-revoke")
                    } else {
                        self.t("settings.extensions.permission-approve")
                    };
                    let id = info.id.clone();
                    let button = div()
                        .id(gpui::SharedString::from(format!(
                            "capability-{}-{}",
                            id, capability.id
                        )))
                        .text_size(px(12.0))
                        .text_color(theme::colors(cx).text_accent)
                        .child(text);
                    row = row.child(button.when(!self.capability_busy, |button| {
                        button.on_click(cx.listener(move |this, _, _, cx| {
                            this.manage_capabilities(id.clone(), Some(request.clone()), cx)
                        }))
                    }));
                } else {
                    row = row.child(self.t("settings.extensions.permission-unsupported"));
                }
                panel = panel.child(row);
            }
            page = page.child(panel);
        }
        if let Some(edit) = &self.extension_edit {
            page =
                page.child(
                    div()
                        .p_4()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(edit.input.clone())
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(
                                    div()
                                        .id("extension-save")
                                        .px_3()
                                        .py_2()
                                        .rounded(px(4.0))
                                        .bg(theme::colors(cx).button_background_selected)
                                        .child(self.t("settings.value.save"))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.commit_extension_edit(cx)
                                        })),
                                )
                                .child(
                                    div()
                                        .id("extension-cancel")
                                        .px_3()
                                        .py_2()
                                        .child(self.t("settings.value.cancel"))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.extension_edit = None;
                                            cx.notify();
                                        })),
                                ),
                        )
                        .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                            if event.keystroke.key == "enter" {
                                cx.stop_propagation();
                                this.commit_extension_edit(cx);
                            }
                        })),
                );
        }
        page
    }

    #[cfg(not(target_family = "wasm"))]
    fn render_extension_action(
        &self,
        id: String,
        enabled: bool,
        allowed: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        if self.extension_manager.is_none() || !allowed {
            return div()
                .text_size(px(11.0))
                .text_color(theme::colors(cx).text_tertiary)
                .child(self.t("settings.extensions.read-only"))
                .into_any_element();
        }
        div()
            .id(gpui::SharedString::from(format!("extension-toggle-{id}")))
            .px_3()
            .py_2()
            .rounded(px(4.0))
            .bg(theme::colors(cx).background_secondary)
            .text_size(px(12.0))
            .text_color(theme::colors(cx).text_primary)
            .child(self.t(if enabled {
                "settings.extensions.disable"
            } else {
                "settings.extensions.enable"
            }))
            .when(!self.extension_management_busy, |button| {
                button.on_click(cx.listener(move |this, _, _, cx| {
                    this.set_extension_enabled(id.clone(), !enabled, cx)
                }))
            })
            .into_any_element()
    }

    #[cfg(target_family = "wasm")]
    fn render_extension_action(
        &self,
        _id: String,
        _enabled: bool,
        _allowed: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        if !desktop_client::web_host::can_manage_extensions() || !_allowed {
            return div()
                .text_size(px(11.0))
                .text_color(theme::colors(cx).text_tertiary)
                .child(self.t("settings.extensions.read-only"))
                .into_any_element();
        }
        let id = _id;
        let enabled = _enabled;
        div()
            .id(gpui::SharedString::from(format!("extension-toggle-{id}")))
            .px_3()
            .py_2()
            .rounded(px(4.0))
            .bg(theme::colors(cx).background_secondary)
            .text_size(px(12.0))
            .text_color(theme::colors(cx).text_primary)
            .child(self.t(if enabled {
                "settings.extensions.disable"
            } else {
                "settings.extensions.enable"
            }))
            .when(!self.extension_management_busy, |button| {
                button.on_click(cx.listener(move |this, _, _, cx| {
                    this.set_extension_enabled(id.clone(), !enabled, cx)
                }))
            })
            .into_any_element()
    }
}
