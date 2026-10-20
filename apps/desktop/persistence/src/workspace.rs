use std::{
    env, fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use lapis_desktop_client::{DocumentViewState, WorkspaceSnapshot, WorkspaceStateRepository};
use lapis_editor_core::DocumentId;
use lapis_workspace::WorkspaceError;

pub struct LocalWorkspaceStateRepository {
    path: PathBuf,
}

impl LocalWorkspaceStateRepository {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn user_default() -> Self {
        let base = env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(env::temp_dir);
        Self::new(base.join("Lapis").join("session-v1.txt"))
    }
}

impl WorkspaceStateRepository for LocalWorkspaceStateRepository {
    fn load(&self) -> Result<Option<WorkspaceSnapshot>, WorkspaceError> {
        let mut input = match fs::File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(WorkspaceError::new(error.to_string())),
        };
        let mut text = String::new();
        input
            .read_to_string(&mut text)
            .map_err(|error| WorkspaceError::new(error.to_string()))?;
        parse_snapshot(&text).map(Some)
    }

    fn save(&self, snapshot: &WorkspaceSnapshot) -> Result<(), WorkspaceError> {
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent).map_err(|error| WorkspaceError::new(error.to_string()))?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)
            .map_err(|error| WorkspaceError::new(error.to_string()))?;
        temporary
            .write_all(format_snapshot(snapshot).as_bytes())
            .and_then(|()| temporary.as_file().sync_all())
            .map_err(|error| WorkspaceError::new(error.to_string()))?;
        temporary
            .persist(&self.path)
            .map_err(|error| WorkspaceError::new(error.error.to_string()))?;
        Ok(())
    }
}

fn format_snapshot(snapshot: &WorkspaceSnapshot) -> String {
    let mut lines = vec!["LAPIS_SESSION_V1".to_owned()];
    if let Some(root) = &snapshot.root {
        lines.push(format!(
            "root\t{}",
            encode_bytes(root.to_string_lossy().as_bytes())
        ));
    }
    if let Some(active) = &snapshot.active_path {
        lines.push(format!(
            "active\t{}",
            encode_bytes(active.to_string_lossy().as_bytes())
        ));
    }
    if let Some(id) = &snapshot.active_document_id {
        lines.push(format!(
            "active_id\t{}",
            encode_bytes(id.as_str().as_bytes())
        ));
    }
    for view in &snapshot.open_documents {
        let mut line = format!(
            "doc\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            encode_bytes(view.path.to_string_lossy().as_bytes()),
            view.cursor_char,
            view.selection_start,
            view.selection_end,
            view.scroll_x,
            view.scroll_y,
            view.draft_content
                .as_deref()
                .map(|draft| encode_bytes(draft.as_bytes()))
                .unwrap_or_else(|| "-".to_owned())
        );
        if let Some(id) = &view.document_id {
            line.push('\t');
            line.push_str(&encode_bytes(id.as_str().as_bytes()));
        }
        lines.push(line);
    }
    lines.push(String::new());
    lines.join("\n")
}

fn parse_snapshot(text: &str) -> Result<WorkspaceSnapshot, WorkspaceError> {
    let mut lines = text.lines();
    if lines.next() != Some("LAPIS_SESSION_V1") {
        return Err(WorkspaceError::new("未対応のWorkspace復元形式です"));
    }
    let mut snapshot = WorkspaceSnapshot::default();
    for line in lines {
        let fields = line.split('\t').collect::<Vec<_>>();
        match fields.as_slice() {
            ["root", value] => snapshot.root = Some(decode_path(value)?),
            ["active", value] => snapshot.active_path = Some(decode_path(value)?),
            ["active_id", value] => snapshot.active_document_id = Some(decode_document_id(value)?),
            [
                "doc",
                path,
                cursor,
                start,
                end,
                scroll_x,
                scroll_y,
                draft,
                identity @ ..,
            ] if identity.len() <= 1 => {
                snapshot.open_documents.push(DocumentViewState {
                    document_id: identity
                        .first()
                        .map(|value| decode_document_id(value))
                        .transpose()?,
                    path: decode_path(path)?,
                    cursor_char: parse_number(cursor, "cursor")?,
                    selection_start: parse_number(start, "selection start")?,
                    selection_end: parse_number(end, "selection end")?,
                    scroll_x: parse_number(scroll_x, "scroll x")?,
                    scroll_y: parse_number(scroll_y, "scroll y")?,
                    draft_content: if *draft == "-" {
                        None
                    } else {
                        Some(
                            String::from_utf8(decode_bytes(draft)?)
                                .map_err(|error| WorkspaceError::new(error.to_string()))?,
                        )
                    },
                });
            }
            [] | [""] => {}
            _ => return Err(WorkspaceError::new("Workspace復元データが壊れています")),
        }
    }
    Ok(snapshot)
}

fn parse_number<T: std::str::FromStr>(value: &str, label: &str) -> Result<T, WorkspaceError> {
    value
        .parse()
        .map_err(|_| WorkspaceError::new(format!("{label}を復元できません")))
}

