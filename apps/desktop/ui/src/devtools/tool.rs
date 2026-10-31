//! DevTool 内部のTool定義と最小登録境界。
//! 固定5タブを定義し、後続PRで各Toolが独立して実装可能にする。

/// DevToolの固定タブ。順序はUI上の表示順序と一致する。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum DevToolTab {
    #[default]
    Elements,
    Console,
    Network,
    Performance,
    State,
}

impl DevToolTab {
    /// 固定5タブの配列（表示順序）
    pub(crate) fn all() -> &'static [DevToolTab] {
        &[
            DevToolTab::Elements,
            DevToolTab::Console,
            DevToolTab::Network,
            DevToolTab::Performance,
            DevToolTab::State,
        ]
    }

    /// タブの表示名
    pub(crate) fn title(self) -> &'static str {
        match self {
            DevToolTab::Elements => "Elements",
            DevToolTab::Console => "Console",
            DevToolTab::Network => "Network",
            DevToolTab::Performance => "Performance",
            DevToolTab::State => "State",
        }
    }

    /// このToolが現在実装済みかどうか（後続PRで各Toolが追加される）
    #[allow(dead_code)]
    pub(crate) fn is_implemented(self) -> bool {
        matches!(self, DevToolTab::Elements)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_tool_tabs_have_fixed_order_and_count() {
        let tabs = DevToolTab::all();
        assert_eq!(tabs.len(), 5);
        assert_eq!(tabs[0], DevToolTab::Elements);
        assert_eq!(tabs[1], DevToolTab::Console);
        assert_eq!(tabs[2], DevToolTab::Network);
        assert_eq!(tabs[3], DevToolTab::Performance);
        assert_eq!(tabs[4], DevToolTab::State);
    }

    #[test]
    fn dev_tool_default_tab_is_elements() {
        assert_eq!(DevToolTab::default(), DevToolTab::Elements);
    }

    #[test]
    fn only_elements_is_implemented() {
        assert!(DevToolTab::Elements.is_implemented());
        assert!(!DevToolTab::Console.is_implemented());
        assert!(!DevToolTab::Network.is_implemented());
        assert!(!DevToolTab::Performance.is_implemented());
        assert!(!DevToolTab::State.is_implemented());
    }

    #[test]
    fn dev_tool_tab_titles_are_distinct() {
        let titles: Vec<_> = DevToolTab::all().iter().map(|t| t.title()).collect();
        let unique: std::collections::HashSet<_> = titles.iter().collect();
        assert_eq!(unique.len(), 5);
    }
}
