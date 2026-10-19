use super::LanguageDefinition;

pub(super) fn definition() -> LanguageDefinition {
    LanguageDefinition::new("javascript", ["js", "mjs", "cjs"])
}

pub(super) fn react_definition() -> LanguageDefinition {
    LanguageDefinition::new("javascriptreact", ["jsx"])
}
