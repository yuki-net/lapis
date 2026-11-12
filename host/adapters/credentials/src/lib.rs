//! Host が発行した credential を SQLite に保存する。

use std::{error::Error, fmt, path::Path};

use backend_state::{DeviceStatus, StoredCredential, StoredDevice};
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
                    workspace_id TEXT,
                    capabilities TEXT NOT NULL,
                    digest TEXT NOT NULL,
                    expires_at INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS devices (
                    id TEXT PRIMARY KEY NOT NULL,
                    client_id TEXT NOT NULL UNIQUE,
                    name TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    status TEXT NOT NULL,
                    created_at INTEGER NOT NULL,
                    last_seen_at INTEGER NOT NULL,
                    approved_at INTEGER,
                    revoked_at INTEGER
                );",
            )
            .map_err(CredentialStoreError::Sqlite)?;
        self.migrate_nullable_workspace()
    }

    /// 旧 schema の `workspace_id NOT NULL` を、固定先を持たない credential 向けに緩める。
    fn migrate_nullable_workspace(&self) -> Result<(), CredentialStoreError> {
        let mut statement = self
            .connection
            .prepare("PRAGMA table_info(credentials)")
            .map_err(CredentialStoreError::Sqlite)?;
        let mut rows = statement.query([]).map_err(CredentialStoreError::Sqlite)?;
        let mut rebuild = false;
        while let Some(row) = rows.next().map_err(CredentialStoreError::Sqlite)? {
            let name: String = row.get(1).map_err(CredentialStoreError::Sqlite)?;
            let not_null: i64 = row.get(3).map_err(CredentialStoreError::Sqlite)?;
            if name == "workspace_id" && not_null != 0 {
                rebuild = true;
            }
        }
        drop(rows);
        drop(statement);
        if !rebuild {
            return Ok(());
        }
        let result = self.connection.execute_batch(
            "BEGIN IMMEDIATE;
                 ALTER TABLE credentials RENAME TO credentials_legacy;
                 CREATE TABLE credentials (
                    id TEXT PRIMARY KEY NOT NULL,
                    client_id TEXT NOT NULL,
                    workspace_id TEXT,
                    capabilities TEXT NOT NULL,
                    digest TEXT NOT NULL,
                    expires_at INTEGER NOT NULL
                 );
                 INSERT INTO credentials
                    (id, client_id, workspace_id, capabilities, digest, expires_at)
                 SELECT id, client_id, workspace_id, capabilities, digest, expires_at
                 FROM credentials_legacy;
                 DROP TABLE credentials_legacy;
                 COMMIT;",
        );
        if result.is_err() {
            let _ = self.connection.execute_batch("ROLLBACK;");
        }
        result.map_err(CredentialStoreError::Sqlite)
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
                    row.get::<_, Option<String>>(2)?,
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

    /// 保存済み device をすべて読み込む。
    pub fn load_devices(&self) -> Result<Vec<StoredDevice>, CredentialStoreError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, client_id, name, kind, status, created_at, last_seen_at,
                        approved_at, revoked_at
                 FROM devices ORDER BY id",
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
                    row.get::<_, i64>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                    row.get::<_, Option<i64>>(8)?,
                ))
            })
            .map_err(CredentialStoreError::Sqlite)?;

        let mut devices = Vec::new();
        for row in rows {
            let (
                id,
                client_id,
                name,
                kind,
                status,
                created_at,
                last_seen_at,
                approved_at,
                revoked_at,
            ) = row.map_err(CredentialStoreError::Sqlite)?;
            devices.push(StoredDevice {
                id,
                client_id,
                name,
                kind,
                status: parse_status(&status)?,
                created_at: created_at.max(0) as u64,
                last_seen_at: last_seen_at.max(0) as u64,
                approved_at: approved_at.map(|value| value.max(0) as u64),
                revoked_at: revoked_at.map(|value| value.max(0) as u64),
            });
        }
        Ok(devices)
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
        write_credentials(&transaction, records)?;
        transaction.commit().map_err(CredentialStoreError::Sqlite)
    }

    /// 保存済み device をすべて置き換える。
    pub fn replace_devices(
        &mut self,
        devices: &[StoredDevice],
    ) -> Result<(), CredentialStoreError> {
        let transaction = self
            .connection
            .transaction()
            .map_err(CredentialStoreError::Sqlite)?;
        transaction
            .execute("DELETE FROM devices", [])
            .map_err(CredentialStoreError::Sqlite)?;
        write_devices(&transaction, devices)?;
        transaction.commit().map_err(CredentialStoreError::Sqlite)
    }

    /// credential と device を同じ transaction で置き換える。
    pub fn replace_auth(
        &mut self,
        credentials: &[StoredCredential],
        devices: &[StoredDevice],
    ) -> Result<(), CredentialStoreError> {
        let transaction = self
            .connection
            .transaction()
            .map_err(CredentialStoreError::Sqlite)?;
        transaction
            .execute("DELETE FROM credentials", [])
            .map_err(CredentialStoreError::Sqlite)?;
        transaction
            .execute("DELETE FROM devices", [])
            .map_err(CredentialStoreError::Sqlite)?;
        write_credentials(&transaction, credentials)?;
        write_devices(&transaction, devices)?;
        transaction.commit().map_err(CredentialStoreError::Sqlite)
    }
}

