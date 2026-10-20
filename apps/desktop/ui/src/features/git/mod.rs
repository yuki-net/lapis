use crate::{
    extension_ui::{ActivationCondition, FeatureDescriptor, UiContribution, UiSlot, ViewId},
    features::id,
};

mod state;
mod view;

use crate::theme;
use lapis_git::ChangeKind;

pub(crate) use state::GitFeature;

pub(super) fn descriptor() -> FeatureDescriptor {
    FeatureDescriptor::bundled(
        id::FEATURE_GIT,
        [ActivationCondition::OnView(ViewId::new(id::VIEW_GIT))],
    )
    .contributes(UiContribution::view(
        id::VIEW_GIT,
        UiSlot::ToolDock,
        "view.git",
        "git",
        30,
    ))
    .requires("workspace.git")
}

pub(super) fn history_descriptor() -> FeatureDescriptor {
    FeatureDescriptor::bundled(
        id::FEATURE_HISTORY,
        [ActivationCondition::OnView(ViewId::new(id::VIEW_HISTORY))],
    )
    .contributes(UiContribution::view(
        id::VIEW_HISTORY,
        UiSlot::ToolDock,
        "view.history",
        "history",
        40,
    ))
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
