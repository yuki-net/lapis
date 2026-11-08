//! Host の同一性と Workspace の登録を SQLite に保存する。
//!
//! Workspace ID は Host が発行し、パスは一意制約にだけ使う。パスのハッシュを ID に
//! すると移動や改名で状態が孤立するため、登録時の ID を永続化する。

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};

const VERSION: i64 = 1;

/// 保存された Workspace の 1 件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceRecord {
    pub id: String,
    pub root: PathBuf,
    pub name: String,
    pub primary: bool,
}

pub struct WorkspaceStore {
    connection: Connection,
}

impl WorkspaceStore {
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let connection = Connection::open(path)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        let store = Self { connection };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self, StoreError> {
        let store = Self {
            connection: Connection::open_in_memory()?,
        };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<(), StoreError> {
        self.connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS metadata (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS workspaces (
                id TEXT PRIMARY KEY,
                root BLOB NOT NULL UNIQUE,
                name TEXT NOT NULL,
                primary_flag INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL
             );",
        )?;
        let version: Option<String> = self
            .connection
            .query_row(
                "SELECT value FROM metadata WHERE key = 'version'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        match version.as_deref() {
            Some(value) if value == VERSION.to_string() => Ok(()),
            Some(_) => Err(StoreError::UnsupportedVersion),
            None => {
                self.connection.execute(
                    "INSERT INTO metadata (key, value) VALUES ('version', ?1)",
                    params![VERSION.to_string()],
                )?;
                Ok(())
            }
        }
    }

    /// Host の同一性。初回に生成して以後は同じ値を返す。
    pub fn host_id(&self) -> Result<String, StoreError> {
        if let Some(value) = self
            .connection
            .query_row(
                "SELECT value FROM metadata WHERE key = 'host_id'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        {
            return Ok(value);
        }
        let value = random_id("host")?;
        self.connection.execute(
            "INSERT INTO metadata (key, value) VALUES ('host_id', ?1)",
            params![value],
        )?;
        Ok(value)
    }

    pub fn list(&self) -> Result<Vec<WorkspaceRecord>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, root, name, primary_flag FROM workspaces ORDER BY created_at, id",
        )?;
        let rows = statement.query_map([], |row| {
            let id: String = row.get(0)?;
            let root: Vec<u8> = row.get(1)?;
            let name: String = row.get(2)?;
            let primary: bool = row.get::<_, i64>(3)? != 0;
            Ok((id, root, name, primary))
        })?;
        let mut records = Vec::new();
        for row in rows {
            let (id, root, name, primary) = row?;
            records.push(WorkspaceRecord {
                id,
                root: decode_path(&root)?,
                name,
                primary,
            });
        }
        Ok(records)
    }

    /// パスから既存の登録を返す。無ければ新しい ID を発行して登録する。
    pub fn register(&self, root: &Path) -> Result<WorkspaceRecord, StoreError> {
        let encoded = encode_path(root);
        if let Some(record) = self.find_by_root(&encoded)? {
            return Ok(record);
        }
        let id = random_id("workspace")?;
        let name = root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Workspace")
            .to_owned();
        let primary = self.list()?.is_empty();
        self.connection.execute(
            "INSERT INTO workspaces (id, root, name, primary_flag, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, encoded, name, i64::from(primary), now_unix_seconds()?],
        )?;
        Ok(WorkspaceRecord {
            id,
            root: root.to_owned(),
            name,
            primary,
        })
    }

    /// 登録を削除する。ファイルは消さない。
    pub fn unregister(&self, id: &str) -> Result<(), StoreError> {
        self.connection
            .execute("DELETE FROM workspaces WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// 既定の Workspace。primary が無ければ ID 順の先頭。
    pub fn primary(&self) -> Result<Option<WorkspaceRecord>, StoreError> {
        let records = self.list()?;
        Ok(records
            .iter()
            .find(|record| record.primary)
            .or_else(|| records.first())
            .cloned())
    }

    fn find_by_root(&self, encoded: &[u8]) -> Result<Option<WorkspaceRecord>, StoreError> {
        let row = self
            .connection
            .query_row(
                "SELECT id, root, name, primary_flag FROM workspaces WHERE root = ?1",
                params![encoded],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)? != 0,
                    ))
                },
            )
            .optional()?;
        row.map(|(id, root, name, primary)| {
            Ok(WorkspaceRecord {
                id,
                root: decode_path(&root)?,
                name,
                primary,
            })
        })
        .transpose()
    }
}

