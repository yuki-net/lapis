//! Workspace を走査する検索 backend。

use std::{
    fs,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

use workspace::{SearchError, SearchHit, WorkspaceSearchBackend};

const MAX_HITS: usize = 2_000;
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Default)]
pub struct LocalWorkspaceSearchBackend;

impl WorkspaceSearchBackend for LocalWorkspaceSearchBackend {
    fn search(
        &self,
        root: &Path,
        query: &str,
        cancelled: &AtomicBool,
    ) -> Result<Vec<SearchHit>, SearchError> {
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let mut hits = Vec::new();
        search_directory(root, root, query, cancelled, &mut hits)?;
        Ok(hits)
    }
}

fn search_directory(
    root: &Path,
    directory: &Path,
    query: &str,
    cancelled: &AtomicBool,
    hits: &mut Vec<SearchHit>,
) -> Result<(), SearchError> {
    if cancelled.load(Ordering::Relaxed) || hits.len() >= MAX_HITS {
        return Ok(());
    }
    let entries = fs::read_dir(directory).map_err(|error| SearchError::new(error.to_string()))?;
    for entry in entries.flatten() {
        if cancelled.load(Ordering::Relaxed) || hits.len() >= MAX_HITS {
            break;
        }
        let name = entry.file_name();
        if matches!(name.to_str(), Some(".git" | "target" | ".DS_Store")) {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            search_directory(root, &path, query, cancelled, hits)?;
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.len() > MAX_FILE_BYTES {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let Ok(text) = std::str::from_utf8(&bytes) else {
            continue;
        };
        for (line_index, line) in text.lines().enumerate() {
            for (column, _) in line.match_indices(query) {
                hits.push(SearchHit {
                    path: path.strip_prefix(root).unwrap_or(&path).to_owned(),
                    line: line_index + 1,
                    utf8_column: column + 1,
                    preview: line.trim().chars().take(240).collect(),
                });
                if hits.len() >= MAX_HITS {
                    return Ok(());
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_finds_utf8_text_and_skips_build_output() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir_all(directory.path().join("src")).unwrap();
        fs::create_dir_all(directory.path().join("target")).unwrap();
        fs::write(
            directory.path().join("src/main.rs"),
            "fn main() { /* 検索対象 */ }\n",
        )
        .unwrap();
        fs::write(directory.path().join("target/generated.rs"), "検索対象\n").unwrap();

        let hits = LocalWorkspaceSearchBackend
            .search(directory.path(), "検索対象", &AtomicBool::new(false))
            .unwrap();

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, Path::new("src/main.rs"));
        assert_eq!(hits[0].line, 1);
    }

    #[test]
    fn empty_query_and_cancelled_search_return_no_hits() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("a.txt"), "needle\n").unwrap();

        assert!(
            LocalWorkspaceSearchBackend
                .search(directory.path(), "", &AtomicBool::new(false))
                .unwrap()
                .is_empty()
        );
        assert!(
            LocalWorkspaceSearchBackend
                .search(directory.path(), "needle", &AtomicBool::new(true))
                .unwrap()
                .is_empty()
        );
    }
}
