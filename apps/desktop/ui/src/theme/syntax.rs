//! 構文スコープの色だけを解決する。原典・ライセンスはassets/syntax-themes/NOTICE.md。
use lapis_settings::SyntaxTheme;
use std::{cell::RefCell, sync::LazyLock};
use syntect::{highlighting::ScopeSelectors, parsing::Scope};

pub(crate) struct Preset {
    foreground: u32,
    colors: [u32; 14],
}

// 順序: comment, string, number, boolean, control, keyword, function, type,
// tag, attribute, variable, constant, operator, markup。スコープの粒度はSyntectに委ねる。
static PRESETS: [Preset; 10] = [
    Preset {
        foreground: 0xe5e7eb,
        colors: [
            0x6a9955, 0xce9178, 0xb5cea8, 0xb5cea8, 0xc586c0, 0xc586c0, 0xdcdcaa, 0x4ec9b0,
            0x569cd6, 0x9cdcfe, 0xe5e7eb, 0x9cdcfe, 0xc586c0, 0x569cd6,
        ],
    },
    Preset {
        foreground: 0x334155,
        colors: [
            0x64748b, 0x287c50, 0xa35a00, 0xa35a00, 0x7c3aed, 0x7c3aed, 0x9a6700, 0x087e8b,
            0x2563eb, 0x8b4d86, 0x334155, 0x8b4d86, 0x7c3aed, 0x2563eb,
        ],
    },
    Preset {
        foreground: 0xd4d4d4,
        colors: [
            0x6a9955, 0xce9178, 0xb5cea8, 0x569cd6, 0xc586c0, 0x569cd6, 0xdcdcaa, 0x4ec9b0,
            0x569cd6, 0x9cdcfe, 0x9cdcfe, 0x4fc1ff, 0xd4d4d4, 0x569cd6,
        ],
    },
    Preset {
        foreground: 0x000000,
        colors: [
            0x008000, 0xa31515, 0x098658, 0x0000ff, 0xaf00db, 0x0000ff, 0x795e26, 0x267f99,
            0x800000, 0xe50000, 0x001080, 0x0070c1, 0x000000, 0x800000,
        ],
    },
    Preset {
        foreground: 0xacb2be,
        colors: [
            0x5d636f, 0xa1c181, 0xbf956a, 0xbf956a, 0xb477cf, 0xb477cf, 0x73ade9, 0x6eb4bf,
            0x74ade8, 0x74ade8, 0xacb2be, 0xdfc184, 0x6eb4bf, 0xd07277,
        ],
    },
    Preset {
        foreground: 0x242529,
        colors: [
            0xa2a3a7, 0x649f57, 0xad6e25, 0xad6e25, 0xa449ab, 0xa449ab, 0x5b79e3, 0x3882b7,
            0x5c78e2, 0x5c78e2, 0x242529, 0xc18401, 0x3882b7, 0xd3604f,
        ],
    },
    Preset {
        foreground: 0xbcbec4,
        colors: [
            0x7a7e85, 0x6aab73, 0x2aacb8, 0xcf8e6d, 0xcf8e6d, 0xcf8e6d, 0x56a8f5, 0xbcbec4,
            0xd5b778, 0xbababa, 0xbcbec4, 0xc77dbb, 0xbcbec4, 0xcf8e6d,
        ],
    },
    Preset {
        foreground: 0x080808,
        colors: [
            0x8c8c8c, 0x067d17, 0x1750eb, 0x0033b3, 0x0033b3, 0x0033b3, 0x00627a, 0x080808,
            0x080808, 0x174ad4, 0x080808, 0x871094, 0x080808, 0x0033b3,
        ],
    },
    Preset {
        foreground: 0xa9b7c6,
        colors: [
            0x808080, 0x6a8759, 0x6897bb, 0xcc7832, 0xcc7832, 0xcc7832, 0xffc66d, 0xa9b7c6,
            0xe8bf6a, 0xbababa, 0xa9b7c6, 0x9876aa, 0xa9b7c6, 0xcc7832,
        ],
    },
    Preset {
        foreground: 0x000000,
        colors: [
            0x808080, 0x008000, 0x0000ff, 0x000080, 0x000080, 0x000080, 0x000000, 0x000000,
            0x000080, 0x0000ff, 0x000000, 0x660e7a, 0x000000, 0x000080,
        ],
    },
];

pub(crate) const FAMILIES: [(SyntaxTheme, &str); 5] = [
    (SyntaxTheme::Lapis, "Lapis"),
    (SyntaxTheme::VsCode, "VS Code"),
    (SyntaxTheme::Zed, "Zed"),
    (SyntaxTheme::Intellij, "IntelliJ (New)"),
    (SyntaxTheme::IntellijClassic, "IntelliJ (Classic / Darcula)"),
];

