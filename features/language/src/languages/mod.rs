mod go;
mod java;
mod javascript;
mod kotlin;
mod markdown;
mod rust;
mod typescript;

use super::LanguageDefinition;

pub(super) fn bundled() -> Vec<LanguageDefinition> {
    vec![
        rust::definition(),
        markdown::definition(),
        javascript::definition(),
        javascript::react_definition(),
        typescript::definition(),
        typescript::react_definition(),
        go::definition(),
        kotlin::definition(),
        java::definition(),
    ]
}
