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
            icon: FileIconId::Folder,
            language: None,
        };
    }

    let language = languages.detect_path(path);
    FileDisplayInfo {
        icon: language
            .as_ref()
            .map(icon_for_language)
            .unwrap_or(FileIconId::Unknown),
        language,
    }
}

pub(crate) fn icon_for_language(language: &LanguageId) -> FileIconId {
    match language.as_str() {
        "javascript" | "javascriptreact" => FileIconId::Javascript,
        "typescript" | "typescriptreact" => FileIconId::Typescript,
        "go" => FileIconId::Go,
        "kotlin" => FileIconId::Kotlin,
        "java" => FileIconId::Java,
        _ => FileIconId::Unknown,
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
        assert_eq!(icon_for_language(&LanguageId::new("go")), FileIconId::Go);
        assert_eq!(
            icon_for_language(&LanguageId::new("kotlin")),
            FileIconId::Kotlin
        );
        assert_eq!(
            icon_for_language(&LanguageId::new("java")),
            FileIconId::Java
        );
    }

    #[test]
    fn directories_and_unknown_files_use_fallback_icons() {
        let languages = LanguageRegistry::bundled();
        assert_eq!(
            display_info(Path::new("src"), FileEntryKind::Directory, &languages).icon,
            FileIconId::Folder
        );
        assert_eq!(
            display_info(Path::new("LICENSE"), FileEntryKind::File, &languages).icon,
            FileIconId::Unknown
        );
    }
}
