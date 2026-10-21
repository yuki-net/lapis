use std::collections::HashMap;

use super::PanelTab;
use crate::extension_ui::PanelPosition;

pub(crate) type HostId = u64;
pub(crate) const ROOT_HOST: HostId = 0;

/// Window の所属・配置だけを所有する。Feature や実行資源は複製しない。
pub(crate) struct GroupLayout {
    next_host: HostId,
    pub(crate) windows: HashMap<HostId, Option<HostId>>,
    pub(crate) tabs: HashMap<PanelTab, (HostId, PanelPosition)>,
}

impl GroupLayout {
    /// Creates a layout containing only the root window and no tab placements.
    pub(crate) fn new() -> Self {
        Self {
            next_host: 1,
            windows: HashMap::from([(ROOT_HOST, None)]),
            tabs: HashMap::new(),
        }
    }

    /// Registers a new child of the root and returns its group-local host ID.
    pub(crate) fn add_window(&mut self) -> HostId {
        let id = self.next_host;
        self.next_host += 1;
        self.windows.insert(id, Some(ROOT_HOST));
        id
    }

    /// Snapshot の受信元ではなく作成元で初期配置し、確定済みの移動を維持する。
    pub(crate) fn register_tab(&mut self, tab: PanelTab, creator: HostId) {
        let owner = if self.windows.contains_key(&creator) {
            creator
        } else {
            ROOT_HOST
        };
        self.tabs.entry(tab).or_insert((owner, PanelPosition::Main));
    }

    /// Updates placement and returns true when the target exists and the source owns the tab.
    /// Invalid targets or ownership mismatches leave the layout unchanged and return false.
    pub(crate) fn move_tab(
        &mut self,
        source: HostId,
        target: HostId,
        panel: PanelPosition,
        tab: &PanelTab,
    ) -> bool {
        if !self.windows.contains_key(&target)
            || self.tabs.get(tab).is_none_or(|(owner, _)| *owner != source)
        {
            return false;
        }
        self.tabs.insert(tab.clone(), (target, panel));
        true
    }

    /// Removes a window and its tab placements without returning them to the root.
    pub(crate) fn remove_window(&mut self, host: HostId) {
        self.windows.remove(&host);
        self.tabs.retain(|_, (owner, _)| *owner != host);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creator_metadata_does_not_undo_moves_or_orphan_tabs_after_host_release() {
        let mut group = GroupLayout::new();
        let child = group.add_window();
        let tab = PanelTab::tool("future-movable-tool");
        group.register_tab(tab.clone(), child);
        assert_eq!(group.tabs[&tab].0, child);
        assert!(group.move_tab(child, ROOT_HOST, PanelPosition::Bottom, &tab));
        group.register_tab(tab.clone(), child);
        assert_eq!(group.tabs[&tab], (ROOT_HOST, PanelPosition::Bottom));
        group.remove_window(child);
        let late_tab = PanelTab::tool("late-result");
        group.register_tab(late_tab.clone(), child);
        assert_eq!(group.tabs[&late_tab].0, ROOT_HOST);
    }

    #[test]
    fn independent_roots_do_not_accept_foreign_tabs() {
        let mut a = GroupLayout::new();
        let mut b = GroupLayout::new();
        let tab = PanelTab::tool("terminal");
        a.tabs
            .insert(tab.clone(), (ROOT_HOST, PanelPosition::Bottom));
        let child = a.add_window();
        assert!(a.move_tab(ROOT_HOST, child, PanelPosition::Main, &tab));
        assert!(!b.move_tab(child, ROOT_HOST, PanelPosition::Main, &tab));
        assert!(!a.move_tab(ROOT_HOST, child, PanelPosition::Main, &tab));
        let sibling = a.add_window();
        assert!(a.move_tab(child, sibling, PanelPosition::Main, &tab));
        assert!(a.move_tab(sibling, ROOT_HOST, PanelPosition::Bottom, &tab));
    }

    #[test]
    fn closing_child_drops_placement_and_does_not_return_tabs() {
        let mut group = GroupLayout::new();
        let child = group.add_window();
        let tab = PanelTab::tool("task");
        group.tabs.insert(tab.clone(), (child, PanelPosition::Main));
        group.remove_window(child);
        assert!(!group.tabs.contains_key(&tab));
        assert_eq!(group.windows.len(), 1);
        assert!(group.add_window() > child);
    }
}
