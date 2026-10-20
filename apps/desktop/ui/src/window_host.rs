//! OS Window と Feature の所有を分離する。Detached は既存 worker を共有する。
use std::collections::HashMap;

mod close;
pub(crate) mod drag;
mod native_smoke;
pub(crate) use native_smoke::run_native_smoke;

use gpui::{
    AppContext, Context, Entity, Focusable, Render, Subscription, WeakEntity, Window, prelude::*,
};
use lapis_editor_core::DocumentId;

use crate::{
    client_adapter::ClientSnapshot,
    commands::UiCommand,
    extension_ui::PanelPosition,
    features::editor::{Editor, EditorEvent},
    root::DesktopRoot,
    shell::{
        PanelTab,
        main::{Shell, ShellPresentation},
        window_group::{GroupLayout, HostId, ROOT_HOST},
    },
};

pub(crate) struct HostMember {
    pub handle: gpui::AnyWindowHandle,
    pub editor: Entity<Editor>,
    pub shell: Entity<Shell>,
    pub bounds: gpui::WindowBounds,
}

pub(crate) struct WindowGroup {
    pub layout: GroupLayout,
    pub members: HashMap<HostId, HostMember>,
    pub latest: Option<ClientSnapshot>,
    pub closing: bool,
    pub locale: lapis_localization::LocaleId,
    pub status: String,
}

impl WindowGroup {
    pub(crate) fn new(
        window: &Window,
        editor: Entity<Editor>,
        shell: Entity<Shell>,
        locale: lapis_localization::LocaleId,
    ) -> Self {
        Self {
            layout: GroupLayout::new(),
            members: HashMap::from([(
                ROOT_HOST,
                HostMember {
                    handle: window.window_handle(),
                    editor,
                    shell,
                    bounds: window.window_bounds(),
                },
            )]),
            latest: None,
            closing: false,
            locale,
            status: String::new(),
        }
    }

    pub(crate) fn documents(&self, host: HostId) -> Vec<DocumentId> {
        self.layout
            .tabs
            .iter()
            .filter_map(|(tab, (owner, _))| match tab {
                PanelTab::Document(id) if *owner == host => Some(id.clone()),
                _ => None,
            })
            .collect()
    }

    pub(crate) fn publish(&mut self, snapshot: ClientSnapshot, cx: &mut Context<Self>) {
        if self
            .latest
            .as_ref()
            .is_some_and(|old| old.sequence >= snapshot.sequence)
        {
            return;
        }
        self.layout.tabs.retain(|tab, _| match tab {
            PanelTab::Document(id) => snapshot.tabs.iter().any(|tab| &tab.id == id),
            PanelTab::Tool(_) => true,
        });
        for tab in &snapshot.tabs {
            let owner = snapshot
                .document_owners
                .get(&tab.id)
                .copied()
                .unwrap_or(ROOT_HOST);
            self.layout
                .register_tab(PanelTab::Document(tab.id.clone()), owner);
        }
        let changed = self.latest.as_ref().is_none_or(|old| {
            old.root != snapshot.root
                || old.conversation != snapshot.conversation
                || old.documents.len() != snapshot.documents.len()
                || old.documents.iter().zip(&snapshot.documents).any(|(a, b)| {
                    a.id != b.id
                        || a.document.revision() != b.document.revision()
                        || a.document.is_dirty() != b.document.is_dirty()
                        || a.document.path() != b.document.path()
                })
        });
        self.latest = Some(snapshot);
        if changed {
            cx.notify();
        }
    }

    pub(crate) fn configure_editor(
        &self,
        host: HostId,
        editor: &Entity<Editor>,
        cx: &mut gpui::App,
    ) {
        let own = self.documents(host);
        let excluded = self
            .layout
            .tabs
            .iter()
            .filter_map(|(tab, (owner, _))| match tab {
                PanelTab::Document(id) if *owner != host => Some(id.clone()),
                _ => None,
            })
            .collect();
        editor.update(cx, |editor, cx| {
            editor.host = host;
            editor.excluded_documents = excluded;
            editor.only_documents = (host != ROOT_HOST).then_some(own);
            editor.set_enabled(!self.closing, cx);
            if let Some(snapshot) = &self.latest {
                editor.receive_snapshot(snapshot, cx);
            }
        });
    }
}

