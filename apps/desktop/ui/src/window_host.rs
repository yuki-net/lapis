//! OS Window と Feature の所有を分離する。Detached は既存 worker を共有する。
use std::collections::HashMap;

pub(crate) mod chrome;
mod close;
pub(crate) mod drag;
pub(crate) mod drag_preview;
mod native_smoke;
pub(crate) use native_smoke::run_native_smoke;

use gpui::{
    AppContext, Context, Entity, Focusable, Render, Subscription, WeakEntity, Window, prelude::*,
};
use lapis_editor_core::DocumentId;

use crate::{
    client_adapter::ClientSnapshot,
    commands::UiCommand,
    extension_ui::ZonePosition,
    features::editor::{Editor, EditorEvent},
    root::DesktopRoot,
    shell::{
        PanelTab,
        main::{Shell, ShellPresentation},
        window_group::{GroupLayout, HostId, ROOT_HOST, TabDestination},
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
    pub synchronization_required: bool,
    pub locale: lapis_localization::LocaleId,
    pub status: String,
}

impl WindowGroup {
    /// Creates a group with the supplied window, editor, and shell as its root member.
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
            synchronization_required: false,
            locale,
            status: String::new(),
        }
    }

    /// Returns document IDs placed in this host, in unspecified order.
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

    /// Accepts a strictly newer snapshot and reconciles document ownership while preserving tab moves.
    /// Notifies observers when workspace, conversation, or document identity, revision, dirty state, or path changes.
    pub(crate) fn publish(&mut self, snapshot: ClientSnapshot, cx: &mut Context<Self>) {
        if self.reconcile(snapshot, false) {
            cx.notify();
        }
    }

    pub(crate) fn publish_reconciled(&mut self, snapshot: ClientSnapshot, cx: &mut Context<Self>) {
        if self.reconcile(snapshot, true) {
            cx.notify();
        }
    }

    fn reconcile(&mut self, snapshot: ClientSnapshot, recovered: bool) -> bool {
        if self
            .latest
            .as_ref()
            .is_some_and(|old| old.sequence >= snapshot.sequence)
        {
            return false;
        }
        self.layout.tabs.retain(|tab, _| match tab {
            PanelTab::Document(id) => snapshot.tabs.iter().any(|tab| &tab.id == id),
            PanelTab::Tool { .. } => true,
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
        let was_required = self.synchronization_required;
        self.synchronization_required =
            snapshot.synchronization_error.is_some() || (was_required && !recovered);
        if let Some(error) = &snapshot.synchronization_error {
            self.status = format!("{error}。再度CloseまたはQuitを実行して状態を確認してください");
        }
        let changed = was_required != self.synchronization_required
            || self.latest.as_ref().is_none_or(|old| {
                old.synchronization_error != snapshot.synchronization_error
                    || old.root != snapshot.root
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
        changed
    }

    /// Applies host ownership filters, closing state, and the latest group snapshot to an editor.
    /// The root accepts unassigned documents; detached editors show only their assigned documents.
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
            editor.set_enabled(!self.closing && !self.synchronization_required, cx);
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
    /// Registers a detached window with local editor and shell state over the group's existing worker.
    /// Subscribes to group updates and routes commands and close requests through the root without polling.
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
                    editor_group.update(cx, |group, cx| group.publish(*snapshot.clone(), cx));
                    this.synchronize(cx);
                }
                EditorEvent::Idle => this.synchronize(cx),
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

    /// Refreshes this host's document filter, locale, nonempty group status, and shell presentation.
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

    /// Updates the detached shell from the latest snapshot and local editor state, then applies panel placement.
    /// Does nothing until a group snapshot is available.
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
    /// Renders the detached shell and routes Quit through the root's application close flow.
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
    /// Refreshes the root's local view and panel placement from the group and schedules persistence.
    pub(crate) fn synchronize_group(&mut self, cx: &mut Context<Self>) {
        self.group.update(cx, |group, cx| {
            group.configure_editor(ROOT_HOST, &self.editor, cx)
        });
        let status = self.group.read(cx).status.clone();
        if !status.is_empty() {
            self.shell
                .update(cx, |shell, cx| shell.set_status(status, cx));
        }
        if let Some(snapshot) = self.group.read(cx).latest.clone() {
            let mut snapshot = snapshot;
            snapshot.active = self.editor.read(cx).document_id();
            let local = self.editor.read(cx).local_snapshot(&snapshot);
            self.apply_snapshot(&local, cx);
            self.record_window_layout(cx);
            apply_panel_layout(&self.group, ROOT_HOST, &self.shell, cx);
            self.schedule_persist(cx);
        }
    }

    /// Records root panel placements and removes absent root tools while preserving detached ownership.
    pub(crate) fn record_window_layout(&mut self, cx: &mut Context<Self>) {
        let placements = self.shell.read(cx).shell.layout.placements();
        self.group.update(cx, |group, _| {
            group.layout.tabs.retain(|tab, (owner, _)| {
                *owner != ROOT_HOST
                    || !matches!(tab, PanelTab::Tool { .. })
                    || placements.iter().any(|(present, _)| present == tab)
            });
            for (tab, target_group) in placements {
                if let PanelTab::Document(id) = &tab
                    && group
                        .latest
                        .as_ref()
                        .is_some_and(|snapshot| !snapshot.tabs.iter().any(|tab| &tab.id == id))
                {
                    continue;
                }
                let (owner, position) = group
                    .layout
                    .tabs
                    .entry(tab)
                    .or_insert((ROOT_HOST, TabDestination::Panel(target_group.clone())));
                if *owner == ROOT_HOST {
                    *position = TabDestination::Panel(target_group);
                }
            }
        });
    }

    /// Dispatches a host's tab, document, save, or close command through this group's root.
    /// Ignores commands while closing and command variants unsupported by detached hosts.
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
        if self.group.read(cx).synchronization_required
            && !matches!(
                command,
                UiCommand::CloseWindow | UiCommand::CloseDocument(_)
            )
        {
            self.set_status(
                "状態を再確認するため、もう一度CloseまたはQuitを実行してください".into(),
                cx,
            );
            return;
        }
        match command {
            UiCommand::ReleaseTab(drag, point) => self.release_tab(host, drag, point, window, cx),
            UiCommand::DetachTab(tab) => self.detach_tab(host, tab, cx),
            UiCommand::DockTab(tab) => {
                self.transfer_tab(host, ROOT_HOST, ZonePosition::Main, tab, cx)
            }
            UiCommand::MoveTab(drag, group) => {
                if drag.group == Some(self.group.entity_id()) {
                    self.transfer_tab_to_group(drag.source_host, host, group, drag.tab, cx);
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

    /// Moves an eligible document tab to an existing group member, seeding its selection and scroll state.
    /// Selects the destination document and panel; invalid ownership, pending input, or closing blocks the move.
    fn transfer_tab(
        &mut self,
        source: HostId,
        target: HostId,
        panel: ZonePosition,
        tab: PanelTab,
        cx: &mut Context<Self>,
    ) {
        self.transfer_tab_to_destination(source, target, TabDestination::Zone(panel), tab, cx);
    }

    fn transfer_tab_to_group(
        &mut self,
        source: HostId,
        target: HostId,
        target_group: crate::shell::PanelId,
        tab: PanelTab,
        cx: &mut Context<Self>,
    ) {
        self.transfer_tab_to_destination(
            source,
            target,
            TabDestination::Panel(target_group),
            tab,
            cx,
        );
    }

    fn transfer_tab_to_destination(
        &mut self,
        source: HostId,
        target: HostId,
        destination: TabDestination,
        tab: PanelTab,
        cx: &mut Context<Self>,
    ) {
        if self.group.read(cx).closing
            || !self.can_move_tab(source, &tab, cx)
            || !self.group.read(cx).members.contains_key(&target)
        {
            return;
        }
        if let TabDestination::Panel(target_group) = &destination
            && self.group.read(cx).members[&target]
                .shell
                .read(cx)
                .shell
                .layout
                .panel(target_group)
                .is_none()
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
        let focus_group = match &destination {
            TabDestination::Panel(group) => Some(group.clone()),
            TabDestination::Zone(_) => None,
        };
        self.group.update(cx, |group, cx| {
            if group.layout.move_tab_to(source, target, destination, &tab) {
                group.members[&target].shell.update(cx, |shell, cx| {
                    shell.shell.focused_panel = focus_group;
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

    /// Opens a detached window at the default bounds for an eligible document tab.
    fn detach_tab(&mut self, host: HostId, tab: PanelTab, cx: &mut Context<Self>) {
        self.detach_tab_at(host, tab, None, cx);
    }

    /// Opens a detached window before transferring an eligible document tab into it.
    /// An optional physical screen point positions it on Windows. Opening failures
    /// retain the source placement and become status messages.
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
                self.transfer_tab(host, id, ZonePosition::Main, tab, cx);
            }
            Err(error) => {
                self.group
                    .update(cx, |group, _| group.layout.remove_window(id));
                self.set_status(format!("Windowを開けませんでした: {error}"), cx);
            }
        }
    }
}

/// Waits asynchronously for one worker response, returning its result or an error if the channel disconnects.
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

/// Moves tabs already present in the shell to their recorded panels for this host.
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
        .map(|(tab, (_, target_group))| (tab.clone(), target_group.clone()))
        .collect::<Vec<_>>();
    shell.update(cx, |shell, cx| {
        let mut changed = false;
        for (tab, target) in placements {
            let source = shell.shell.layout.panel_for_tab(&tab);
            if source.as_ref().is_some_and(|source| match &target {
                TabDestination::Panel(target) => source == target,
                TabDestination::Zone(position) => shell
                    .shell
                    .layout
                    .panel(source)
                    .is_some_and(|panel| panel.position == *position),
            }) {
                continue;
            }
            let success = match target {
                TabDestination::Panel(target) if shell.shell.layout.panel(&target).is_some() => {
                    shell.shell.open_panel_tab(&target, tab.clone())
                }
                TabDestination::Zone(position) => shell
                    .shell
                    .layout
                    .move_tab_to_zone(position, tab.clone(), std::time::Instant::now())
                    .is_ok(),
                TabDestination::Panel(_) => shell
                    .shell
                    .layout
                    .move_tab_to_zone(ZonePosition::Main, tab.clone(), std::time::Instant::now())
                    .is_ok(),
            };
            if success {
                if let Some(source) = source
                    && shell
                        .shell
                        .layout
                        .panel(&source)
                        .is_some_and(|panel| panel.tabs.is_empty())
                {
                    shell.schedule_empty_close(source, cx);
                }
                changed = true;
            }
        }
        if changed {
            cx.notify();
        }
    });
}

#[cfg(test)]
mod close_tests {
    use super::*;
    use crate::client_adapter::tests::{Fixture, plan};

    fn group() -> WindowGroup {
        WindowGroup {
            layout: GroupLayout::new(),
            members: HashMap::new(),
            latest: None,
            closing: false,
            synchronization_required: false,
            locale: Default::default(),
            status: String::new(),
        }
    }

    #[test]
    fn partial_close_reconciles_root_and_detached_without_reopening_stale_tabs() {
        let fixture = Fixture::default();
        let mut client = fixture.client();
        let decisions = plan(&client);
        let mut group = group();
        let first = group.layout.add_window();
        let second = group.layout.add_window();
        client.document_owners.insert(decisions[1].0.clone(), first);
        client
            .document_owners
            .insert(decisions[2].0.clone(), second);
        let old = client.snapshot().unwrap();
        group.reconcile(old.clone(), true);
        fixture.0.lock().unwrap().fail_close = Some(decisions[2].0.clone());
        let report = client.close_documents(&decisions, Fixture::view());
        assert_eq!(report.close.unwrap_err().progress.closed.len(), 2);
        group.reconcile(report.snapshot, true);
        assert!(group.documents(ROOT_HOST).is_empty());
        assert!(group.documents(first).is_empty());
        assert_eq!(group.documents(second), vec![decisions[2].0.clone()]);
        assert_eq!(group.layout.windows.len(), 3);
        assert_eq!(group.latest.as_ref().unwrap().document_owners.len(), 1);
        assert!(!group.reconcile(old, true));
        assert_eq!(group.layout.tabs.len(), 1);
        fixture.0.lock().unwrap().fail_close = None;
        let report = client.close_documents(&plan(&client), Fixture::view());
        assert!(report.close.is_ok());
        group.reconcile(report.snapshot, true);
        assert!(group.layout.tabs.is_empty());
        assert_eq!(fixture.0.lock().unwrap().closed.len(), 3);
    }

    #[test]
    fn group_a_success_and_b_failure_keep_each_actual_membership_and_recover() {
        let a = Fixture::default();
        let b = Fixture::default();
        let mut ca = a.client();
        let mut cb = b.client();
        let mut ga = group();
        let mut gb = group();
        ga.reconcile(ca.snapshot().unwrap(), true);
        gb.reconcile(cb.snapshot().unwrap(), true);
        let da = plan(&ca);
        let db = plan(&cb);
        b.0.lock().unwrap().fail_close = Some(db[1].0.clone());
        let ra = ca.close_documents(&da, Fixture::view());
        let rb = cb.close_documents(&db, Fixture::view());
        assert!(ra.close.is_ok());
        assert_eq!(rb.close.unwrap_err().progress.closed.len(), 1);
        ga.reconcile(ra.snapshot, true);
        gb.reconcile(rb.snapshot, true);
        assert!(ga.layout.tabs.is_empty());
        assert_eq!(gb.layout.tabs.len(), 2);
        assert_eq!(ga.layout.windows.len(), 1);
        assert_eq!(gb.layout.windows.len(), 1);
        b.0.lock().unwrap().fail_attach = true;
        gb.reconcile(cb.reconcile_snapshot(), true);
        assert!(gb.synchronization_required);
        assert_eq!(gb.layout.tabs.len(), 2);
        b.0.lock().unwrap().fail_attach = false;
        gb.reconcile(cb.snapshot().unwrap(), true);
        assert!(!gb.synchronization_required);
        assert!(ga.layout.tabs.is_empty());
    }

    #[test]
    fn delayed_snapshot_cannot_unlock_an_uncertain_worker_result() {
        let fixture = Fixture::default();
        let mut client = fixture.client();
        let mut group = group();
        group.reconcile(client.snapshot().unwrap(), true);
        let delayed = client.snapshot().unwrap();
        group.synchronization_required = true;
        group.reconcile(delayed, false);
        assert!(group.synchronization_required);
        group.reconcile(client.snapshot().unwrap(), true);
        assert!(!group.synchronization_required);
    }
}
