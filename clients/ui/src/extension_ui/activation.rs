use lapis_language::LanguageId;

use super::{CommandId, ViewId, WorkspaceCapabilityId};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ActivationCondition {
    OnWorkspaceOpen,
    OnLanguage(LanguageId),
    OnCommand(CommandId),
    OnView(ViewId),
    OnWorkspaceCapability(WorkspaceCapabilityId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActivationEvent {
    WorkspaceOpened,
    LanguageChanged(Option<LanguageId>),
    CommandInvoked(CommandId),
    WorkspaceCapabilityChanged(WorkspaceCapabilityId, bool),
}
