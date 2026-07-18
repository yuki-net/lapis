use std::sync::mpsc;

use lapis_app_services::LspSession;
use lapis_language::LanguageRegistry;
use lapis_lsp::{CompletionItem, DefinitionTarget};

pub(crate) struct ProblemsFeature {
    pub lsp: LspSession,
    pub languages: LanguageRegistry,
    pub completion_receiver:
        Option<mpsc::Receiver<Result<Vec<CompletionItem>, lapis_lsp::LspError>>>,
    pub definition_receiver:
        Option<mpsc::Receiver<Result<Option<DefinitionTarget>, lapis_lsp::LspError>>>,
}

impl ProblemsFeature {
    pub fn new(lsp: LspSession) -> Self {
        Self {
            lsp,
            languages: LanguageRegistry::bundled(),
            completion_receiver: None,
            definition_receiver: None,
        }
    }
}
