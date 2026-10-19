use std::path::Path;

use super::{LanguageDefinition, LanguageId, languages};

#[derive(Clone, Debug, Default)]
pub struct LanguageRegistry {
    definitions: Vec<LanguageDefinition>,
}

impl LanguageRegistry {
    pub fn bundled() -> Self {
        Self {
            definitions: languages::bundled(),
        }
    }

    pub fn register(&mut self, definition: LanguageDefinition) -> Result<(), LanguageId> {
        if self
            .definitions
            .iter()
            .any(|current| current.id == definition.id)
        {
            return Err(definition.id);
        }
        self.definitions.push(definition);
        Ok(())
    }

    pub fn detect_path(&self, path: &Path) -> Option<LanguageId> {
        let extension = path.extension()?.to_str()?;
        self.definitions
            .iter()
            .find(|definition| {
                definition
                    .extensions
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(extension))
            })
            .map(|definition| definition.id.clone())
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn bundled_registry_detects_all_supported_languages() {
        let registry = LanguageRegistry::bundled();
        let cases = [
            ("src/lib.rs", "rust"),
            ("README.MD", "markdown"),
            ("app.js", "javascript"),
            ("APP.JS", "javascript"),
            ("app.mjs", "javascript"),
            ("app.cjs", "javascript"),
            ("app.ts", "typescript"),
            ("app.mts", "typescript"),
            ("app.cts", "typescript"),
            ("Component.jsx", "javascriptreact"),
            ("Component.tsx", "typescriptreact"),
            ("COMPONENT.TSX", "typescriptreact"),
            ("main.go", "go"),
            ("Main.kt", "kotlin"),
            ("build.gradle.kts", "kotlin"),
            ("Main.java", "java"),
            ("index.html", "html"),
            ("app.css", "css"),
            ("data.json", "json"),
            ("layout.xml", "xml"),
            ("config.yaml", "yaml"),
            ("config.yml", "yaml"),
        ];

        for (path, expected) in cases {
            assert_eq!(
                registry.detect_path(Path::new(path)),
                Some(LanguageId::new(expected)),
                "language detection failed for {path}"
            );
        }
    }

    #[test]
    fn unknown_or_compound_extensions_are_not_detected() {
        let registry = LanguageRegistry::bundled();

        for path in ["LICENSE", "app.tsx.bak", "archive.zip"] {
            assert_eq!(registry.detect_path(Path::new(path)), None);
        }
    }
}
