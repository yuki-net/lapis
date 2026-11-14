use serde::Serialize;

/// 導入済み拡張の状態。目録に載る拡張が「入っているか」「有効か」だけを持つ。
///
/// 有効な拡張は必ず導入済みである。`installed == false` は、目録が把握しているが
/// 実体を置いていない拡張（将来の非互換・導入失敗の宣言）を表す。
///
/// 単体では読まず、保存値からの復元は [`ExtensionState::from_parts`] を通す。
/// これにより「有効なのに未導入」の組み合わせを作らない。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ExtensionState {
    installed: bool,
    enabled: bool,
}

impl ExtensionState {
    /// 導入済みで、指定した有効状態を持つ。
    pub const fn installed(enabled: bool) -> Self {
        Self {
            installed: true,
            enabled,
        }
    }

    /// 目録だけが把握していて実体の無い状態。
    pub const fn unavailable() -> Self {
        Self {
            installed: false,
            enabled: false,
        }
    }

    pub const fn is_installed(self) -> bool {
        self.installed
    }

    pub const fn is_enabled(self) -> bool {
        self.enabled
    }

    /// 有効なのに未導入という組み合わせを作らない。
    pub const fn is_consistent(self) -> bool {
        !self.enabled || self.installed
    }

    /// 保存値から復元する。整合しない組み合わせは拒否する。
    pub const fn from_parts(installed: bool, enabled: bool) -> Option<Self> {
        let state = Self { installed, enabled };
        if state.is_consistent() {
            Some(state)
        } else {
            None
        }
    }

    pub const fn with_enabled(self, enabled: bool) -> Self {
        Self {
            installed: self.installed,
            enabled: enabled && self.installed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_always_implies_installed() {
        assert_eq!(
            ExtensionState::from_parts(true, true),
            Some(ExtensionState::installed(true))
        );
        assert_eq!(
            ExtensionState::from_parts(false, false),
            Some(ExtensionState::unavailable())
        );
        assert_eq!(ExtensionState::from_parts(false, true), None);
        assert_eq!(
            ExtensionState::unavailable().with_enabled(true),
            ExtensionState::unavailable()
        );
        assert_eq!(
            ExtensionState::installed(false).with_enabled(true),
            ExtensionState::installed(true)
        );
    }
}
