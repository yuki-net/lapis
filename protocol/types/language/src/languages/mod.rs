mod css;
mod go;
mod html;
mod java;
mod javascript;
mod json;
mod kotlin;
mod markdown;
mod rust;
mod typescript;
mod xml;
mod yaml;

use super::LanguageDefinition;

pub(super) fn bundled() -> Vec<LanguageDefinition> {
    vec![
        rust::definition(),
        markdown::definition(),
        css::definition(),
        html::definition(),
        json::definition(),
        xml::definition(),
        yaml::definition(),
        javascript::definition(),
        javascript::react_definition(),
        typescript::definition(),
        typescript::react_definition(),
        go::definition(),
        kotlin::definition(),
        java::definition(),
    ]
}