/// 各 Window が Shell・Editor の focus/scroll/active tab と購読だけを所有する。
struct WindowHost {
    root: WeakEntity<DesktopRoot>,
    group: Entity<WindowGroup>,
    host: HostId,
    shell: Entity<Shell>,
    editor: Entity<Editor>,
    _subscriptions: Vec<Subscription>,
}

impl WindowHost {
    fn new(
        group: Entity<WindowGroup>,
        host: HostId,
        root: WeakEntity<DesktopRoot>,
        worker: crate::client_adapter::ClientWorker<crate::client_adapter::DocumentClient>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let editor = cx.new(|cx| Editor::new_with_polling(worker, false, cx));
        let shell = cx.new(|cx| Shell::new(editor.clone().into(), HashMap::new(), cx));
        shell.update(cx, |shell, _| {
            shell.host = host;
            shell.group = Some(group.entity_id());
            shell.detached = true;
        });
        group.update(cx, |group, _| {
            group.members.insert(
                host,
                HostMember {
                    handle: window.window_handle(),
                    editor: editor.clone(),
                    shell: shell.clone(),
                    bounds: window.window_bounds(),
                },
            );
        });
        let command_root = root.clone();
        let editor_group = group.clone();
        let mut subscriptions = vec![
            cx.observe_window_activation(window, |this, window, cx| {
                if !window.is_window_active() {
                    this.shell
                        .update(cx, |shell, cx| shell.cancel_tab_drag(window, cx));
                }
            }),
            cx.observe_window_bounds(window, |this, window, cx| {
                this.group.update(cx, |group, _| {
                    if let Some(member) = group.members.get_mut(&this.host) {
                        member.bounds = window.window_bounds();
                    }
                });
            }),
            cx.subscribe_in(&shell, window, move |_, _, command, window, cx| {
                let _ = command_root.update(cx, |root, cx| {
                    root.host_command(host, command.clone(), window, cx)
                });
            }),
            cx.subscribe(&editor, move |this, _, event, cx| match event {
                EditorEvent::Snapshot(snapshot) => {
                    editor_group.update(cx, |group, cx| group.publish(*snapshot.clone(), cx))
                }
                EditorEvent::Status(message) => this
                    .shell
                    .update(cx, |shell, cx| shell.set_status(message.clone(), cx)),
                _ => {}
            }),
            cx.observe(&group, |this, _, cx| this.synchronize(cx)),
            cx.observe(&editor, |this, _, cx| this.update_presentation(cx)),
        ];
        // native close と独自 Window Controls を同じ終了経路へ集約する。
        let close_root = root.clone();
        window.on_window_should_close(cx, move |window, cx| {
            let _ = close_root.update(cx, |root, cx| {
                root.host_command(host, UiCommand::CloseWindow, window, cx)
            });
            false
        });
        let release_group = group.clone();
        subscriptions.push(cx.on_release(move |_, cx| {
            release_group.update(cx, |group, cx| {
                group.members.remove(&host);
                group.layout.remove_window(host);
                cx.notify();
            });
        }));
        window.focus(&editor.focus_handle(cx));
        let mut this = Self {
            root,
            group,
            host,
            shell,
            editor,
            _subscriptions: subscriptions,
        };
        this.synchronize(cx);
        this
    }

    fn synchronize(&mut self, cx: &mut Context<Self>) {
        self.group.update(cx, |group, cx| {
            group.configure_editor(self.host, &self.editor, cx)
        });
        let locale = self.group.read(cx).locale.clone();
        let status = self.group.read(cx).status.clone();
        self.shell.update(cx, |shell, cx| {
            shell.set_locale(locale, cx);
            if !status.is_empty() {
                shell.set_status(status, cx);
            }
        });
        self.update_presentation(cx);
    }

