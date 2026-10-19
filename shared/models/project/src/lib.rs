use std::path::{Path, PathBuf};

use lapis_editor_core::ProjectId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    id: ProjectId,
    root: PathBuf,
    display_name: String,
}

impl Project {
    pub fn new(id: ProjectId, root: PathBuf) -> Self {
        let display_name = root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Workspace")
            .to_owned();
        Self {
            id,
            root,
            display_name,
        }
    }

    pub fn id(&self) -> &ProjectId {
        &self.id
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }
}
