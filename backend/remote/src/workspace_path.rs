use std::{error::Error, fmt, fs, io::ErrorKind, path::PathBuf};

use lapis_client_api::WorkspaceRelativePath;

#[derive(Clone, Debug)]
pub struct WorkspacePathResolver {
    canonical_root: PathBuf,
}

impl WorkspacePathResolver {
    pub fn new(root: PathBuf) -> Result<Self, PathSecurityError> {
        let canonical_root = fs::canonicalize(root).map_err(PathSecurityError::Io)?;
        if !canonical_root.is_dir() {
            return Err(PathSecurityError::InvalidRoot);
        }
        Ok(Self { canonical_root })
    }

    pub fn root(&self) -> &std::path::Path {
        &self.canonical_root
    }

    pub fn resolve_existing(
        &self,
        relative: &WorkspaceRelativePath,
    ) -> Result<PathBuf, PathSecurityError> {
        let candidate = self.canonical_root.join(relative.as_str());
        let canonical = fs::canonicalize(candidate).map_err(PathSecurityError::Io)?;
        self.require_inside(canonical)
    }

    pub fn resolve_new_file(
        &self,
        relative: &WorkspaceRelativePath,
    ) -> Result<PathBuf, PathSecurityError> {
        let candidate = self.canonical_root.join(relative.as_str());
        match fs::symlink_metadata(&candidate) {
            Ok(_) => return self.resolve_existing(relative),
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(PathSecurityError::Io(error)),
        }

        let parent = candidate
            .parent()
            .ok_or(PathSecurityError::WorkspaceEscape)?;
        let canonical_parent = fs::canonicalize(parent).map_err(PathSecurityError::Io)?;
        let canonical_parent = self.require_inside(canonical_parent)?;
        let name = candidate
            .file_name()
            .ok_or(PathSecurityError::WorkspaceEscape)?;
        Ok(canonical_parent.join(name))
    }

    fn require_inside(&self, candidate: PathBuf) -> Result<PathBuf, PathSecurityError> {
        if candidate.starts_with(&self.canonical_root) {
            Ok(candidate)
        } else {
            Err(PathSecurityError::WorkspaceEscape)
        }
    }
}

#[derive(Debug)]
pub enum PathSecurityError {
    InvalidRoot,
    WorkspaceEscape,
    Io(std::io::Error),
}

impl fmt::Display for PathSecurityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRoot => formatter.write_str("workspace root is not a directory"),
            Self::WorkspaceEscape => formatter.write_str("path resolves outside the workspace"),
            Self::Io(error) => write!(formatter, "workspace path resolution failed: {error}"),
        }
    }
}

impl Error for PathSecurityError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_existing_and_new_files_inside_workspace() {
        let workspace = tempfile::tempdir().unwrap();
        fs::create_dir(workspace.path().join("src")).unwrap();
        fs::write(workspace.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        let resolver = WorkspacePathResolver::new(workspace.path().to_owned()).unwrap();

        let existing = resolver
            .resolve_existing(&WorkspaceRelativePath::parse("src/main.rs").unwrap())
            .unwrap();
        let new_file = resolver
            .resolve_new_file(&WorkspaceRelativePath::parse("src/new.rs").unwrap())
            .unwrap();

        assert!(existing.starts_with(resolver.root()));
        assert!(new_file.starts_with(resolver.root()));
    }

    #[test]
    fn rejects_symlink_escape_when_platform_allows_symlinks() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_file = outside.path().join("secret.txt");
        fs::write(&outside_file, "secret").unwrap();
        let link = workspace.path().join("linked-secret.txt");

        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside_file, &link).unwrap();
        #[cfg(windows)]
        if std::os::windows::fs::symlink_file(&outside_file, &link).is_err() {
            return;
        }

        let resolver = WorkspacePathResolver::new(workspace.path().to_owned()).unwrap();
        assert!(matches!(
            resolver.resolve_existing(&WorkspaceRelativePath::parse("linked-secret.txt").unwrap()),
            Err(PathSecurityError::WorkspaceEscape)
        ));
    }

    #[test]
    fn rejects_dangling_symlink_as_new_file_when_platform_allows_symlinks() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_file = outside.path().join("not-created.txt");
        let link = workspace.path().join("new.txt");

        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside_file, &link).unwrap();
        #[cfg(windows)]
        if std::os::windows::fs::symlink_file(&outside_file, &link).is_err() {
            return;
        }

        let resolver = WorkspacePathResolver::new(workspace.path().to_owned()).unwrap();
        assert!(
            resolver
                .resolve_new_file(&WorkspaceRelativePath::parse("new.txt").unwrap())
                .is_err()
        );
    }
}
