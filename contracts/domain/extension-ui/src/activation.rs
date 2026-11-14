use language::LanguageId;

use crate::{CommandId, ViewId, WorkspaceCapabilityId};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ActivationCondition {
    OnWorkspaceOpen,
    OnLanguage(LanguageId),
    OnCommand(CommandId),
    OnView(ViewId),
    OnWorkspaceCapability(WorkspaceCapabilityId),
}
