//! GPUI の capture を維持したまま、離した位置を Window / Panel に解決する。
use super::*;
use crate::shell::DraggedPanelTab;
pub(crate) use workbench_shell::ScreenPoint;
use workbench_shell::screen::{client_position, is_target};
pub(crate) use workbench_shell::screen::{is_source_window, position_window, screen_position};

impl DesktopRoot {
    /// Allows moves only for a document owned by a live host when the group is not closing
    /// and no member has pending worker operations or draft recovery.
    pub(super) fn can_move_tab(&self, host: HostId, tab: &PanelTab, cx: &gpui::App) -> bool {
        let group = self.group.read(cx);
        !group.closing
            && matches!(tab, PanelTab::Document(_))
            && group
                .layout
                .tabs
                .get(tab)
                .is_some_and(|(owner, _)| *owner == host)
            && group.members.contains_key(&host)
            && !group
                .members
                .values()
                .any(|member| member.editor.read(cx).has_pending())
    }

    /// Resolves a drag release at a physical screen point.
    /// On Windows, moves eligible tabs into a panel in this group or detaches over
    /// external windows and desktop space. Releases over the source, another group,
    /// or a non-panel Lapis region leave placement unchanged. Other platforms
    /// cannot resolve native targets and detach eligible tabs if called directly.
    pub(super) fn release_tab(
        &mut self,
        host: HostId,
        drag: DraggedPanelTab,
        point: ScreenPoint,
        source_window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if drag.source_host != host
            || !drag.belongs_to(Some(self.group.entity_id()))
            || !self.can_move_tab(host, &drag.tab, cx)
        {
            return;
        }
        // WindowFromPoint を使い、背後の Lapis や別グループへ落とさない。
        // 外部アプリ上は新 Window、別 Lapis Window の非 Panel 領域は取消。
        let mut lapis_target = false;
        let mut target = None;
        for handle in cx.windows() {
            if handle == source_window.window_handle() {
                if is_target(source_window, point) {
                    return;
                }
                continue;
            }
            let hit = handle
                .update(cx, |_, window, _| {
                    is_target(window, point).then(|| client_position(window, point))
                })
                .ok()
                .flatten();
            if let Some(position) = hit {
                lapis_target = true;
                target = position.and_then(|position| {
                    self.group
                        .read(cx)
                        .members
                        .iter()
                        .find_map(|(host, member)| {
                            if member.handle != handle {
                                return None;
                            }
                            let shell = member.shell.read(cx);
                            let panel = shell
                                .panel_bounds
                                .borrow()
                                .iter()
                                .find(|(_, bounds)| bounds.contains(&position))
                                .map(|(panel_id, _)| TabDestination::Panel(panel_id.clone()));
                            let destination = panel.or_else(|| {
                                (!shell.shell.layout.zone_has_panels(ZonePosition::Main)
                                    && shell
                                        .zone_bounds
                                        .borrow()
                                        .get(&ZonePosition::Main)
                                        .is_some_and(|bounds| bounds.contains(&position)))
                                .then_some(TabDestination::Zone(ZonePosition::Main))
                            });
                            destination.map(|destination| (*host, destination))
                        })
                });
                break;
            }
        }
        if let Some((target, destination)) = target {
            let handle = self.group.read(cx).members[&target].handle;
            self.transfer_tab_to_destination(host, target, destination, drag.tab, cx);
            let editor = self.group.read(cx).members[&target].editor.clone();
            let _ = handle.update(cx, |_, window, cx| {
                window.activate_window();
                crate::gpui_compat::focus(window, &editor.focus_handle(cx), cx);
            });
        } else if !lapis_target {
            self.detach_tab_at(host, drag.tab, Some(point), cx);
        }
    }
}
