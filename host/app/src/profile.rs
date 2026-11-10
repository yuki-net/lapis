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

/// Desktop の接続画面へ出す Host の表示情報。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostProfileInfo {
    pub name: String,
    pub host_id: Option<String>,
}

/// 保存済み profile から Host の表示情報を読む。
pub fn host_profile_info(path: &Path) -> Result<HostProfileInfo, Error> {
    let record = read_profile(path)?;
    Ok(HostProfileInfo {
        name: record.label.clone(),
        host_id: record.host_id.clone(),
    })
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

/// 同じパスへ書き直す招待。起動ごとに作り直す1回限りのtokenを置き換える。
pub fn write_invitation_replacing(path: &Path, invitation: &HostInvitation) -> Result<(), Error> {
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    fs::create_dir_all(&directory)?;
    let mut file = tempfile::NamedTempFile::new_in(&directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    serde_json::to_writer(file.as_file_mut(), invitation)?;
    file.as_file_mut().flush()?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    Ok(())
}

/// 保護済みprofileを読む。パス名を鍵にしているため、別名へ複製すると読めない。
fn read_profile(path: &Path) -> Result<Record, Error> {
    let mut bytes = Zeroizing::new(Vec::new());
    fs::File::open(path)?
        .take(128 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 128 * 1024 {
        return Err("Host情報の上限を超えています".into());
    }
    let key = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or("Host profile名が不正です")?;
    let plaintext = protect(&bytes, key.as_bytes(), true)?;
    let record: Record = serde_json::from_slice(&plaintext)?;
    if record.key != key {
        return Err("Host profileが一致しません".into());
    }
    Ok(record)
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
        (read_profile(path)?, path.to_owned())
    } else {
        let invitation = parse_invitation(&bytes)?;
        if invitation.token.is_empty() {
            return Err("OIDC招待はDesktopのOIDCログインで処理してください".into());
        }
        let key = profile_key(
            "remote",
            invitation.address,
            &invitation.server_name,
            &invitation.workspace_id,
            &invitation.certificate,
            None,
        )?;
        let existing = directory.join(format!("{key}.host"));
        // tokenは1回限り。同じHostへ接続済みなら保存済みprofileを再利用する。
        if existing.is_file() {
            // 保存済みprofileで繋げない場合は、新しい招待でのペアリングへ進む。
            let reused = read_profile(&existing).and_then(|record| {
                let root = remote_root(directory, &record)?;
                let backend = BackendClient::remote(root.clone(), record.config()?)?;
                Ok((backend, root))
            });
            match reused {
                Ok((backend, root)) => {
                    eprintln!(
                        "保存済みのHost接続profileを使います: {}",
                        existing.display()
                    );
                    return Ok((backend, root, existing));
                }
                Err(error) => eprintln!("保存済みのHost接続profileを使えません: {error}"),
            }
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
        let hello = HelloRequest {
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
            workspace_request(&invitation.workspace_id)?,
            &token,
        ))?;
        let key = profile_key(
            "remote",
            invitation.address,
            &invitation.server_name,
            &invitation.workspace_id,
            &invitation.certificate,
            None,
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
            host_id: config.host_id.clone(),
            certificate: config.certificate.to_vec(),
            workspace: config
                .workspace_id
                .as_ref()
                .map(|workspace| workspace.as_str().to_owned())
                .unwrap_or_default(),
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

/// 招待ファイルを読み込み、検証する。Token が空なら OIDC 招待。
pub fn read_invitation(path: &Path) -> Result<HostInvitation, Error> {
    let mut bytes = Zeroizing::new(Vec::new());
    fs::File::open(path)?
        .take(128 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 128 * 1024 {
        return Err("Host情報の上限を超えています".into());
    }
    parse_invitation(&bytes)
}

fn parse_invitation(bytes: &[u8]) -> Result<HostInvitation, Error> {
    let invitation: HostInvitation = serde_json::from_slice(bytes)?;
    if invitation.version != 1
        || !matches!(invitation.token.len(), 0 | 32)
        || invitation.certificate.len() > 64 * 1024
        || invitation.server_name.len() > 253
    {
        return Err("Host招待が不正です".into());
    }
    Ok(invitation)
}

/// 招待の Workspace。空は Host の選択に任せる。
fn workspace_request(value: &str) -> Result<Option<WorkspaceId>, Error> {
    if value.trim().is_empty() {
        Ok(None)
    } else {
        Ok(Some(WorkspaceId::try_new(value)?))
    }
}

fn profile_key(
    auth: &str,
    address: SocketAddr,
    server_name: &str,
    workspace_id: &str,
    certificate: &[u8],
    account: Option<&str>,
) -> Result<String, Error> {
    Ok(format!(
        "{auth}-{:x}",
        Sha256::digest(serde_json::to_vec(&(
            address,
            server_name,
            workspace_id,
            certificate,
            account
        ))?)
    ))
}

fn write_profile(directory: &Path, record: &Record) -> Result<PathBuf, Error> {
    let profile = directory.join(format!("{}.host", record.key));
    let plaintext = Zeroizing::new(serde_json::to_vec(record)?);
    let ciphertext = protect(&plaintext, record.key.as_bytes(), false)?;
    if ciphertext.len() > 128 * 1024 {
        return Err("Host情報の上限を超えています".into());
    }
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
    pub host_id: Option<String>,
    pub client: String,
    /// Host が検証した ID Token の `sub`。保存先をアカウント別に分ける。
    pub account: String,
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
        host_id,
        client,
        account,
        id_token,
    } = profile;
    if certificate.len() > 64 * 1024 || server_name.len() > 253 {
        return Err("Host情報が不正です".into());
    }
    ClientId::try_new(&client)?;
    if !workspace_id.trim().is_empty() {
        WorkspaceId::try_new(&workspace_id)?;
    }
    if account.is_empty() {
        return Err("OIDC accountが空です".into());
    }
    if id_token.is_empty() {
        return Err("OIDC ID Tokenが空です".into());
    }
    let key = profile_key(
        "oidc",
        address,
        &server_name,
        &workspace_id,
        &certificate,
        Some(&account),
    )?;
    let record = Record {
        version: 1,
        key,
        pid: 0,
        started_at: None,
        managed_lease: false,
        address,
        server_name,
        label: name,
        host_id,
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
                host_id: None,
                client: "desktop-1".to_owned(),
                account: "alice".to_owned(),
                id_token: "id-token".to_owned(),
            },
        )
        .unwrap();
        assert!(path.is_file());
        assert!(
            path.file_stem()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("oidc-")
        );

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
    fn oidc_profile_does_not_replace_a_credential_profile() {
        let state = tempfile::tempdir().unwrap();
        let profiles = state.path().join("profiles");
        fs::create_dir_all(&profiles).unwrap();
        let address = "127.0.0.1:1".parse().unwrap();
        let certificate = vec![1, 2, 3];
        let key = profile_key(
            "remote",
            address,
            "lapis-host",
            "workspace-1",
            &certificate,
            None,
        )
        .unwrap();
        let credential = Record {
            version: 1,
            key,
            pid: 0,
            started_at: None,
            managed_lease: false,
            address,
            server_name: "lapis-host".to_owned(),
            label: "Remote".to_owned(),
            host_id: None,
            certificate: certificate.clone(),
            workspace: "workspace-1".to_owned(),
            client: "desktop-1".to_owned(),
            credential: Some("00000000000000000000000000000000".to_owned()),
            secret: Some(vec![1; 32]),
            id_token: None,
        };
        let credential_path = write_profile(&profiles, &credential).unwrap();
        let (_, _, oidc_path) = write_oidc_profile(
            &profiles,
            OidcProfile {
                address,
                server_name: "lapis-host".to_owned(),
                certificate,
                workspace_id: "workspace-1".to_owned(),
                name: "Remote".to_owned(),
                host_id: None,
                client: "desktop-1".to_owned(),
                account: "alice".to_owned(),
                id_token: "id-token".to_owned(),
            },
        )
        .unwrap();
        assert_ne!(credential_path, oidc_path);
        assert!(credential_path.is_file());
        assert!(oidc_path.is_file());
    }

    #[test]
    fn oidc_profiles_are_separated_by_account() {
        let state = tempfile::tempdir().unwrap();
        let profiles = state.path().join("profiles");
        let write = |account: &str, id_token: &str| {
            write_oidc_profile(
                &profiles,
                OidcProfile {
                    address: "127.0.0.1:1".parse().unwrap(),
                    server_name: "lapis-host".to_owned(),
                    certificate: vec![1, 2, 3],
                    workspace_id: "workspace-1".to_owned(),
                    name: "Remote".to_owned(),
                    host_id: None,
                    client: "desktop-1".to_owned(),
                    account: account.to_owned(),
                    id_token: id_token.to_owned(),
                },
            )
            .unwrap()
            .2
        };
        let alice = write("alice", "id-token-1");
        let bob = write("bob", "id-token-2");
        assert_ne!(alice, bob);
        assert!(alice.is_file());
        assert!(bob.is_file());

        let alice_updated = write("alice", "id-token-3");
        assert_eq!(alice, alice_updated);
    }

    #[test]
    fn empty_oidc_account_is_rejected() {
        let state = tempfile::tempdir().unwrap();
        let result = write_oidc_profile(
            state.path(),
            OidcProfile {
                address: "127.0.0.1:1".parse().unwrap(),
                server_name: "lapis-host".to_owned(),
                certificate: Vec::new(),
                workspace_id: "workspace-1".to_owned(),
                name: "Remote".to_owned(),
                host_id: None,
                client: "desktop-1".to_owned(),
                account: String::new(),
                id_token: "id-token".to_owned(),
            },
        );
        let Err(error) = result else {
            panic!("empty account must be rejected");
        };
        assert!(error.to_string().contains("account"));
    }

    #[test]
    fn oversized_oidc_profile_is_rejected() {
        let state = tempfile::tempdir().unwrap();
        let result = write_oidc_profile(
            state.path(),
            OidcProfile {
                address: "127.0.0.1:1".parse().unwrap(),
                server_name: "lapis-host".to_owned(),
                certificate: Vec::new(),
                workspace_id: "workspace-1".to_owned(),
                name: "Remote".to_owned(),
                host_id: None,
                client: "desktop-1".to_owned(),
                account: "alice".to_owned(),
                id_token: "a".repeat(200 * 1024),
            },
        );
        let Err(error) = result else {
            panic!("oversized profile must be rejected");
        };
        assert!(error.to_string().contains("上限"));
    }

    #[test]
    fn oidc_invitation_without_token_is_readable_but_not_pairing_input() {
        let state = tempfile::tempdir().unwrap();
        let path = state.path().join("oidc.json");
        let invitation = HostInvitation {
            version: 1,
            address: "127.0.0.1:1".parse().unwrap(),
            server_name: "lapis-host".to_owned(),
            certificate: vec![1, 2, 3],
            workspace_id: "workspace-1".to_owned(),
            name: "Remote".to_owned(),
            token: Vec::new(),
        };
        write_invitation(&path, &invitation).unwrap();
        let read = read_invitation(&path).unwrap();
        assert!(read.token.is_empty());

        let result = open_host_file(&path, &state.path().join("profiles"));
        let Err(error) = result else {
            panic!("OIDC invitation must not be paired");
        };
        assert!(error.to_string().contains("OIDC"));
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
                host_id: None,
                client: "desktop-1".to_owned(),
                account: "alice".to_owned(),
                id_token: String::new(),
            },
        );
        let Err(error) = result else {
            panic!("empty ID Token must be rejected");
        };
        assert!(error.to_string().contains("ID Token"));
    }
}
