//! Host が発行した credential を SQLite に保存する。

use std::{error::Error, fmt, path::Path};

use backend_state::StoredCredential;
use rusqlite::{Connection, params};

/// SQLite の credential store。
pub struct CredentialStore {
    connection: Connection,
}

impl CredentialStore {
    /// 指定したパスの SQLite を開き、必要なテーブルを作成する。
    pub fn open(path: impl AsRef<Path>) -> Result<Self, CredentialStoreError> {
        let path = path.as_ref();
        let connection = Connection::open(path).map_err(CredentialStoreError::Sqlite)?;
        restrict_permissions(path)?;
        Self::from_connection(connection)
    }

    /// テスト用のインメモリ SQLite を開く。
    pub fn open_in_memory() -> Result<Self, CredentialStoreError> {
        let connection = Connection::open_in_memory().map_err(CredentialStoreError::Sqlite)?;
        Self::from_connection(connection)
    }

    fn from_connection(connection: Connection) -> Result<Self, CredentialStoreError> {
        let store = Self { connection };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<(), CredentialStoreError> {
        self.connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS credentials (
                    id TEXT PRIMARY KEY NOT NULL,
                    client_id TEXT NOT NULL,
                    workspace_id TEXT NOT NULL,
                    capabilities TEXT NOT NULL,
                    digest TEXT NOT NULL,
                    expires_at INTEGER NOT NULL
                );",
            )
            .map_err(CredentialStoreError::Sqlite)
    }

    /// 保存済み credential をすべて読み込む。
    pub fn load(&self) -> Result<Vec<StoredCredential>, CredentialStoreError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, client_id, workspace_id, capabilities, digest, expires_at
                 FROM credentials ORDER BY id",
            )
            .map_err(CredentialStoreError::Sqlite)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })
            .map_err(CredentialStoreError::Sqlite)?;

        let mut records = Vec::new();
        for row in rows {
            let (id, client_id, workspace_id, capabilities, digest, expires_at) =
                row.map_err(CredentialStoreError::Sqlite)?;
            let capabilities =
                serde_json::from_str(&capabilities).map_err(CredentialStoreError::Json)?;
            records.push(StoredCredential {
                id,
                client_id,
                workspace_id,
                capabilities,
                digest,
                expires_at: expires_at.max(0) as u64,
            });
        }
        Ok(records)
    }

    /// 保存済み credential をすべて置き換える。
    pub fn replace_all(
        &mut self,
        records: &[StoredCredential],
    ) -> Result<(), CredentialStoreError> {
        let transaction = self
            .connection
            .transaction()
            .map_err(CredentialStoreError::Sqlite)?;
        transaction
            .execute("DELETE FROM credentials", [])
            .map_err(CredentialStoreError::Sqlite)?;
        {
            let mut insert = transaction
                .prepare(
                    "INSERT INTO credentials
                     (id, client_id, workspace_id, capabilities, digest, expires_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                )
                .map_err(CredentialStoreError::Sqlite)?;
            for record in records {
                let capabilities = serde_json::to_string(&record.capabilities)
                    .map_err(CredentialStoreError::Json)?;
                insert
                    .execute(params![
                        &record.id,
                        &record.client_id,
                        &record.workspace_id,
                        capabilities,
                        &record.digest,
                        record.expires_at as i64,
                    ])
                    .map_err(CredentialStoreError::Sqlite)?;
            }
        }
        transaction.commit().map_err(CredentialStoreError::Sqlite)
    }
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) -> Result<(), CredentialStoreError> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(CredentialStoreError::Io)
}

#[cfg(not(unix))]
fn restrict_permissions(_: &Path) -> Result<(), CredentialStoreError> {
    Ok(())
}

#[derive(Debug)]
pub enum CredentialStoreError {
    Sqlite(rusqlite::Error),
    Json(serde_json::Error),
    Io(std::io::Error),
}

impl fmt::Display for CredentialStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(error) => write!(formatter, "credential database error: {error}"),
            Self::Json(error) => write!(formatter, "credential JSON error: {error}"),
            Self::Io(error) => write!(formatter, "credential file error: {error}"),
        }
    }
}

impl Error for CredentialStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Sqlite(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::Io(error) => Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str) -> StoredCredential {
        StoredCredential {
            id: id.to_owned(),
            client_id: "client-1".to_owned(),
            workspace_id: "workspace-1".to_owned(),
            capabilities: vec!["workspace.sync".to_owned()],
            digest: "digest".to_owned(),
            expires_at: 42,
        }
    }

    #[test]
    fn credentials_round_trip_through_sqlite() {
        let mut store = CredentialStore::open_in_memory().unwrap();
        store.replace_all(&[record("a"), record("b")]).unwrap();

        let loaded = store.load().unwrap();

        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0], record("a"));
        assert_eq!(loaded[1], record("b"));
    }

    #[test]
    fn replace_all_removes_missing_credentials() {
        let mut store = CredentialStore::open_in_memory().unwrap();
        store.replace_all(&[record("a"), record("b")]).unwrap();
        store.replace_all(&[record("b")]).unwrap();

        let loaded = store.load().unwrap();

        assert_eq!(loaded, vec![record("b")]);
    }

    #[cfg(unix)]
    #[test]
    fn credentials_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("credentials.sqlite3");

        let _store = CredentialStore::open(&path).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn credentials_persist_across_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("credentials.sqlite3");
        {
            let mut store = CredentialStore::open(&path).unwrap();
            store.replace_all(&[record("persisted")]).unwrap();
        }

        let store = CredentialStore::open(&path).unwrap();

        assert_eq!(store.load().unwrap(), vec![record("persisted")]);
    }
}
