use super::tab_state::PanelTab;
use crate::extension_ui::ZonePosition;

/// PanelはTabの表示と選択だけを所有し、Resourceの寿命を扱わない。
#[derive(Clone, Debug)]
pub(crate) struct PanelHost {
    pub position: ZonePosition,
    pub tabs: Vec<PanelTab>,
    pub active: Option<PanelTab>,
    pub open: bool,
}

impl PanelHost {
    pub(super) fn new(position: ZonePosition, tabs: Vec<PanelTab>, open: bool) -> Self {
        let active = tabs.first().cloned();
        Self {
            position,
            tabs,
            active,
            open,
        }
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
        if self.active.as_ref() == Some(tab) {
            self.active = self.tabs.get(index.saturating_sub(1)).cloned();
        }
        true
    }
    pub fn contains(&self, tab: &PanelTab) -> bool {
        self.tabs.contains(tab)
    }
}
