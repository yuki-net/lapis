use super::tab_state::PanelTab;

/// PanelはTabの表示と選択だけを所有し、Resourceの寿命を扱わない。
#[derive(Clone, Debug)]
pub(crate) struct PanelHost {
    pub tabs: Vec<PanelTab>,
    pub pinned: std::collections::HashSet<PanelTab>,
    pub active: Option<PanelTab>,
    pub open: bool,
}

impl PanelHost {
    pub(super) fn new(tabs: Vec<PanelTab>, open: bool) -> Self {
        let active = tabs.first().cloned();
        Self {
            tabs,
            active,
            open,
            pinned: Default::default(),
        }
    }

    pub(super) fn set_pinned(&mut self, tab: &PanelTab, pinned: bool) -> bool {
        if !self.tabs.contains(tab) {
            return false;
        }
        if pinned {
            self.pinned.insert(tab.clone());
        } else {
            self.pinned.remove(tab);
        }
        self.tabs.sort_by_key(|tab| !self.pinned.contains(tab));
        true
    }

    pub(super) fn reorder(&mut self, tab: &PanelTab, target: &PanelTab, after: bool) -> bool {
        let Some(source) = self.tabs.iter().position(|item| item == tab) else {
            return false;
        };
        let Some(target_index) = self.tabs.iter().position(|item| item == target) else {
            return false;
        };
        if source == target_index {
            return false;
        }
        let item = self.tabs.remove(source);
        let mut index = target_index + usize::from(after);
        if source < index {
            index -= 1;
        }
        self.tabs.insert(index.min(self.tabs.len()), item);
        self.tabs.sort_by_key(|item| !self.pinned.contains(item));
        true
    }

    pub fn activate(&mut self, tab: PanelTab) {
        self.activate_without_open(tab);
        self.open = true;
    }
    pub fn activate_without_open(&mut self, tab: PanelTab) {
        if !self.tabs.contains(&tab) {
            self.tabs.push(tab.clone());
        }
        self.active = Some(tab);
    }
    pub fn set_open_immediate(&mut self, open: bool) {
        self.open = open;
    }
    pub(super) fn requested_open(&self) -> bool {
        self.open
    }
    pub fn remove(&mut self, tab: &PanelTab) -> bool {
        let Some(index) = self.tabs.iter().position(|candidate| candidate == tab) else {
            return false;
        };
        self.tabs.remove(index);
        self.pinned.remove(tab);
        if self.active.as_ref() == Some(tab) {
            self.active = self.tabs.get(index.saturating_sub(1)).cloned();
        }
        true
    }
    pub fn contains(&self, tab: &PanelTab) -> bool {
        self.tabs.contains(tab)
    }
}
