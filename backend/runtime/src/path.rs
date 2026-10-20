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

    pub fn resolve_directory(
        &self,
        relative: Option<&WorkspaceRelativePath>,
    ) -> Result<PathBuf, PathSecurityError> {
        let directory = match relative {
            Some(relative) => self.resolve_existing(relative),
            None => Ok(self.canonical_root.clone()),
        }?;
        if !directory.is_dir() {
            return Err(PathSecurityError::InvalidDirectory);
        }
        Ok(directory)
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

    pub fn resolve_scoped_path(
        &self,
        relative: &WorkspaceRelativePath,
    ) -> Result<PathBuf, PathSecurityError> {
        let mut ancestor = self.canonical_root.join(relative.as_str());
        let mut missing = Vec::new();
        loop {
            match fs::symlink_metadata(&ancestor) {
                Ok(_) => {
                    let canonical = fs::canonicalize(&ancestor).map_err(PathSecurityError::Io)?;
                    let mut resolved = self.require_inside(canonical)?;
                    if !missing.is_empty() && !resolved.is_dir() {
                        return Err(PathSecurityError::InvalidDirectory);
                    }
                    resolved.extend(missing.into_iter().rev());
                    return Ok(resolved);
                }
                Err(error) if error.kind() == ErrorKind::NotFound => {
                    let name = ancestor
                        .file_name()
                        .ok_or(PathSecurityError::WorkspaceEscape)?
                        .to_owned();
                    missing.push(name);
                    if !ancestor.pop() {
                        return Err(PathSecurityError::WorkspaceEscape);
                    }
                }
                Err(error) => return Err(PathSecurityError::Io(error)),
            }
        }
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
    InvalidDirectory,
    InvalidEntry,
    WorkspaceEscape,
    Io(std::io::Error),
}

impl fmt::Display for PathSecurityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRoot => formatter.write_str("workspace root is not a directory"),
            Self::InvalidDirectory => formatter.write_str("workspace path is not a directory"),
            Self::InvalidEntry => formatter.write_str("workspace entry cannot be represented"),
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
    fn scoped_path_resolves_missing_nested_tail_without_relaxing_file_creation() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("existing")).unwrap();
        let resolver = WorkspacePathResolver::new(root.path().to_owned()).unwrap();
        for relative in ["removed/nested/file.txt", "existing/removed/file.txt"] {
            let path = WorkspaceRelativePath::parse(relative).unwrap();
            assert_eq!(
                resolver.resolve_scoped_path(&path).unwrap(),
                resolver.root().join(relative)
            );
            assert!(resolver.resolve_new_file(&path).is_err());
        }
    }

    #[test]
    fn scoped_path_does_not_treat_regular_files_as_missing_directories() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("file.txt"), "data").unwrap();
        let resolver = WorkspacePathResolver::new(root.path().to_owned()).unwrap();
        assert!(
            resolver
                .resolve_scoped_path(&WorkspaceRelativePath::parse("file.txt/missing.txt").unwrap())
                .is_err()
        );
        assert_eq!(
            resolver
                .resolve_scoped_path(&WorkspaceRelativePath::parse("file.txt").unwrap())
                .unwrap(),
            resolver.root().join("file.txt")
        );
    }

    #[cfg(any(unix, windows))]
    fn symlink_directory(target: &std::path::Path, link: &std::path::Path) -> bool {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).unwrap();
            true
        }
        #[cfg(windows)]
        {
            match std::os::windows::fs::symlink_dir(target, link) {
                Ok(()) => true,
                Err(error) if error.kind() == ErrorKind::PermissionDenied => false,
                Err(error) => panic!("symlink 作成失敗: {error}"),
            }
        }
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn scoped_path_rejects_external_and_dangling_directory_symlinks_with_missing_tail() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        if !symlink_directory(outside.path(), &root.path().join("escape")) {
            return;
        }
        assert!(symlink_directory(
            &outside.path().join("nonexistent"),
            &root.path().join("dangling")
        ));
        let resolver = WorkspacePathResolver::new(root.path().to_owned()).unwrap();
        assert!(matches!(
            resolver.resolve_scoped_path(
                &WorkspaceRelativePath::parse("escape/deleted/nested/file.txt").unwrap()
            ),
            Err(PathSecurityError::WorkspaceEscape)
        ));
        for relative in ["dangling", "dangling/deleted/file.txt"] {
            assert!(
                resolver
                    .resolve_scoped_path(&WorkspaceRelativePath::parse(relative).unwrap())
                    .is_err()
            );
        }
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn scoped_path_preserves_safe_internal_directory_symlinks() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("target")).unwrap();
        if !symlink_directory(&root.path().join("target"), &root.path().join("alias")) {
            return;
        }
        let resolver = WorkspacePathResolver::new(root.path().to_owned()).unwrap();
        assert_eq!(
            resolver
                .resolve_scoped_path(
                    &WorkspaceRelativePath::parse("alias/deleted/file.txt").unwrap()
                )
                .unwrap(),
            resolver.root().join("target/deleted/file.txt")
        );
    }
}
