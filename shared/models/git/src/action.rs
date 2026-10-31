use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum GitAction {
    Diff {
        path: String,
        staged: bool,
    },
    Stage {
        path: String,
    },
    Unstage {
        path: String,
    },
    StageHunk {
        path: String,
        staged: bool,
        hunk: usize,
        expected_patch: String,
    },
    Commit {
        message: String,
    },
    History,
    Branches,
    CreateBranch {
        name: String,
    },
    SwitchBranch {
        name: String,
    },
    DeleteBranch {
        name: String,
    },
    Fetch,
    Pull,
    Push,
    Stashes,
    StashPush {
        message: String,
    },
    StashApply {
        index: u32,
        expected_oid: String,
    },
    StashPop {
        index: u32,
        expected_oid: String,
    },
    StashDrop {
        index: u32,
        expected_oid: String,
    },
    Merge {
        branch: String,
    },
    AbortMerge,
    Resolve {
        path: String,
        side: ConflictSide,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictSide {
    Ours,
    Theirs,
}

impl GitAction {
    pub const fn is_mutation(&self) -> bool {
        !matches!(
            self,
            Self::Diff { .. } | Self::History | Self::Branches | Self::Stashes
        )
    }
    pub fn needs_confirmation(&self) -> bool {
        matches!(
            self,
            Self::SwitchBranch { .. }
                | Self::DeleteBranch { .. }
                | Self::Pull
                | Self::Push
                | Self::StashPush { .. }
                | Self::StashApply { .. }
                | Self::StashPop { .. }
                | Self::StashDrop { .. }
                | Self::Merge { .. }
                | Self::AbortMerge
                | Self::Resolve { .. }
        )
    }
    pub fn path(&self) -> Option<&str> {
        match self {
            Self::Diff { path, .. }
            | Self::Stage { path }
            | Self::Unstage { path }
            | Self::StageHunk { path, .. }
            | Self::Resolve { path, .. } => Some(path),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitActionRequest {
    pub action: GitAction,
    /// 表示・確認したHEAD。空文字列は初回Commit前のRepositoryを表す。
    pub expected_head: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitActionResult {
    pub output: String,
    #[serde(default)]
    pub diff: Option<crate::GitDiffSnapshot>,
    #[serde(default)]
    pub stashes: Vec<GitStash>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitStash {
    pub index: u32,
    pub oid: String,
    pub subject: String,
}