    fn update_presentation(&mut self, cx: &mut Context<Self>) {
        let Some(snapshot) = self.group.read(cx).latest.as_ref() else {
            return;
        };
        let editor = self.editor.read(cx);
        let mut snapshot = snapshot.clone();
        snapshot.active = editor.document_id();
        let snapshot = editor.local_snapshot(&snapshot);
        let presentation = ShellPresentation {
            documents: snapshot.tabs,
            active: editor.document_id(),
            root: snapshot.root,
            workspace_name: snapshot.workspace_name,
            cursor: editor.cursor_line_column(),
            line_ending: if editor.document().contains_crlf() {
                "CRLF"
            } else {
                "LF"
            },
            encoding: editor.document().encoding(),
            ..Default::default()
        };
        self.shell
            .update(cx, |shell, cx| shell.update_presentation(presentation, cx));
        apply_panel_layout(&self.group, self.host, &self.shell, cx);
    }
}

impl Render for WindowHost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        gpui::div()
            .size_full()
            .on_action(cx.listener(|this, _: &crate::app::Quit, window, cx| {
                cx.stop_propagation();
                let _ = this
                    .root
                    .update(cx, |root, cx| root.close_application(window, cx));
            }))
            .child(self.shell.clone())
    }
}

impl DesktopRoot {
    pub(crate) fn synchronize_group(&mut self, cx: &mut Context<Self>) {
        self.group.update(cx, |group, cx| {
            group.configure_editor(ROOT_HOST, &self.editor, cx)
        });
        if let Some(snapshot) = self.group.read(cx).latest.clone() {
            let mut snapshot = snapshot;
            snapshot.active = self.editor.read(cx).document_id();
            let local = self.editor.read(cx).local_snapshot(&snapshot);
            self.apply_snapshot(&local, cx);
            apply_panel_layout(&self.group, ROOT_HOST, &self.shell, cx);
            self.schedule_persist(cx);
        }
    }

    pub(crate) fn record_window_layout(&mut self, cx: &mut Context<Self>) {
        let placements = self
            .shell
            .read(cx)
            .shell
            .panels()
            .into_iter()
            .flat_map(|panel| {
                panel
                    .tabs
                    .iter()
                    .cloned()
                    .map(move |tab| (tab, panel.position))
            })
            .collect::<Vec<_>>();
        self.group.update(cx, |group, _| {
            group.layout.tabs.retain(|tab, (owner, _)| {
                *owner != ROOT_HOST
                    || !matches!(tab, PanelTab::Tool(_))
                    || placements.iter().any(|(present, _)| present == tab)
            });
            for (tab, panel) in placements {
                let (owner, position) = group.layout.tabs.entry(tab).or_insert((ROOT_HOST, panel));
                if *owner == ROOT_HOST {
                    *position = panel;
                }
            }
        });
    }

    pub(crate) fn host_command(
        &mut self,
        host: HostId,
        command: UiCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.group.read(cx).closing {
            return;
        }
        match command {
            UiCommand::ReleaseTab(drag, point) => self.release_tab(host, drag, point, window, cx),
            UiCommand::DetachTab(tab) => self.detach_tab(host, tab, cx),
            UiCommand::DockTab(tab) => {
                self.transfer_tab(host, ROOT_HOST, PanelPosition::Main, tab, cx)
            }
            UiCommand::MoveTab(drag, panel) => {
                if drag.group == Some(self.group.entity_id()) {
                    self.transfer_tab(drag.source_host, host, panel, drag.tab, cx);
                }
            }
            UiCommand::CloseWindow => self.close_hosts(host, None, window, cx),
            UiCommand::CloseDocument(id) => self.close_hosts(host, Some(id), window, cx),
            UiCommand::ActivateDocument(id) => {
                if self.group.read(cx).documents(host).contains(&id) {
                    let editor = self.group.read(cx).members[&host].editor.clone();
                    editor.update(cx, |editor, cx| editor.activate(id, cx));
                    window.focus(&editor.focus_handle(cx));
                }
            }
            UiCommand::Save => {
                let editor = self.group.read(cx).members[&host].editor.clone();
                editor.update(cx, |editor, cx| {
                    editor.submit(
                        false,
                        |client| {
                            client
                                .editor
                                .save_document()
                                .map(|_| ())
                                .map_err(|e| e.to_string())
                        },
                        cx,
                    );
                });
            }
            _ => {}
        }
    }

