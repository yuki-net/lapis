use crate::{
    extension_ui::{ActivationCondition, FeatureDescriptor, UiContribution, UiSlot, ViewId},
    features::id,
};

mod changes;
mod state;
mod view;

use crate::theme;
use lapis_git::ChangeKind;

pub(crate) use changes::ChangesFeature;
pub(crate) use state::{GitEvent, GitFeature, GitInputRequest};

pub(super) fn descriptor() -> FeatureDescriptor {
    FeatureDescriptor::bundled(
        id::FEATURE_GIT,
        [
            ActivationCondition::OnView(ViewId::new(id::VIEW_GIT)),
            ActivationCondition::OnView(ViewId::new(id::VIEW_CHANGES)),
        ],
    )
    .contributes(UiContribution::view(
        id::VIEW_GIT,
        UiSlot::ToolDock,
        "view.git",
        "git",
        30,
    ))
    .contributes(UiContribution::view(
        id::VIEW_CHANGES,
        UiSlot::ToolDock,
        "view.changes",
        "git",
        29,
    ))
    .requires("workspace.git")
}

fn change_label(kind: ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Added => "A",
        ChangeKind::Modified => "M",
        ChangeKind::Deleted => "D",
        ChangeKind::Renamed => "R",
        ChangeKind::Untracked => "?",
        ChangeKind::Conflicted => "!",
    }
}

fn change_color(kind: ChangeKind) -> gpui::Rgba {
    match kind {
        ChangeKind::Added => theme::colors().positive_text,
        ChangeKind::Deleted | ChangeKind::Conflicted => theme::colors().danger_text,
        ChangeKind::Modified | ChangeKind::Renamed => theme::colors().warning_text,
        ChangeKind::Untracked => theme::colors().text_secondary,
    }
}
