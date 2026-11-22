//! 設定の所有者（スコープ）と、解決対象のコンテキスト。

use serde::{Deserialize, Serialize};

/// Host の同一性。接続先ごとに別の設定を許すために使う。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HostId(String);

impl HostId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Project の同一性。共有設定とローカル設定を区別して保持する。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProjectId(String);

impl ProjectId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// スコープの種類。定義データは所有者IDを持たない種類だけを宣言する。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScopeKind {
    User,
    Host,
    ProjectShared,
    ProjectLocal,
}

/// 設定値の所有者。列挙順が優先順位で、後ろほど強く適用される。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SettingsScope {
    /// このユーザーの全ウィンドウで共有する設定。
    User,
    /// 接続先 Host 単位の設定。
    Host { id: HostId },
    /// Project の共有設定。リポジトリ内に保存する想定で、書き込みは未対応。
    ProjectShared { id: ProjectId },
    /// Project のローカル設定。このマシンだけに保存する。
    ProjectLocal { id: ProjectId },
}

impl SettingsScope {
    pub fn kind(&self) -> ScopeKind {
        match self {
            Self::User => ScopeKind::User,
            Self::Host { .. } => ScopeKind::Host,
            Self::ProjectShared { .. } => ScopeKind::ProjectShared,
            Self::ProjectLocal { .. } => ScopeKind::ProjectLocal,
        }
    }
}

/// 設定を解決する対象。Project や Host が無い場合はそのスコープを参照しない。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SettingsContext {
    host: Option<HostId>,
    project: Option<ProjectId>,
}

impl SettingsContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_host(mut self, host: HostId) -> Self {
        self.host = Some(host);
        self
    }

    pub fn with_project(mut self, project: ProjectId) -> Self {
        self.project = Some(project);
        self
    }

    pub fn host(&self) -> Option<&HostId> {
        self.host.as_ref()
    }

    pub fn project(&self) -> Option<&ProjectId> {
        self.project.as_ref()
    }

    /// 優先順位の低い順に、解決で参照するスコープを返す。
    pub fn scopes(&self) -> Vec<SettingsScope> {
        let mut scopes = vec![SettingsScope::User];
        if let Some(host) = &self.host {
            scopes.push(SettingsScope::Host { id: host.clone() });
        }
        if let Some(project) = &self.project {
            scopes.push(SettingsScope::ProjectShared {
                id: project.clone(),
            });
            scopes.push(SettingsScope::ProjectLocal {
                id: project.clone(),
            });
        }
        scopes
    }

    /// このコンテキストで値を参照できるスコープか。
    pub fn contains(&self, scope: &SettingsScope) -> bool {
        self.scopes().contains(scope)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_are_ordered_from_user_to_local_project() {
        let context = SettingsContext::new()
            .with_host(HostId::new("host-a"))
            .with_project(ProjectId::new("lapis"));
        assert_eq!(
            context.scopes(),
            vec![
                SettingsScope::User,
                SettingsScope::Host {
                    id: HostId::new("host-a")
                },
                SettingsScope::ProjectShared {
                    id: ProjectId::new("lapis")
                },
                SettingsScope::ProjectLocal {
                    id: ProjectId::new("lapis")
                },
            ]
        );
        assert!(context.contains(&SettingsScope::ProjectLocal {
            id: ProjectId::new("lapis")
        }));
        assert!(!context.contains(&SettingsScope::ProjectLocal {
            id: ProjectId::new("other")
        }));
    }

    #[test]
    fn scope_kind_round_trips_through_json() {
        for scope in [
            SettingsScope::User,
            SettingsScope::Host {
                id: HostId::new("host-a"),
            },
            SettingsScope::ProjectShared {
                id: ProjectId::new("lapis"),
            },
            SettingsScope::ProjectLocal {
                id: ProjectId::new("lapis"),
            },
        ] {
            let json = serde_json::to_string(&scope).unwrap();
            let loaded: SettingsScope = serde_json::from_str(&json).unwrap();
            assert_eq!(loaded, scope);
        }
        assert_eq!(
            serde_json::to_value(SettingsScope::User).unwrap(),
            serde_json::json!({"kind": "user"})
        );
    }

    #[test]
    fn project_scopes_require_a_project_context() {
        let context = SettingsContext::new().with_host(HostId::new("host-a"));
        assert_eq!(
            context.scopes(),
            vec![
                SettingsScope::User,
                SettingsScope::Host {
                    id: HostId::new("host-a")
                }
            ]
        );
    }
}