fn decode_document_id(value: &str) -> Result<DocumentId, WorkspaceError> {
    String::from_utf8(decode_bytes(value)?)
        .map(DocumentId::new)
        .map_err(|error| WorkspaceError::new(error.to_string()))
}

fn decode_path(value: &str) -> Result<PathBuf, WorkspaceError> {
    let bytes = decode_bytes(value)?;
    String::from_utf8(bytes)
        .map(PathBuf::from)
        .map_err(|error| WorkspaceError::new(error.to_string()))
}

fn encode_bytes(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn decode_bytes(value: &str) -> Result<Vec<u8>, WorkspaceError> {
    if !value.len().is_multiple_of(2) {
        return Err(WorkspaceError::new("16進復元データの長さが不正です"));
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = decode_hex(pair[0])?;
            let low = decode_hex(pair[1])?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn decode_hex(value: u8) -> Result<u8, WorkspaceError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(WorkspaceError::new("16進復元データが不正です")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_round_trip_preserves_unicode_draft_and_view() {
        let directory = tempfile::tempdir().unwrap();
        let state = LocalWorkspaceStateRepository::new(directory.path().join("session.txt"));
        let snapshot = WorkspaceSnapshot {
            root: Some(PathBuf::from("C:/作業/lapis")),
            active_path: Some(PathBuf::from("C:/作業/lapis/メモ.md")),
            active_document_id: None,
            open_documents: vec![DocumentViewState {
                path: PathBuf::from("C:/作業/lapis/メモ.md"),
                cursor_char: 4,
                selection_start: 1,
                selection_end: 4,
                scroll_x: 12.5,
                scroll_y: 33.0,
                draft_content: Some("日本😀\n".to_owned()),
                ..DocumentViewState::default()
            }],
        };
        state.save(&snapshot).unwrap();
        assert_eq!(state.load().unwrap(), Some(snapshot));
    }

    #[test]
    fn session_v1_format_remains_byte_compatible() {
        let snapshot = WorkspaceSnapshot {
            root: Some(PathBuf::from("repo")),
            active_path: Some(PathBuf::from("a.txt")),
            active_document_id: None,
            open_documents: vec![DocumentViewState {
                path: PathBuf::from("a.txt"),
                cursor_char: 4,
                selection_start: 1,
                selection_end: 4,
                scroll_x: 12.5,
                scroll_y: 33.0,
                draft_content: Some("hi\n".into()),
                ..DocumentViewState::default()
            }],
        };
        let stored = "LAPIS_SESSION_V1\nroot\t7265706f\nactive\t612e747874\ndoc\t612e747874\t4\t1\t4\t12.5\t33\t68690a\n";
        assert_eq!(format_snapshot(&snapshot), stored);
        assert_eq!(parse_snapshot(stored).unwrap(), snapshot);
    }

    #[test]
    fn session_v1_preserves_absent_and_empty_drafts() {
        let snapshot = WorkspaceSnapshot {
            open_documents: vec![
                DocumentViewState {
                    path: PathBuf::from("saved.txt"),
                    ..DocumentViewState::default()
                },
                DocumentViewState {
                    path: PathBuf::from("draft.txt"),
                    draft_content: Some(String::new()),
                    ..DocumentViewState::default()
                },
            ],
            ..WorkspaceSnapshot::default()
        };
        assert_eq!(
            parse_snapshot(&format_snapshot(&snapshot)).unwrap(),
            snapshot
        );
    }

    #[test]
    fn snapshot_preserves_active_untitled_identity_with_legacy_session_support() {
        let id = DocumentId::new("recovered-😀");
        let snapshot = WorkspaceSnapshot {
            active_document_id: Some(id.clone()),
            open_documents: vec![DocumentViewState {
                document_id: Some(id),
                draft_content: Some("recovered input".into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(
            parse_snapshot(&format_snapshot(&snapshot)).unwrap(),
            snapshot
        );
        let legacy = r#"{"root":null,"open_documents":[],"active_path":null}"#;
        let restored: WorkspaceSnapshot = serde_json::from_str(legacy).unwrap();
        assert_eq!(restored, WorkspaceSnapshot::default());
    }

    #[test]
    fn missing_session_is_not_an_error_and_malformed_sessions_are_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state/session.txt");
        let repository = LocalWorkspaceStateRepository::new(path.clone());
        assert_eq!(repository.load().unwrap(), None);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        for invalid in [
            "",
            "LAPIS_SESSION_V2\n",
            "LAPIS_SESSION_V1\nunknown\t00\n",
            "LAPIS_SESSION_V1\nroot\t0\n",
            "LAPIS_SESSION_V1\nroot\tzz\n",
            "LAPIS_SESSION_V1\nroot\tff\n",
            "LAPIS_SESSION_V1\nactive_id\tzz\n",
            "LAPIS_SESSION_V1\ndoc\t\t0\t0\t0\t0\t0\t-\tff\n",
            "LAPIS_SESSION_V1\ndoc\t61\tbad\t0\t0\t0\t0\t-\n",
        ] {
            std::fs::write(&path, invalid).unwrap();
            assert!(repository.load().is_err(), "拒否されなかった: {invalid:?}");
        }
    }
}
