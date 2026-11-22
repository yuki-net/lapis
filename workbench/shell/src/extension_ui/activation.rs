use language::LanguageId;

use extension_ui_contract::{CommandId, WorkspaceCapabilityId};

/// Client がレジストリへ渡す実行時の出来事。宣言（契約層）ではなく状態遷移。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActivationEvent {
    WorkspaceOpened,
    LanguageChanged(Option<LanguageId>),
    CommandInvoked(CommandId),
    WorkspaceCapabilityChanged(WorkspaceCapabilityId, bool),
}
