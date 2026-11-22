use super::LanguageDefinition;

pub(super) fn definition() -> LanguageDefinition {
    LanguageDefinition::new("typescript", ["ts", "mts", "cts"])
}

pub(super) fn react_definition() -> LanguageDefinition {
    LanguageDefinition::new("typescriptreact", ["tsx"])
}
