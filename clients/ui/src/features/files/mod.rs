use crate::features::id;
use extension_ui_contract::{ActivationCondition, FeatureDescriptor, UiContribution, UiSlot};

mod display;
mod view;

pub(crate) use display::display_info;

pub(super) fn descriptor() -> FeatureDescriptor {
    FeatureDescriptor::bundled(id::FEATURE_FILES, [ActivationCondition::OnWorkspaceOpen])
        .contributes(UiContribution::view(
            id::VIEW_FILES,
            UiSlot::ToolDock,
            "view.files",
            "files",
            10,
        ))
        .requires("workspace.files.read")
}
use crate::{client_adapter::ClientSnapshot, commands::UiCommand, theme};
use gpui::{Context, Render, Window, div, prelude::*, px};
use std::{collections::HashSet, path::PathBuf};
use workspace::{FileEntry, FileEntryKind};

pub(crate) struct FilesFeature {
    workspace_name: String,
    entries: Vec<FileEntry>,
    active_path: Option<PathBuf>,
    root: Option<PathBuf>,
    expanded_directories: HashSet<PathBuf>,
    languages: language::LanguageRegistry,
}
impl gpui::EventEmitter<UiCommand> for FilesFeature {}
impl FilesFeature {
    pub(crate) fn new() -> Self {
        Self {
            workspace_name: String::new(),
            entries: Vec::new(),
            active_path: None,
            root: None,
            expanded_directories: HashSet::new(),
            languages: language::LanguageRegistry::bundled(),
        }
    }
    pub(crate) fn update(&mut self, snapshot: &ClientSnapshot, cx: &mut Context<Self>) {
        if self.root != snapshot.root {
            self.expanded_directories.clear();
        }
        self.root = snapshot.root.clone();
        self.workspace_name = snapshot.workspace_name.clone();
        self.entries = snapshot.files.clone();
        self.active_path = snapshot
            .tabs
            .iter()
            .find(|tab| tab.active)
            .and_then(|tab| tab.path.clone());
        cx.notify();
    }
}
impl Render for FilesFeature {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_files_content(cx)
    }
}
