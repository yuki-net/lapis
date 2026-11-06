use std::path::Path;

use language::{LanguageId, LanguageRegistry};
use workspace::FileEntryKind;

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
            .and_then(icon_override_for_language)
            .unwrap_or(FileIconId::TextAlignStart),
        language,
    }
}

fn icon_override_for_language(language: &LanguageId) -> Option<FileIconId> {
    match language.as_str() {
        "javascript" | "javascriptreact" => Some(FileIconId::Javascript),
        "typescript" | "typescriptreact" => Some(FileIconId::Typescript),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_icons_override_fallback_only_when_available() {
        let languages = LanguageRegistry::bundled();
        for (path, expected) in [
            ("component.jsx", FileIconId::Javascript),
            ("component.tsx", FileIconId::Typescript),
            (".coderabbit.yaml", FileIconId::TextAlignStart),
            ("main.rs", FileIconId::TextAlignStart),
            ("unknown.ext", FileIconId::TextAlignStart),
        ] {
            assert_eq!(
                display_info(Path::new(path), FileEntryKind::File, &languages).icon,
                expected,
                "{path}"
            );
        }
        assert!(
            display_info(
                Path::new(".coderabbit.yaml"),
                FileEntryKind::File,
                &languages
            )
            .language
            .is_some()
        );
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
