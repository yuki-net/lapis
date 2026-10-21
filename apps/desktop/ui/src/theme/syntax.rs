//! 構文スコープの色だけを解決する。原典・ライセンスはassets/syntax-themes/NOTICE.md。
use std::{cell::RefCell, sync::LazyLock};
use syntect::{highlighting::ScopeSelectors, parsing::Scope};

pub(crate) struct Preset {
    pub(crate) id: &'static str,
    pub(crate) name: &'static str,
    foreground: u32,
    colors: [u32; 14],
}

// 順序: comment, string, number, boolean, control, keyword, function, type,
// tag, attribute, variable, constant, operator, markup。スコープの粒度はSyntectに委ねる。
pub(crate) static PRESETS: [Preset; 6] = [
    Preset {
        id: "lapis.dark",
        name: "Lapis Dark",
        foreground: 0xe5e7eb,
        colors: [
            0x6a9955, 0xce9178, 0xb5cea8, 0xb5cea8, 0xc586c0, 0xc586c0, 0xdcdcaa, 0x4ec9b0,
            0x569cd6, 0x9cdcfe, 0xe5e7eb, 0x9cdcfe, 0xc586c0, 0x569cd6,
        ],
    },
    Preset {
        id: "lapis.light",
        name: "Lapis Light",
        foreground: 0x334155,
        colors: [
            0x64748b, 0x287c50, 0xa35a00, 0xa35a00, 0x7c3aed, 0x7c3aed, 0x9a6700, 0x087e8b,
            0x2563eb, 0x8b4d86, 0x334155, 0x8b4d86, 0x7c3aed, 0x2563eb,
        ],
    },
    Preset {
        id: "vscode.dark-plus",
        name: "VS Code Dark+",
        foreground: 0xd4d4d4,
        colors: [
            0x6a9955, 0xce9178, 0xb5cea8, 0x569cd6, 0xc586c0, 0x569cd6, 0xdcdcaa, 0x4ec9b0,
            0x569cd6, 0x9cdcfe, 0x9cdcfe, 0x4fc1ff, 0xd4d4d4, 0x569cd6,
        ],
    },
    Preset {
        id: "zed.one-dark",
        name: "Zed One Dark",
        foreground: 0xacb2be,
        colors: [
            0x5d636f, 0xa1c181, 0xbf956a, 0xbf956a, 0xb477cf, 0xb477cf, 0x73ade9, 0x6eb4bf,
            0x74ade8, 0x74ade8, 0xacb2be, 0xdfc184, 0x6eb4bf, 0xd07277,
        ],
    },
    Preset {
        id: "intellij.dark",
        name: "IntelliJ Dark",
        foreground: 0xbcbec4,
        colors: [
            0x7a7e85, 0x6aab73, 0x2aacb8, 0xcf8e6d, 0xcf8e6d, 0xcf8e6d, 0x56a8f5, 0xbcbec4,
            0xd5b778, 0xbababa, 0xbcbec4, 0xc77dbb, 0xbcbec4, 0xcf8e6d,
        ],
    },
    Preset {
        id: "intellij.darcula",
        name: "IntelliJ Darcula",
        foreground: 0xa9b7c6,
        colors: [
            0x808080, 0x6a8759, 0x6897bb, 0xcc7832, 0xcc7832, 0xcc7832, 0xffc66d, 0xa9b7c6,
            0xe8bf6a, 0xbababa, 0xa9b7c6, 0x9876aa, 0xa9b7c6, 0xcc7832,
        ],
    },
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

thread_local! { static SELECTED: RefCell<Option<String>> = const { RefCell::new(None) }; }

pub(crate) fn set_active(id: Option<String>) {
    SELECTED.with(|selected| *selected.borrow_mut() = id);
}

pub(crate) fn resolve(id: Option<&str>, ui_theme: &str) -> &'static Preset {
    id.and_then(|id| PRESETS.iter().find(|preset| preset.id == id))
        .unwrap_or_else(|| {
            if ui_theme == "lapis.white" {
                &PRESETS[1]
            } else {
                &PRESETS[0]
            }
        })
}

pub(crate) fn active() -> &'static Preset {
    SELECTED.with(|selected| resolve(selected.borrow().as_deref(), super::active_id().as_str()))
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

    #[test]
    fn six_presets_have_scoped_colors_and_a_plain_text_fallback() {
        let keyword: Scope = "keyword.control.rust".parse().unwrap();
        let unknown: Scope = "source.unknown".parse().unwrap();
        let mut ids = std::collections::HashSet::new();
        for preset in &PRESETS {
            assert!(ids.insert(preset.id));
            assert_ne!(preset.color(&[keyword]), preset.foreground());
            assert_eq!(preset.color(&[unknown]), preset.foreground());
        }
    }

    #[test]
    fn implicit_selection_follows_ui_and_explicit_selection_is_independent() {
        assert_eq!(resolve(None, "lapis.dark").id, "lapis.dark");
        assert_eq!(resolve(None, "lapis.white").id, "lapis.light");
        assert_eq!(resolve(Some("missing"), "lapis.white").id, "lapis.light");
        assert_eq!(
            resolve(Some("zed.one-dark"), "lapis.white").id,
            "zed.one-dark"
        );
    }
}
