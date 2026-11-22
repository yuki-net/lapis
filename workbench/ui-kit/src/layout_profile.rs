//! Viewport 幅と入力方式から UI プロファイルと操作ポリシーを決める。

/// Wide / Compact のレイアウト。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutProfile {
    Wide,
    Compact,
}

/// 主な入力方式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputKind {
    Mouse,
    Touch,
}

/// コンテキストメニューを開く操作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextMenuTrigger {
    RightClick,
    LongPress,
}

/// レイアウトと入力方式に対応する操作の許可。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InteractionPolicy {
    pub panel_drag: bool,
    pub panel_resize: bool,
    pub tab_detach: bool,
    pub hover_actions: bool,
    pub context_menu: ContextMenuTrigger,
}

/// Wide に切り替える幅。Compact へはヒステリシス分だけ低い幅で切り替える。
pub const WIDE_BREAKPOINT: f32 = 960.0;
/// 境界付近のリサイズでプロファイルが往復しないための幅。
pub const COMPACT_HYSTERESIS: f32 = 32.0;

impl LayoutProfile {
    /// 前回のプロファイルと現在の幅から、境界付近の往復を抑えて決める。
    pub fn resolve(previous: Option<Self>, width: f32) -> Self {
        match previous {
            Some(Self::Wide) if width < WIDE_BREAKPOINT - COMPACT_HYSTERESIS => Self::Compact,
            Some(Self::Compact) if width >= WIDE_BREAKPOINT => Self::Wide,
            Some(profile) => profile,
            None if width >= WIDE_BREAKPOINT => Self::Wide,
            None => Self::Compact,
        }
    }
}

impl InputKind {
    /// pointer が coarse、または hover できなければ Touch とする。
    pub fn resolve(coarse_pointer: bool, hover: bool) -> Self {
        if coarse_pointer || !hover {
            Self::Touch
        } else {
            Self::Mouse
        }
    }
}

impl InteractionPolicy {
    pub fn for_profile(profile: LayoutProfile, input: InputKind) -> Self {
        match (profile, input) {
            (LayoutProfile::Wide, InputKind::Mouse) => Self {
                panel_drag: true,
                panel_resize: true,
                tab_detach: true,
                hover_actions: true,
                context_menu: ContextMenuTrigger::RightClick,
            },
            (LayoutProfile::Wide, InputKind::Touch) => Self {
                panel_drag: false,
                panel_resize: false,
                tab_detach: false,
                hover_actions: false,
                context_menu: ContextMenuTrigger::LongPress,
            },
            (LayoutProfile::Compact, InputKind::Mouse) => Self {
                panel_drag: false,
                panel_resize: false,
                tab_detach: false,
                hover_actions: true,
                context_menu: ContextMenuTrigger::RightClick,
            },
            (LayoutProfile::Compact, InputKind::Touch) => Self {
                panel_drag: false,
                panel_resize: false,
                tab_detach: false,
                hover_actions: false,
                context_menu: ContextMenuTrigger::LongPress,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_and_compact_use_hysteresis() {
        assert_eq!(LayoutProfile::resolve(None, 1200.0), LayoutProfile::Wide);
        assert_eq!(LayoutProfile::resolve(None, 800.0), LayoutProfile::Compact);
        assert_eq!(
            LayoutProfile::resolve(Some(LayoutProfile::Wide), 940.0),
            LayoutProfile::Wide
        );
        assert_eq!(
            LayoutProfile::resolve(Some(LayoutProfile::Wide), 927.0),
            LayoutProfile::Compact
        );
        assert_eq!(
            LayoutProfile::resolve(Some(LayoutProfile::Compact), 959.0),
            LayoutProfile::Compact
        );
        assert_eq!(
            LayoutProfile::resolve(Some(LayoutProfile::Compact), 960.0),
            LayoutProfile::Wide
        );
    }

    #[test]
    fn coarse_or_non_hover_input_is_touch() {
        assert_eq!(InputKind::resolve(false, true), InputKind::Mouse);
        assert_eq!(InputKind::resolve(true, true), InputKind::Touch);
        assert_eq!(InputKind::resolve(false, false), InputKind::Touch);
    }

    #[test]
    fn compact_and_touch_disable_drag_and_hover_actions() {
        let wide_mouse = InteractionPolicy::for_profile(LayoutProfile::Wide, InputKind::Mouse);
        assert!(wide_mouse.panel_drag);
        assert!(wide_mouse.panel_resize);
        assert!(wide_mouse.tab_detach);

        let compact_mouse =
            InteractionPolicy::for_profile(LayoutProfile::Compact, InputKind::Mouse);
        assert!(!compact_mouse.panel_drag);
        assert!(!compact_mouse.panel_resize);
        assert!(compact_mouse.hover_actions);
        assert_eq!(compact_mouse.context_menu, ContextMenuTrigger::RightClick);

        let touch = InteractionPolicy::for_profile(LayoutProfile::Wide, InputKind::Touch);
        assert!(!touch.panel_drag);
        assert!(!touch.hover_actions);
        assert_eq!(touch.context_menu, ContextMenuTrigger::LongPress);
    }
}
