//! 複数機能で共有する不透明IDのための最小境界。

use std::fmt;

use serde::{Deserialize, Serialize};

macro_rules! define_id {
    ($name:ident) => {
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

define_id!(ProjectId);
define_id!(WorkspaceId);
define_id!(ConversationId);
define_id!(DocumentId);
define_id!(TaskId);
define_id!(ExecutionId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_of_different_kinds_are_distinct_types() {
        let project = ProjectId::new("same");
        let workspace = WorkspaceId::new("same");
        assert_eq!(project.as_str(), workspace.as_str());
        assert_eq!(project.to_string(), "same");
    }
}
