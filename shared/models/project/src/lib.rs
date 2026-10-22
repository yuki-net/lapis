use std::path::{Path, PathBuf};

use lapis_editor_core::{ProjectId, TaskId};
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
    #[serde(default)]
    primary_worktree: Option<WorktreeBinding>,
    #[serde(default)]
    sub_worktree: Option<WorktreeBinding>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeBinding {
    pub id: TaskId,
    pub root: PathBuf,
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
            primary_worktree: None,
            sub_worktree: None,
        })
    }

    pub fn project(&self) -> &ProjectId {
        &self.project
    }

    pub fn reassign_project(&mut self, project: &Project) -> Result<(), &'static str> {
        Self::new(project, self.primary.clone(), self.sub.clone())?;
        self.project = project.id.clone();
        Ok(())
    }
    pub fn primary(&self) -> &Path {
        &self.primary
    }
    pub fn sub(&self) -> Option<&Path> {
        self.sub.as_deref()
    }
    pub fn root(&self, role: SourceRole) -> Option<&Path> {
        self.worktree(role)
            .map(|binding| binding.root.as_path())
            .or_else(|| self.source_root(role))
    }
    pub fn source_root(&self, role: SourceRole) -> Option<&Path> {
        match role {
            SourceRole::Primary => Some(&self.primary),
            SourceRole::Sub => self.sub(),
        }
    }
    pub fn worktree(&self, role: SourceRole) -> Option<&WorktreeBinding> {
        match role {
            SourceRole::Primary => self.primary_worktree.as_ref(),
            SourceRole::Sub => self.sub_worktree.as_ref(),
        }
    }
    pub fn set_worktree(
        &mut self,
        role: SourceRole,
        binding: Option<WorktreeBinding>,
    ) -> Result<(), &'static str> {
        if self.source_root(role).is_none() {
            return Err("このTaskにSubはありません");
        }
        if binding.as_ref().is_some_and(|binding| {
            binding.root.as_os_str().is_empty()
                || binding.id.as_str().is_empty()
                || !binding
                    .id
                    .as_str()
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        }) {
            return Err("Worktreeの参照が不正です");
        }
        let other = match role {
            SourceRole::Primary => SourceRole::Sub,
            SourceRole::Sub => SourceRole::Primary,
        };
        let next = binding
            .as_ref()
            .map(|binding| binding.root.as_path())
            .or_else(|| self.source_root(role));
        if next == self.root(other) {
            return Err("PrimaryとSubは同じ環境を選択できません");
        }
        match role {
            SourceRole::Primary => self.primary_worktree = binding,
            SourceRole::Sub => self.sub_worktree = binding,
        }
        Ok(())
    }
    pub fn validate(&self, project: &Project) -> bool {
        let Ok(_) = Self::new(project, self.primary.clone(), self.sub.clone()) else {
            return false;
        };
        let mut checked = self.clone();
        self.project == project.id
            && checked
                .set_worktree(SourceRole::Primary, self.primary_worktree.clone())
                .is_ok()
            && (self.sub.is_none() && self.sub_worktree.is_none()
                || checked
                    .set_worktree(SourceRole::Sub, self.sub_worktree.clone())
                    .is_ok())
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
