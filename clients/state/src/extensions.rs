//! Client 側の拡張機能レジストリ。

/// 組み込み拡張の識別子。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExtensionId {
    Git,
}

/// Client が利用する拡張の宣言。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExtensionDescriptor {
    pub id: ExtensionId,
    pub name: &'static str,
    /// Host が要求する capability。Client は接続時にこれを要求する。
    pub read_capability: &'static str,
    pub write_capability: &'static str,
}

/// 登録済み拡張の一覧。重複登録は無視する。
#[derive(Clone, Debug, Default)]
pub struct ExtensionRegistry {
    extensions: Vec<ExtensionDescriptor>,
}

impl ExtensionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 組み込み拡張を登録した状態を作る。
    pub fn with_defaults() -> Self {
        let mut registry = Self::new();
        registry.register(git_extension());
        registry
    }

    /// 未登録なら追加して true を返す。既に同じ ID があれば false。
    pub fn register(&mut self, descriptor: ExtensionDescriptor) -> bool {
        if self.get(descriptor.id).is_some() {
            return false;
        }
        self.extensions.push(descriptor);
        true
    }

    pub fn get(&self, id: ExtensionId) -> Option<&ExtensionDescriptor> {
        self.extensions.iter().find(|extension| extension.id == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &ExtensionDescriptor> {
        self.extensions.iter()
    }
}

pub fn git_extension() -> ExtensionDescriptor {
    ExtensionDescriptor {
        id: ExtensionId::Git,
        name: "git",
        read_capability: "workspace.git.read",
        write_capability: "workspace.git.write",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_contain_git() {
        let registry = ExtensionRegistry::with_defaults();
        let git = registry.get(ExtensionId::Git).unwrap();
        assert_eq!(git.name, "git");
        assert_eq!(git.read_capability, "workspace.git.read");
        assert_eq!(git.write_capability, "workspace.git.write");
    }

    #[test]
    fn duplicate_registration_is_ignored() {
        let mut registry = ExtensionRegistry::with_defaults();
        assert!(!registry.register(git_extension()));
        assert_eq!(registry.iter().count(), 1);
    }
}
