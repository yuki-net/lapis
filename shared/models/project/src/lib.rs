use std::path::{Path, PathBuf};

use lapis_editor_core::ProjectId;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    id: ProjectId,
    root: PathBuf,
    display_name: String,
    #[serde(default)]
    additional_sources: Vec<PathBuf>,
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
            additional_sources: Vec::new(),
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

    pub fn sources(&self) -> impl Iterator<Item = &PathBuf> {
        std::iter::once(&self.root).chain(&self.additional_sources)
    }

    pub fn register_source(&mut self, root: PathBuf) {
        if !self.sources().any(|source| source == &root) {
            self.additional_sources.push(root);
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceRole {
    #[default]
    Primary,
    Sub,
}

/// 作業Taskの既存環境。AgentのExecutionやWorktree作成とは独立する。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskEnvironment {
    project: ProjectId,
    primary: PathBuf,
    sub: Option<PathBuf>,
}

impl TaskEnvironment {
    pub fn new(
        project: &Project,
        primary: PathBuf,
        sub: Option<PathBuf>,
    ) -> Result<Self, &'static str> {
        if !project.sources().any(|source| source == &primary)
            || sub.as_ref().is_some_and(|sub| {
                sub == &primary || !project.sources().any(|source| source == sub)
            })
        {
            return Err("Primary/SubはProjectに登録した異なる既存環境から選択してください");
        }
        Ok(Self {
            project: project.id.clone(),
            primary,
            sub,
        })
    }

    pub fn project(&self) -> &ProjectId {
        &self.project
    }
    pub fn primary(&self) -> &Path {
        &self.primary
    }
    pub fn sub(&self) -> Option<&Path> {
        self.sub.as_deref()
    }
    pub fn root(&self, role: SourceRole) -> Option<&Path> {
        match role {
            SourceRole::Primary => Some(&self.primary),
            SourceRole::Sub => self.sub(),
        }
    }
    pub fn validate(&self, project: &Project) -> bool {
        self.project == project.id
            && Self::new(project, self.primary.clone(), self.sub.clone()).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn task_requires_registered_distinct_primary_and_optional_sub() {
        let mut project = Project::new(ProjectId::new("p"), "one".into());
        project.register_source("two".into());
        project.register_source("two".into());
        assert_eq!(project.sources().count(), 2);
        assert!(TaskEnvironment::new(&project, "one".into(), Some("two".into())).is_ok());
        assert!(TaskEnvironment::new(&project, "outside".into(), None).is_err());
        assert!(TaskEnvironment::new(&project, "one".into(), Some("one".into())).is_err());
        assert!(TaskEnvironment::new(&project, "one".into(), Some("outside".into())).is_err());
    }
}