fn write_credentials(
    transaction: &rusqlite::Transaction<'_>,
    records: &[StoredCredential],
) -> Result<(), CredentialStoreError> {
    let mut insert = transaction
        .prepare(
            "INSERT INTO credentials
             (id, client_id, workspace_id, capabilities, digest, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .map_err(CredentialStoreError::Sqlite)?;
    for record in records {
        let capabilities =
            serde_json::to_string(&record.capabilities).map_err(CredentialStoreError::Json)?;
        insert
            .execute(params![
                &record.id,
                &record.client_id,
                record.workspace_id.as_deref(),
                capabilities,
                &record.digest,
                record.expires_at.min(i64::MAX as u64) as i64,
            ])
            .map_err(CredentialStoreError::Sqlite)?;
    }
    Ok(())
}

fn write_devices(
    transaction: &rusqlite::Transaction<'_>,
    devices: &[StoredDevice],
) -> Result<(), CredentialStoreError> {
    let mut insert = transaction
        .prepare(
            "INSERT INTO devices
             (id, client_id, name, kind, status, created_at, last_seen_at, approved_at, revoked_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )
        .map_err(CredentialStoreError::Sqlite)?;
    for device in devices {
        insert
            .execute(params![
                &device.id,
                &device.client_id,
                &device.name,
                &device.kind,
                status_text(device.status),
                device.created_at as i64,
                device.last_seen_at as i64,
                device.approved_at.map(|value| value as i64),
                device.revoked_at.map(|value| value as i64),
            ])
            .map_err(CredentialStoreError::Sqlite)?;
    }
    Ok(())
}

fn status_text(status: DeviceStatus) -> &'static str {
    match status {
        DeviceStatus::Pending => "pending",
        DeviceStatus::Approved => "approved",
        DeviceStatus::Revoked => "revoked",
    }
}

fn parse_status(value: &str) -> Result<DeviceStatus, CredentialStoreError> {
    match value {
        "pending" => Ok(DeviceStatus::Pending),
        "approved" => Ok(DeviceStatus::Approved),
        "revoked" => Ok(DeviceStatus::Revoked),
        _ => Err(CredentialStoreError::InvalidData(format!(
            "unknown device status: {value}"
        ))),
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
    InvalidData(String),
}

impl fmt::Display for CredentialStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(error) => write!(formatter, "credential database error: {error}"),
            Self::Json(error) => write!(formatter, "credential JSON error: {error}"),
            Self::Io(error) => write!(formatter, "credential file error: {error}"),
            Self::InvalidData(message) => write!(formatter, "credential data error: {message}"),
        }
    }
}

