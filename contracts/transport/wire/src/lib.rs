//! ClientとHostが共用する接続識別子。
pub const REMOTE_WEBSOCKET_PATH: &str = "/remote";
pub const REMOTE_WEBSOCKET_PROTOCOL: &str = "lapis.v0";
pub const REMOTE_QUIC_PROTOCOL: &[u8] = b"lapis.v0";

/// Hostが待受開始後に書き出す接続情報の版。
pub const HOST_CONNECTION_VERSION: u32 = 1;

/// 接続情報として受け付ける上限。公開証明書のDERでも数KBに収まる。
#[cfg(not(target_arch = "wasm32"))]
const MAX_CONNECTION_BYTES: usize = 64 * 1024;

/// Hostが待受開始後に書き出す非機密の接続情報。tokenや秘密鍵を含めない。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostConnection {
    pub version: u32,
    pub address: std::net::SocketAddr,
    pub server_name: String,
    pub workspace_id: String,
    /// 検証に使う公開証明書のDER。
    pub certificate: Vec<u8>,
}

/// 一時ファイルへ書いてから置き換える。既存ファイルは上書きする。
#[cfg(not(target_arch = "wasm32"))]
pub fn write_connection(
    path: &std::path::Path,
    connection: &HostConnection,
) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(connection)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    if bytes.len() > MAX_CONNECTION_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "接続情報が大きすぎます",
        ));
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("connection.json");
    let temporary = path.with_file_name(format!("{name}.{}.tmp", std::process::id()));
    std::fs::write(&temporary, &bytes)?;
    match std::fs::rename(&temporary, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = std::fs::remove_file(&temporary);
            Err(error)
        }
    }
}

/// 版と必須項目を検証して読む。破損・版不一致はエラー。
#[cfg(not(target_arch = "wasm32"))]
pub fn read_connection(path: &std::path::Path) -> Result<HostConnection, String> {
    let bytes = std::fs::read(path)
        .map_err(|error| format!("接続情報を読み込めません ({}): {error}", path.display()))?;
    if bytes.len() > MAX_CONNECTION_BYTES {
        return Err(format!("接続情報が大きすぎます ({})", path.display()));
    }
    let connection: HostConnection = serde_json::from_slice(&bytes)
        .map_err(|error| format!("接続情報が不正です ({}): {error}", path.display()))?;
    if connection.version != HOST_CONNECTION_VERSION {
        return Err(format!(
            "接続情報の版が一致しません ({}): {}",
            path.display(),
            connection.version
        ));
    }
    if connection.server_name.is_empty() || connection.server_name.len() > 253 {
        return Err(format!(
            "接続情報のサーバー名が不正です ({})",
            path.display()
        ));
    }
    if connection.workspace_id.is_empty() {
        return Err(format!(
            "接続情報のWorkspace IDが空です ({})",
            path.display()
        ));
    }
    if connection.certificate.is_empty() || connection.certificate.len() > MAX_CONNECTION_BYTES {
        return Err(format!("接続情報の証明書が不正です ({})", path.display()));
    }
    Ok(connection)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    fn sample() -> HostConnection {
        HostConnection {
            version: HOST_CONNECTION_VERSION,
            address: "127.0.0.1:54321".parse().unwrap(),
            server_name: "lapis-host".to_owned(),
            workspace_id: "workspace-1".to_owned(),
            certificate: vec![48, 130, 1, 2],
        }
    }

    #[test]
    fn connection_round_trips() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("connection.json");
        let connection = sample();
        write_connection(&path, &connection).unwrap();

        let read = read_connection(&path).unwrap();
        assert_eq!(read.version, connection.version);
        assert_eq!(read.address, connection.address);
        assert_eq!(read.server_name, connection.server_name);
        assert_eq!(read.workspace_id, connection.workspace_id);
        assert_eq!(read.certificate, connection.certificate);
        assert!(!directory.path().join("connection.json.0.tmp").exists());
    }

    #[test]
    fn connection_overwrites_existing_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("connection.json");
        write_connection(&path, &sample()).unwrap();

        let mut updated = sample();
        updated.address = "127.0.0.1:12345".parse().unwrap();
        write_connection(&path, &updated).unwrap();

        assert_eq!(read_connection(&path).unwrap().address, updated.address);
    }

    #[test]
    fn unknown_field_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("connection.json");
        std::fs::write(
            &path,
            r#"{"version":1,"address":"127.0.0.1:1","server_name":"lapis-host","workspace_id":"workspace-1","certificate":[1],"token":[1,2,3]}"#,
        )
        .unwrap();

        let error = read_connection(&path).unwrap_err();
        assert!(error.contains("connection.json"), "{error}");
    }

    #[test]
    fn mismatched_version_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("connection.json");
        let mut value = serde_json::to_value(sample()).unwrap();
        value["version"] = serde_json::json!(HOST_CONNECTION_VERSION + 1);
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();

        let error = read_connection(&path).unwrap_err();
        assert!(error.contains("版"), "{error}");
    }

    #[test]
    fn broken_json_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("connection.json");
        std::fs::write(&path, b"{not json").unwrap();

        let error = read_connection(&path).unwrap_err();
        assert!(error.contains("不正"), "{error}");
    }
}