static SELECTORS: LazyLock<Vec<ScopeSelectors>> = LazyLock::new(|| {
    [
        "comment",
        "string",
        "constant.numeric",
        "constant.language",
        "keyword.control",
        "keyword, storage",
        "entity.name.function, support.function",
        "entity.name.type, support.type, entity.name.class",
        "entity.name.tag",
        "entity.other.attribute-name",
        "variable",
        "constant.other, variable.other.constant",
        "keyword.operator",
        "markup.heading, markup.bold, markup.list",
    ]
    .into_iter()
    .map(|s| s.parse().expect("bundled syntax selector"))
    .collect()
});

thread_local! { static SELECTED: RefCell<Option<SyntaxTheme>> = const { RefCell::new(None) }; }

pub(crate) fn set_active(id: Option<SyntaxTheme>) {
    SELECTED.with(|selected| *selected.borrow_mut() = id);
}

pub(crate) fn active_family() -> SyntaxTheme {
    SELECTED.with(|selected| selected.borrow().unwrap_or_default())
}

pub(crate) fn resolve(family: Option<SyntaxTheme>, ui_theme: &str) -> &'static Preset {
    let pair = match family.unwrap_or_default() {
        SyntaxTheme::Lapis => 0,
        SyntaxTheme::VsCode => 2,
        SyntaxTheme::Zed => 4,
        SyntaxTheme::Intellij => 6,
        SyntaxTheme::IntellijClassic => 8,
    };
    &PRESETS[pair + usize::from(ui_theme == "lapis.white")]
}

pub(crate) fn active() -> &'static Preset {
    resolve(Some(active_family()), super::active_id().as_str())
}

impl Preset {
    pub(crate) fn foreground(&self) -> gpui::Hsla {
        gpui::rgb(self.foreground).into()
    }

    pub(crate) fn color(&self, scopes: &[Scope]) -> gpui::Hsla {
        SELECTORS
            .iter()
            .enumerate()
            .filter_map(|(i, selector)| selector.does_match(scopes).map(|power| (power, i)))
            .max_by_key(|(power, _)| *power)
            .map_or_else(
                || self.foreground(),
                |(_, i)| gpui::rgb(self.colors[i]).into(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension_ui::ThemeId;

    #[test]
    fn five_families_have_ten_scoped_palettes() {
        assert_eq!(FAMILIES.len(), 5);
        assert_eq!(PRESETS.len(), 10);
        let keyword: Scope = "keyword.control.rust".parse().unwrap();
        let unknown: Scope = "source.unknown".parse().unwrap();
        for preset in &PRESETS {
            assert_ne!(preset.color(&[keyword]), preset.foreground());
            assert_eq!(preset.color(&[unknown]), preset.foreground());
        }
        for (family, _) in FAMILIES {
            assert_ne!(
                resolve(Some(family), "lapis.dark").foreground(),
                resolve(Some(family), "lapis.white").foreground()
            );
        }
        assert_eq!(resolve(None, "lapis.dark").foreground, 0xe5e7eb);
        assert_eq!(resolve(None, "lapis.white").foreground, 0x334155);
    }

    #[test]
    fn intellij_new_and_classic_keep_distinct_colors_in_both_ui_modes() {
        let string: Scope = "string.quoted.double".parse().unwrap();
        for ui in ["lapis.dark", "lapis.white"] {
            let modern = resolve(Some(SyntaxTheme::Intellij), ui);
            let classic = resolve(Some(SyntaxTheme::IntellijClassic), ui);
            assert_ne!(modern.color(&[string]), classic.color(&[string]));
        }
        assert_eq!(
            resolve(Some(SyntaxTheme::IntellijClassic), "lapis.dark").colors[1],
            0x6a8759
        );
        assert_eq!(
            resolve(Some(SyntaxTheme::IntellijClassic), "lapis.white").colors[1],
            0x008000
        );
    }

    #[test]
    fn explicit_family_follows_ui_without_changing_the_selection_or_ui_colors() {
        let original = super::super::active_id();
        for (family, _) in FAMILIES {
            let previous_ui = super::super::active_id();
            let previous_background = super::super::colors().background_primary;
            set_active(Some(family));
            assert_eq!(super::super::active_id(), previous_ui);
            assert_eq!(
                super::super::colors().background_primary,
                previous_background
            );
            for ui in ["lapis.dark", "lapis.white", "lapis.dark"] {
                assert!(super::super::set_active(&ThemeId::new(ui)));
                let background = super::super::colors().background_primary;
                assert_eq!(active_family(), family);
                assert_eq!(
                    active().foreground(),
                    resolve(Some(family), ui).foreground()
                );
                assert_eq!(super::super::active_id().as_str(), ui);
                assert_eq!(super::super::colors().background_primary, background);
            }
        }
        set_active(None);
        assert!(super::super::set_active(&original));
    }
}
