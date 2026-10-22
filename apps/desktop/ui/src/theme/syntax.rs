use std::sync::LazyLock;
use syntect::{highlighting::ScopeSelectors, parsing::Scope};

static RULES: LazyLock<Vec<(ScopeSelectors, u32)>> = LazyLock::new(|| {
    [
        ("comment", 0x6a9955),
        ("string", 0xce9178),
        ("constant.numeric, constant.language", 0xb5cea8),
        ("keyword, storage", 0xc586c0),
        ("entity.name.function, support.function", 0xdcdcaa),
        (
            "entity.name.type, support.type, entity.name.class",
            0x4ec9b0,
        ),
        ("entity.name.tag", 0x569cd6),
        ("entity.other.attribute-name, variable.parameter", 0x9cdcfe),
    ]
    .into_iter()
    .map(|(s, color)| (s.parse().expect("bundled syntax selector"), color))
    .collect()
});

pub(crate) fn color(scopes: &[Scope], fallback: gpui::Hsla) -> gpui::Hsla {
    RULES
        .iter()
        .filter_map(|(selector, color)| selector.does_match(scopes).map(|power| (power, color)))
        .max_by_key(|(power, _)| *power)
        .map_or(fallback, |(_, color)| gpui::rgb(*color).into())
}