    fn transfer_tab(
        &mut self,
        source: HostId,
        target: HostId,
        panel: PanelPosition,
        tab: PanelTab,
        cx: &mut Context<Self>,
    ) {
        if !self.can_move_tab(source, &tab, cx)
            || !self.group.read(cx).members.contains_key(&target)
        {
            return;
        }
        let view = match &tab {
            PanelTab::Document(id) => self
                .group
                .read(cx)
                .members
                .get(&source)
                .and_then(|member| member.editor.read(cx).tab_view(id)),
            _ => None,
        };
        if let Some(view) = view
            && let Some(member) = self.group.read(cx).members.get(&target)
        {
            let editor = member.editor.clone();
            editor.update(cx, |editor, _| editor.seed_view(view));
        }
        self.group.update(cx, |group, cx| {
            if group.layout.move_tab(source, target, panel, &tab) {
                group.members[&target].shell.update(cx, |shell, cx| {
                    shell.shell.focused_panel = panel;
                    cx.notify();
                });
                let editor = group.members[&target].editor.clone();
                group.configure_editor(target, &editor, cx);
                if let (Some(snapshot), PanelTab::Document(id)) = (&group.latest, &tab) {
                    editor.update(cx, |editor, cx| {
                        editor.select_transferred(snapshot, id.clone(), cx)
                    });
                }
                cx.notify();
            }
        });
        // open_window 中に登録した購読は effect の最後に有効になる。
        // 初回 snapshot と同じ通知に併合されても新しい Host に配置を渡す。
        let group = self.group.clone();
        cx.defer(move |cx| group.update(cx, |_, cx| cx.notify()));
    }

    fn detach_tab(&mut self, host: HostId, tab: PanelTab, cx: &mut Context<Self>) {
        self.detach_tab_at(host, tab, None, cx);
    }

    fn detach_tab_at(
        &mut self,
        host: HostId,
        tab: PanelTab,
        point: Option<drag::ScreenPoint>,
        cx: &mut Context<Self>,
    ) {
        if !self.can_move_tab(host, &tab, cx) {
            return;
        }
        let id = self.group.update(cx, |group, _| group.layout.add_window());
        let group = self.group.clone();
        let root = cx.entity().downgrade();
        let worker = self.worker.clone();
        let options = crate::app::window_options(cx);
        let result = cx.open_window(options, move |window, cx| {
            cx.new(|cx| WindowHost::new(group, id, root, worker, window, cx))
        });
        match result {
            Ok(handle) => {
                if let Some(point) = point {
                    let _ = handle.update(cx, |_, window, _| drag::position_window(window, point));
                }
                self.transfer_tab(host, id, PanelPosition::Main, tab, cx);
            }
            Err(error) => {
                self.group
                    .update(cx, |group, _| group.layout.remove_window(id));
                self.set_status(format!("Windowを開けませんでした: {error}"), cx);
            }
        }
    }
}

async fn wait_client<T>(
    receiver: std::sync::mpsc::Receiver<Result<T, String>>,
) -> Result<T, String> {
    loop {
        match receiver.try_recv() {
            Ok(result) => return result,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                return Err("Client workerが終了しました".into());
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                gpui::Timer::after(std::time::Duration::from_millis(16)).await;
            }
        }
    }
}

fn apply_panel_layout(
    group: &Entity<WindowGroup>,
    host: HostId,
    shell: &Entity<Shell>,
    cx: &mut gpui::App,
) {
    let placements = group
        .read(cx)
        .layout
        .tabs
        .iter()
        .filter(|(_, (owner, _))| *owner == host)
        .map(|(tab, (_, panel))| (tab.clone(), *panel))
        .collect::<Vec<_>>();
    shell.update(cx, |shell, cx| {
        let mut changed = false;
        for (tab, target) in placements {
            let source = shell
                .shell
                .panels()
                .into_iter()
                .find(|panel| panel.contains(&tab))
                .map(|panel| panel.position);
            if let Some(source) = source
                && source != target
            {
                shell.shell.move_tab(source, target, tab);
                changed = true;
            }
        }
        if changed {
            cx.notify();
        }
    });
}
