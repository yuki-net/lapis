use super::LanguageDefinition;

pub(super) fn definition() -> LanguageDefinition {
    LanguageDefinition::new("html", ["html", "htm"])
}
