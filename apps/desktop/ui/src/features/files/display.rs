use std::path::Path;

use lapis_language::{LanguageId, LanguageRegistry};
use lapis_workspace::FileEntryKind;

use crate::components::FileIconId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileDisplayInfo {
    pub icon: FileIconId,
    pub language: Option<LanguageId>,
}

pub(crate) fn display_info(
    path: &Path,
    kind: FileEntryKind,
    languages: &LanguageRegistry,
) -> FileDisplayInfo {
    if kind == FileEntryKind::Directory {
        return FileDisplayInfo {
            icon: FileIconId::TextAlignStart,
            language: None,
        };
    }

    let language = languages.detect_path(path);
    FileDisplayInfo {
        icon: language
            .as_ref()
            .map(icon_for_language)
            .unwrap_or(FileIconId::TextAlignStart),
        language,
    }
}

pub(crate) fn icon_for_language(language: &LanguageId) -> FileIconId {
    match language.as_str() {
        "javascript" | "javascriptreact" => FileIconId::Javascript,
        "typescript" | "typescriptreact" => FileIconId::Typescript,
        "html" | "css" | "markdown" | "json" | "xml" | "yaml" => FileIconId::TextAlignStart,
        _ => FileIconId::TextAlignStart,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_supported_languages_to_file_icons() {
        assert_eq!(
            icon_for_language(&LanguageId::new("javascriptreact")),
            FileIconId::Javascript
        );
        assert_eq!(
            icon_for_language(&LanguageId::new("typescriptreact")),
            FileIconId::Typescript
        );

        for language in [
            "html", "css", "markdown", "json", "xml", "yaml", "rust", "go",
        ] {
            assert_eq!(
                icon_for_language(&LanguageId::new(language)),
                FileIconId::TextAlignStart,
                "fallback icon mismatch for {language}"
            );
        }
    }

    #[test]
    fn directories_and_unknown_files_use_fallback_icons() {
        let languages = LanguageRegistry::bundled();
        assert_eq!(
            display_info(Path::new("src"), FileEntryKind::Directory, &languages).icon,
            FileIconId::TextAlignStart
        );
        assert_eq!(
            display_info(Path::new("LICENSE"), FileEntryKind::File, &languages).icon,
            FileIconId::TextAlignStart
        );
    }
}