#[derive(Debug)]
pub enum StoreError {
    Sqlite(rusqlite::Error),
    Io(std::io::Error),
    UnsupportedVersion,
    InvalidRecord,
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sqlite(error) => write!(formatter, "workspace store: {error}"),
            Self::Io(error) => write!(formatter, "workspace store io: {error}"),
            Self::UnsupportedVersion => {
                formatter.write_str("workspace store version is unsupported")
            }
            Self::InvalidRecord => formatter.write_str("workspace store record is invalid"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

impl From<std::io::Error> for StoreError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

fn now_unix_seconds() -> Result<i64, StoreError> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| StoreError::InvalidRecord)?
        .as_secs() as i64)
}

/// 推測できない識別子を作る。暗号学的乱数を使い、衝突時は呼出側の一意制約で検出する。
fn random_id(prefix: &str) -> Result<String, StoreError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| StoreError::InvalidRecord)?;
    let hex = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!("{prefix}-{hex}"))
}

#[cfg(windows)]
fn encode_path(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .flat_map(|unit| unit.to_le_bytes())
        .collect()
}

#[cfg(unix)]
fn encode_path(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(windows)]
fn decode_path(bytes: &[u8]) -> Result<PathBuf, StoreError> {
    use std::os::windows::ffi::OsStringExt;
    if bytes.len() % 2 != 0 {
        return Err(StoreError::InvalidRecord);
    }
    let units = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    Ok(PathBuf::from(std::ffi::OsString::from_wide(&units)))
}

#[cfg(unix)]
fn decode_path(bytes: &[u8]) -> Result<PathBuf, StoreError> {
    use std::os::unix::ffi::OsStringExt;
    Ok(PathBuf::from(std::ffi::OsString::from_vec(bytes.to_vec())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_issued_once_per_root_and_survive_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("workspaces.sqlite3");
        let root = directory.path().join("workspace");
        std::fs::create_dir_all(&root).unwrap();
        let id = {
            let store = WorkspaceStore::open(&path).unwrap();
            let record = store.register(&root).unwrap();
            assert!(record.id.starts_with("workspace-"));
            assert!(record.primary);
            record.id
        };
        let store = WorkspaceStore::open(&path).unwrap();
        let record = store.register(&root).unwrap();
        assert_eq!(record.id, id);
        assert_eq!(store.list().unwrap().len(), 1);
        assert_eq!(store.primary().unwrap().unwrap().id, id);
    }

    #[test]
    fn manifest_no_longer_determines_the_id() {
        let directory = tempfile::tempdir().unwrap();
        let store = WorkspaceStore::open_in_memory().unwrap();
        let first = directory.path().join("a");
        let second = directory.path().join("b");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        let first_id = store.register(&first).unwrap().id;
        let second_id = store.register(&second).unwrap().id;
        assert_ne!(first_id, second_id);
        assert!(!first_id.starts_with(&format!("workspace-{}", "0".repeat(64))));
    }

    #[test]
    fn host_id_is_stable_and_distinct_per_store() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("workspaces.sqlite3");
        let first = {
            let store = WorkspaceStore::open(&path).unwrap();
            store.host_id().unwrap()
        };
        let store = WorkspaceStore::open(&path).unwrap();
        assert_eq!(store.host_id().unwrap(), first);

        let other = WorkspaceStore::open_in_memory().unwrap();
        assert_ne!(other.host_id().unwrap(), first);
    }

    #[test]
    fn unregister_only_removes_the_record() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("workspace");
        std::fs::create_dir_all(&root).unwrap();
        let store = WorkspaceStore::open_in_memory().unwrap();
        let record = store.register(&root).unwrap();
        store.unregister(&record.id).unwrap();
        assert!(store.list().unwrap().is_empty());
        assert!(root.is_dir());
    }
}
