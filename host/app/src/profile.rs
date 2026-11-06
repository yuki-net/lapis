use crate::{BackendClient, local::Record, protected::protect};
use client_api::*;
use remote::QuicConnectionConfig;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
};
use zeroize::{Zeroize, Zeroizing};
type Error = Box<dyn std::error::Error + Send + Sync>;

/// 利用者が明示して配布する一回限りの招待。Debugや資格情報の引数渡しは行わない。
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostInvitation {
    pub version: u32,
    pub address: SocketAddr,
    pub server_name: String,
    pub certificate: Vec<u8>,
    pub workspace_id: String,
    pub name: String,
    pub token: Vec<u8>,
}
impl Drop for HostInvitation {
    fn drop(&mut self) {
        self.token.zeroize();
    }
}

pub fn write_invitation(path: &Path, invitation: &HostInvitation) -> Result<(), Error> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(path)?;
    serde_json::to_writer(&mut file, invitation)?;
    file.flush()?;
    file.sync_all()?;
    Ok(())
}

/// 招待からの初回接続、または保護済みprofileからの再接続を行う。
/// 戻り値は接続port、Client内の仮想root、次回起動で使うprofileの保存先。
pub fn open_host_file(
    path: &Path,
    directory: &Path,
) -> Result<(BackendClient, PathBuf, PathBuf), Error> {
    fs::create_dir_all(directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
    }
    let mut bytes = Zeroizing::new(Vec::new());
    fs::File::open(path)?
        .take(128 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 128 * 1024 {
        return Err("Host情報の上限を超えています".into());
    }
    let (record, profile) = if path
        .extension()
        .is_some_and(|extension| extension == "host")
    {
        let key = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or("Host profile名が不正です")?;
        let plaintext = protect(&bytes, key.as_bytes(), true)?;
        let record: Record = serde_json::from_slice(&plaintext)?;
        if record.key != key {
            return Err("Host profileが一致しません".into());
        }
        (record, path.to_owned())
    } else {
        let invitation: HostInvitation = serde_json::from_slice(&bytes)?;
        if invitation.version != 1
            || invitation.token.len() != 32
            || invitation.certificate.len() > 64 * 1024
            || invitation.server_name.len() > 253
        {
            return Err("Host招待が不正です".into());
        }
        let mut random = [0u8; 16];
        getrandom::fill(&mut random)?;
        let client = format!(
            "desktop-{}",
            random
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        let hello = ClientHello {
            protocol: ProtocolRange::exact(CURRENT_PROTOCOL),
            client_id: ClientId::try_new(&client)?,
            client_name: "Desktop".to_owned(),
            client_kind: ClientKind::Desktop,
            requested_capabilities: crate::supported_capabilities(),
        };
        let mut token = Zeroizing::new([0; 32]);
        token.copy_from_slice(&invitation.token);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let config = runtime.block_on(QuicConnectionConfig::pair(
            invitation.address,
            invitation.server_name.clone(),
            invitation.certificate.clone().into(),
            hello,
            WorkspaceId::try_new(&invitation.workspace_id)?,
            &token,
        ))?;
        let key = profile_key(
            invitation.address,
            &invitation.server_name,
            &invitation.workspace_id,
            &invitation.certificate,
        )?;
        let credential = config
            .credential()
            .ok_or("ペアリング結果にcredentialがありません")?;
        let record = Record {
            version: 1,
            key,
            pid: 0,
            started_at: None,
            managed_lease: false,
            address: config.address,
            server_name: config.server_name.clone(),
            label: invitation.name.clone(),
            certificate: config.certificate.to_vec(),
            workspace: config.workspace_id.as_str().to_owned(),
            client,
            credential: Some(credential.id().as_str().to_owned()),
            secret: Some(credential.secret_bytes().to_vec()),
            id_token: None,
        };
        let profile = write_profile(directory, &record)?;
        (record, profile)
    };
    let root = remote_root(directory, &record)?;
    let backend = BackendClient::remote(root.clone(), record.config()?)?;
    Ok((backend, root, profile))
}

fn profile_key(
    address: SocketAddr,
    server_name: &str,
    workspace_id: &str,
    certificate: &[u8],
) -> Result<String, Error> {
    Ok(format!(
        "remote-{:x}",
        Sha256::digest(serde_json::to_vec(&(
            address,
            server_name,
            workspace_id,
            certificate
        ))?)
    ))
}

fn write_profile(directory: &Path, record: &Record) -> Result<PathBuf, Error> {
    let profile = directory.join(format!("{}.host", record.key));
    let plaintext = Zeroizing::new(serde_json::to_vec(record)?);
    let ciphertext = protect(&plaintext, record.key.as_bytes(), false)?;
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(&ciphertext)?;
    file.as_file().sync_all()?;
    file.persist(&profile)?;
    Ok(profile)
}

fn remote_root(directory: &Path, record: &Record) -> Result<PathBuf, Error> {
    let label = record
        .label
        .chars()
        .take(80)
        .map(|character| {
            if character.is_control() || "\\/:*?\"<>|".contains(character) {
                '_'
            } else {
                character
            }
        })
        .collect::<String>();
    let label = if label.trim_matches([' ', '.']).is_empty() {
        "Remote".to_owned()
    } else {
        label
    };
    let root = directory.join("remote").join(&record.key).join(label);
    if root.is_absolute() {
        Ok(root)
    } else {
        Ok(std::env::current_dir()?.join(root))
    }
}

/// OIDC で保存する接続プロファイル。
pub struct OidcProfile {
    pub address: SocketAddr,
    pub server_name: String,
    pub certificate: Vec<u8>,
    pub workspace_id: String,
    pub name: String,
    pub client: String,
    pub id_token: String,
}

/// OIDC で認証したプロファイルを保護して保存し、Remote接続を作る。
pub fn write_oidc_profile(
    directory: &Path,
    profile: OidcProfile,
) -> Result<(BackendClient, PathBuf, PathBuf), Error> {
    fs::create_dir_all(directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
    }
    let OidcProfile {
        address,
        server_name,
        certificate,
        workspace_id,
        name,
        client,
        id_token,
    } = profile;
    if certificate.len() > 64 * 1024 || server_name.len() > 253 {
        return Err("Host情報が不正です".into());
    }
    ClientId::try_new(&client)?;
    WorkspaceId::try_new(&workspace_id)?;
    if id_token.is_empty() {
        return Err("OIDC ID Tokenが空です".into());
    }
    let key = profile_key(address, &server_name, &workspace_id, &certificate)?;
    let record = Record {
        version: 1,
        key,
        pid: 0,
        started_at: None,
        managed_lease: false,
        address,
        server_name,
        label: name,
        certificate,
        workspace: workspace_id,
        client,
        credential: None,
        secret: None,
        id_token: Some(id_token),
    };
    let profile = write_profile(directory, &record)?;
    let root = remote_root(directory, &record)?;
    let backend = BackendClient::remote(root.clone(), record.config()?)?;
    Ok((backend, root, profile))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oidc_profile_is_protected_and_reloads_with_the_id_token() {
        let state = tempfile::tempdir().unwrap();
        let profiles = state.path().join("profiles");
        let (_, root, path) = write_oidc_profile(
            &profiles,
            OidcProfile {
                address: "127.0.0.1:1".parse().unwrap(),
                server_name: "lapis-host".to_owned(),
                certificate: vec![1, 2, 3],
                workspace_id: "workspace-1".to_owned(),
                name: "Remote".to_owned(),
                client: "desktop-1".to_owned(),
                id_token: "id-token".to_owned(),
            },
        )
        .unwrap();
        assert!(path.is_file());

        let key = path.file_stem().unwrap().to_str().unwrap();
        let plaintext = protect(&fs::read(&path).unwrap(), key.as_bytes(), true).unwrap();
        let record: Record = serde_json::from_slice(&plaintext).unwrap();
        assert_eq!(record.id_token.as_deref(), Some("id-token"));
        assert!(record.credential.is_none());
        assert!(record.secret.is_none());
        #[cfg(windows)]
        assert_ne!(fs::read(&path).unwrap().first(), Some(&b'{'));

        let (_, restored_root, restored_path) = open_host_file(&path, &profiles).unwrap();
        assert_eq!(root, restored_root);
        assert_eq!(path, restored_path);
    }

    #[test]
    fn empty_oidc_token_is_rejected() {
        let state = tempfile::tempdir().unwrap();
        let result = write_oidc_profile(
            state.path(),
            OidcProfile {
                address: "127.0.0.1:1".parse().unwrap(),
                server_name: "lapis-host".to_owned(),
                certificate: Vec::new(),
                workspace_id: "workspace-1".to_owned(),
                name: "Remote".to_owned(),
                client: "desktop-1".to_owned(),
                id_token: String::new(),
            },
        );
        let Err(error) = result else {
            panic!("empty ID Token must be rejected");
        };
        assert!(error.to_string().contains("ID Token"));
    }
}
