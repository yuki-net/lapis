mod view;

pub(crate) use view::{
    DoubleShiftDetector, QuickSearch, QuickSearchEvent, SearchBackspace, SearchConfirm,
    SearchDelete, SearchDismiss, SearchEnd, SearchHome, SearchLeft, SearchNext, SearchPrevious,
    SearchRight, SearchSelectAll,
};
pub(crate) use workbench_shell::command_search::{
    CommandSearchProvider, SearchItem, SearchProvider,
};
