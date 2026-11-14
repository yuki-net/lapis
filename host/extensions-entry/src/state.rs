use serde::Serialize;

/// 導入済み拡張の状態。目録に載る拡張が「入っているか」「有効か」「非互換か」を持つ。
///
/// 有効な拡張は必ず導入済みである。`installed == false` は、目録が把握しているが
/// 実体を置いていない拡張を表す。`incompatible` は、package を読んだが互換照合に
/// 失敗したため実体を置いていない状態であり、`installed` と同時にはならない。
///
/// 単体では読まず、保存値からの復元は [`ExtensionState::from_parts`] を通す。
/// これにより「有効なのに未導入」「非互換なのに有効」の組み合わせを作らない。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ExtensionState {
    installed: bool,
    enabled: bool,
    incompatible: bool,
}

impl ExtensionState {
    /// 導入済みで、指定した有効状態を持つ。
    pub const fn installed(enabled: bool) -> Self {
        Self {
            installed: true,
            enabled,
            incompatible: false,
        }
    }

    /// 目録だけが把握していて実体の無い状態。
    pub const fn unavailable() -> Self {
        Self {
            installed: false,
            enabled: false,
            incompatible: false,
        }
    }

    /// package を読んだが互換性がなく、実体を置いていない状態。
    pub const fn incompatible() -> Self {
        Self {
            installed: false,
            enabled: false,
            incompatible: true,
        }
    }

    pub const fn is_installed(self) -> bool {
        self.installed
    }

    pub const fn is_enabled(self) -> bool {
        self.enabled
    }

    /// manifest が要求する契約を Host が満たさなかった。
    pub const fn is_incompatible(self) -> bool {
        self.incompatible
    }

    /// 有効なのに未導入という組み合わせを作らない。非互換は未導入と同じ扱いである。
    pub const fn is_consistent(self) -> bool {
        (!self.enabled || self.installed) && !(self.incompatible && self.installed)
    }

    /// 保存値から復元する。整合しない組み合わせは拒否する。
    ///
    /// `incompatible` を欠く保存値（この状態を導入する前の目録）は「非互換でない」と
    /// 同じ意味なので、呼び出し側で `false` を補う。
    pub const fn from_parts(installed: bool, enabled: bool, incompatible: bool) -> Option<Self> {
        let state = Self {
            installed,
            enabled,
            incompatible,
        };
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
            incompatible: self.incompatible,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_always_implies_installed() {
        assert_eq!(
            ExtensionState::from_parts(true, true, false),
            Some(ExtensionState::installed(true))
        );
        assert_eq!(
            ExtensionState::from_parts(false, false, false),
            Some(ExtensionState::unavailable())
        );
        assert_eq!(ExtensionState::from_parts(false, true, false), None);
        assert_eq!(
            ExtensionState::unavailable().with_enabled(true),
            ExtensionState::unavailable()
        );
        assert_eq!(
            ExtensionState::installed(false).with_enabled(true),
            ExtensionState::installed(true)
        );
    }

    #[test]
    fn incompatible_is_a_kind_of_uninstalled() {
        assert_eq!(
            ExtensionState::from_parts(false, false, true),
            Some(ExtensionState::incompatible())
        );
        assert!(ExtensionState::incompatible().is_incompatible());
        assert!(!ExtensionState::incompatible().is_installed());
        assert!(
            !ExtensionState::incompatible()
                .with_enabled(true)
                .is_enabled(),
            "非互換の拡張は有効にできない"
        );
        // 実体がある状態を非互換にはできない。互換照合に失敗した package は展開しない。
        assert_eq!(ExtensionState::from_parts(true, true, true), None);
        assert_eq!(ExtensionState::from_parts(true, false, true), None);
    }
}