impl Error for CredentialStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Sqlite(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::InvalidData(_) => None,
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
            workspace_id: Some("workspace-1".to_owned()),
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
    fn distant_expiration_does_not_wrap_into_an_expired_value() {
        let mut store = CredentialStore::open_in_memory().unwrap();
        let mut credential = record("long-lived");
        credential.expires_at = u64::MAX;
        store.replace_all(&[credential]).unwrap();
        assert_eq!(store.load().unwrap()[0].expires_at, i64::MAX as u64);
    }

    #[test]
    fn switchable_credentials_round_trip_without_a_workspace() {
        let mut store = CredentialStore::open_in_memory().unwrap();
        let mut switchable = record("switchable");
        switchable.workspace_id = None;
        store.replace_all(&[switchable.clone()]).unwrap();

        assert_eq!(store.load().unwrap(), vec![switchable]);
    }

    #[test]
    fn legacy_not_null_workspace_column_is_migrated() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("credentials.sqlite3");
        {
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE credentials (
                        id TEXT PRIMARY KEY NOT NULL,
                        client_id TEXT NOT NULL,
                        workspace_id TEXT NOT NULL,
                        capabilities TEXT NOT NULL,
                        digest TEXT NOT NULL,
                        expires_at INTEGER NOT NULL
                    );",
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO credentials VALUES ('legacy', 'client-1', 'workspace-1', '[]', 'digest', 42)",
                    [],
                )
                .unwrap();
        }

        let mut store = CredentialStore::open(&path).unwrap();
        let mut switchable = record("switchable");
        switchable.workspace_id = None;
        store.replace_all(&[switchable.clone()]).unwrap();

        assert_eq!(store.load().unwrap(), vec![switchable]);
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

    fn device(id: &str, client_id: &str, status: DeviceStatus) -> StoredDevice {
        StoredDevice {
            id: id.to_owned(),
            client_id: client_id.to_owned(),
            name: "Lapis Web".to_owned(),
            kind: "web".to_owned(),
            status,
            created_at: 10,
            last_seen_at: 20,
            approved_at: (status == DeviceStatus::Approved).then_some(30),
            revoked_at: (status == DeviceStatus::Revoked).then_some(40),
        }
    }

    #[test]
    fn devices_round_trip_through_sqlite() {
        let mut store = CredentialStore::open_in_memory().unwrap();
        let devices = [
            device("device-a", "client-a", DeviceStatus::Pending),
            device("device-b", "client-b", DeviceStatus::Approved),
            device("device-c", "client-c", DeviceStatus::Revoked),
        ];
        store.replace_devices(&devices).unwrap();

        assert_eq!(store.load_devices().unwrap(), devices);
    }

    #[test]
    fn replace_auth_replaces_credentials_and_devices_together() {
        let mut store = CredentialStore::open_in_memory().unwrap();
        store
            .replace_devices(&[device("old", "old", DeviceStatus::Pending)])
            .unwrap();
        store
            .replace_auth(
                &[record("credential")],
                &[device("new", "new", DeviceStatus::Approved)],
            )
            .unwrap();

        assert_eq!(store.load().unwrap(), vec![record("credential")]);
        assert_eq!(
            store.load_devices().unwrap(),
            vec![device("new", "new", DeviceStatus::Approved)]
        );
    }

    #[test]
    fn devices_persist_across_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("credentials.sqlite3");
        let expected = device("persisted", "client", DeviceStatus::Approved);
        {
            let mut store = CredentialStore::open(&path).unwrap();
            store
                .replace_devices(std::slice::from_ref(&expected))
                .unwrap();
        }

        let store = CredentialStore::open(&path).unwrap();

        assert_eq!(store.load_devices().unwrap(), vec![expected]);
    }
}
