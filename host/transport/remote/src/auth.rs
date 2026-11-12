use std::{collections::HashMap, error::Error, fmt, marker::PhantomData, sync::Arc};

use backend_state::DeviceStatus;
pub use backend_state::{StoredCredential, StoredDevice};
use client_api::{
    AuthMethod, AuthRequired, CapabilityId, CapabilitySet, ClientId, HelloRequest,
    OidcClientConfig, RequestBody, SessionId, WorkspaceId,
};

use crate::{
    authorization::{AuthorizationError, SessionGrant},
    clock::{Clock, SystemClock},
    credential::{CredentialHandle, CredentialId, CredentialRecord},
    oidc::OidcVerifier,
    pairing::{PairingToken, secret_from_token},
    random::{OsRandom, RandomError, RandomSource},
    secret::{SECRET_BYTES, constant_time_digest_matches, digest},
};

const MAX_PENDING_PAIRINGS: usize = 8;
const MAX_CREDENTIALS: usize = 64;
const MAX_DEVICES: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PairingLifetime(u64);

impl PairingLifetime {
    pub fn new(seconds: u64) -> Result<Self, AuthConfigError> {
        NonZeroSeconds::new(seconds).map(|value| Self(value.0))
    }

    fn seconds(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CredentialLifetime(u64);

impl CredentialLifetime {
    pub fn new(seconds: u64) -> Result<Self, AuthConfigError> {
        NonZeroSeconds::new(seconds).map(|value| Self(value.0))
    }

    fn seconds(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NonZeroSeconds(u64);

impl NonZeroSeconds {
    fn new(seconds: u64) -> Result<Self, AuthConfigError> {
        if seconds == 0 {
            Err(AuthConfigError::ZeroLifetime)
        } else {
            Ok(Self(seconds))
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthConfig {
    pub pairing_lifetime: PairingLifetime,
    pub credential_lifetime: CredentialLifetime,
    auto_approve_oidc_devices: bool,
}

impl AuthConfig {
    pub fn new(pairing_lifetime: PairingLifetime, credential_lifetime: CredentialLifetime) -> Self {
        Self {
            pairing_lifetime,
            credential_lifetime,
            auto_approve_oidc_devices: false,
        }
    }

    /// OIDC の新規 Device を Host 所有者の承認なしで許可する。
    pub fn with_auto_approve_oidc_devices(mut self, value: bool) -> Self {
        self.auto_approve_oidc_devices = value;
        self
    }

    pub fn auto_approve_oidc_devices(self) -> bool {
        self.auto_approve_oidc_devices
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthPolicy {
    allowed_capabilities: CapabilitySet,
}

impl AuthPolicy {
    pub fn new(allowed_capabilities: CapabilitySet) -> Self {
        Self {
            allowed_capabilities,
        }
    }

    pub fn allowed_capabilities(&self) -> &CapabilitySet {
        &self.allowed_capabilities
    }
}

/// Host が所有する Workspace 登録と Host 同一性を認証層へ提供する。
///
/// Workspace の正本は Host が持つため、認証層は候補を推測せずこの境界に問い合わせる。
pub trait WorkspaceResolver: Send + Sync {
    /// 永続化された Host ID。state directory を持たない Host は `None`。
    fn host_id(&self) -> Option<String>;

    /// 接続先を指定しなかった Client に提示する primary Workspace。
    fn primary_workspace(&self) -> Option<WorkspaceId>;

    /// 登録済み Workspace かどうか。
    fn contains(&self, workspace_id: &WorkspaceId) -> bool;
}

#[derive(Debug)]
pub enum AuthConfigError {
    ZeroLifetime,
}

impl fmt::Display for AuthConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroLifetime => formatter.write_str("authentication lifetime must be non-zero"),
        }
    }
}

impl Error for AuthConfigError {}

#[derive(Debug)]
pub enum AuthError {
    Disabled,
    PairingUnavailable,
    CredentialUnavailable,
    CredentialBindingMismatch,
    CredentialExpired,
    WorkspaceMismatch,
    WorkspaceUnavailable,
    HostMismatch,
    CapacityReached,
    Random(RandomError),
    OidcUnavailable,
    DeviceApprovalRequired,
    DeviceRevoked,
    PersistenceUnavailable,
}

impl fmt::Display for AuthError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disabled => formatter.write_str("remote authentication is disabled"),
            Self::PairingUnavailable => formatter.write_str("pairing token is invalid or expired"),
            Self::CredentialUnavailable => formatter.write_str("credential is invalid"),
            Self::CredentialBindingMismatch => {
                formatter.write_str("credential binding does not match")
            }
            Self::CredentialExpired => formatter.write_str("credential has expired"),
            Self::WorkspaceMismatch => {
                formatter.write_str("credential is not valid for this workspace")
            }
            Self::WorkspaceUnavailable => formatter.write_str("workspace is not registered"),
            Self::HostMismatch => formatter.write_str("credential belongs to another host"),
            Self::CapacityReached => formatter.write_str("authentication capacity reached"),
            Self::Random(_) => formatter.write_str("authentication secret generation failed"),
            Self::OidcUnavailable => formatter.write_str("OIDC token is invalid or unavailable"),
            Self::DeviceApprovalRequired => formatter.write_str("device approval is required"),
            Self::DeviceRevoked => formatter.write_str("device has been revoked"),
            Self::PersistenceUnavailable => {
                formatter.write_str("authentication state could not be saved")
            }
        }
    }
}

#[derive(Debug)]
pub enum AccessError {
    Authentication(AuthError),
    Authorization(AuthorizationError),
}

impl fmt::Display for AccessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Authentication(error) => error.fmt(formatter),
            Self::Authorization(error) => error.fmt(formatter),
        }
    }
}

impl Error for AccessError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Authentication(error) => Some(error),
            Self::Authorization(error) => Some(error),
        }
    }
}

impl Error for AuthError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Random(error) => Some(error),
            _ => None,
        }
    }
}

struct PendingPairing {
    workspace_id: WorkspaceId,
    expires_at: u64,
    scope: PairingScope,
}

#[derive(Clone, Copy)]
enum PairingScope {
    Workspace,
    User,
    Owner,
}

impl<C: Clock, R: RandomSource> RemoteAuth<C, R> {
    pub fn export_credentials(&self) -> Vec<StoredCredential> {
        self.credentials
            .iter()
            .map(|(id, record)| StoredCredential {
                id: id.as_str().to_owned(),
                client_id: record.client_id.as_str().to_owned(),
                workspace_id: record
                    .workspace_id
                    .as_ref()
                    .map(|workspace| workspace.as_str().to_owned()),
                capabilities: record
                    .capabilities
                    .iter()
                    .map(|capability| capability.as_str().to_owned())
                    .collect(),
                digest: hex_encode(&record.digest),
                expires_at: record.expires_at,
            })
            .collect()
    }

    pub fn import_credentials(&mut self, records: Vec<StoredCredential>) {
        let now = self.clock.now_unix_seconds();
        for record in records {
            let Ok(id) = CredentialId::parse(record.id) else {
                continue;
            };
            let Ok(client_id) = ClientId::try_new(record.client_id) else {
                continue;
            };
            let workspace_id = match record.workspace_id {
                Some(value) => match WorkspaceId::try_new(value) {
                    Ok(workspace_id) => Some(workspace_id),
                    Err(_) => continue,
                },
                None => None,
            };
            let capabilities = record
                .capabilities
                .iter()
                .map(|capability| CapabilityId::try_new(capability.clone()))
                .collect::<Result<Vec<_>, _>>();
            let Ok(capabilities) = capabilities else {
                continue;
            };
            let Ok(capabilities) = CapabilitySet::try_new(capabilities) else {
                continue;
            };
            let Some(digest) = hex_decode(&record.digest) else {
                continue;
            };
            if record.expires_at <= now {
                continue;
            }
            self.credentials.insert(
                id,
                CredentialRecord {
                    client_id,
                    workspace_id,
                    capabilities,
                    digest,
                    expires_at: record.expires_at,
                },
            );
        }
    }

    /// 保存済み device を返す。承認 UI と永続化で使う。
    pub fn devices(&self) -> Vec<StoredDevice> {
        let mut devices = self.devices.values().cloned().collect::<Vec<_>>();
        devices.sort_by(|left, right| left.id.cmp(&right.id));
        devices
    }

    /// Session の端末 ID。失効済みの credential は返さない。
    pub fn session_client_id(&self, session_id: &SessionId) -> Option<&str> {
        match self.sessions.get(session_id)? {
            SessionOwner::Credential(id) => Some(self.credentials.get(id)?.client_id.as_str()),
            SessionOwner::Oidc(session) => Some(&session.client_id),
        }
    }

    /// 保存済み device をすべて返す。
    pub fn export_devices(&self) -> Vec<StoredDevice> {
        self.devices()
    }

    /// 永続化された device を読み込む。
    pub fn import_devices(&mut self, devices: Vec<StoredDevice>) {
        for device in devices {
            if device.id.is_empty() || device.client_id.is_empty() {
                continue;
            }
            self.devices.insert(device.client_id.clone(), device);
        }
    }

    /// 承認待ちの device を許可する。
    pub fn approve_device(&mut self, device_id: &str) -> bool {
        let now = self.clock.now_unix_seconds();
        let Some(device) = self
            .devices
            .values_mut()
            .find(|device| device.id == device_id)
        else {
            return false;
        };
        device.status = DeviceStatus::Approved;
        device.approved_at = Some(now);
        device.revoked_at = None;
        true
    }

    /// device を失効させ、その Client の credential と OIDC Session を破棄する。
    pub fn revoke_device(&mut self, device_id: &str) -> bool {
        let now = self.clock.now_unix_seconds();
        let Some(client_id) = self
            .devices
            .values()
            .find(|device| device.id == device_id)
            .map(|device| device.client_id.clone())
        else {
            return false;
        };
        if let Some(device) = self.devices.get_mut(&client_id) {
            device.status = DeviceStatus::Revoked;
            device.revoked_at = Some(now);
        }
        self.credentials
            .retain(|_, record| record.client_id.as_str() != client_id);
        self.sessions.retain(|_, owner| match owner {
            SessionOwner::Credential(_) => true,
            SessionOwner::Oidc(session) => session.client_id != client_id,
        });
        true
    }

    /// 失効結果の返却前に保存する。失敗時は認証を停止し、成功を通知しない。
    /// 永続化 I/O を含むため blocking worker で呼び出す。
    pub fn revoke_device_and_persist(&mut self, device_id: &str) -> Result<bool, AuthError> {
        let revoked = self.revoke_device(device_id);
        if revoked {
            self.persist_state()?;
        }
        Ok(revoked)
    }

    /// 明示的な pairing token で接続した Client を承認済み device として登録する。
    pub fn ensure_token_device(&mut self, hello: &HelloRequest) -> Result<(), AuthError> {
        self.ensure_device(
            &hello.client_id,
            &hello.client_name,
            &kind_label(hello.client_kind),
            true,
        )
    }

    fn ensure_device(
        &mut self,
        client_id: &ClientId,
        name: &str,
        kind: &str,
        auto_approve: bool,
    ) -> Result<(), AuthError> {
        let now = self.clock.now_unix_seconds();
        if let Some(device) = self.devices.get_mut(client_id.as_str()) {
            device.last_seen_at = now;
            return match device.status {
                DeviceStatus::Approved => Ok(()),
                DeviceStatus::Pending => Err(AuthError::DeviceApprovalRequired),
                DeviceStatus::Revoked => Err(AuthError::DeviceRevoked),
            };
        }
        if self.devices.len() >= MAX_DEVICES {
            return Err(AuthError::CapacityReached);
        }
        let status = if auto_approve {
            DeviceStatus::Approved
        } else {
            DeviceStatus::Pending
        };
        let id = self.generate_device_id()?;
        let device = StoredDevice {
            id: id.clone(),
            client_id: client_id.as_str().to_owned(),
            name: name.to_owned(),
            kind: kind.to_owned(),
            status,
            created_at: now,
            last_seen_at: now,
            approved_at: (status == DeviceStatus::Approved).then_some(now),
            revoked_at: None,
        };
        self.devices.insert(device.client_id.clone(), device);
        if status == DeviceStatus::Pending {
            eprintln!("New device pending approval: {id} ({name})");
            Err(AuthError::DeviceApprovalRequired)
        } else {
            Ok(())
        }
    }

    fn generate_device_id(&mut self) -> Result<String, AuthError> {
        for _ in 0..4 {
            let id = crate::credential::generate_id(&mut self.random).map_err(AuthError::Random)?;
            let id = format!("device-{}", id.as_str());
            if !self.devices.values().any(|device| device.id == id) {
                return Ok(id);
            }
        }
        Err(AuthError::Random(RandomError))
    }
}

fn kind_label(kind: client_api::ClientKind) -> String {
    format!("{kind:?}").to_lowercase()
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn hex_decode(value: &str) -> Option<[u8; SECRET_BYTES]> {
    if value.len() != SECRET_BYTES * 2 {
        return None;
    }
    let mut output = [0u8; SECRET_BYTES];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let digit = |byte: u8| match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            _ => None,
        };
        output[index] = (digit(pair[0])? << 4) | digit(pair[1])?;
    }
    Some(output)
}

/// Session の所有者。Token credential と OIDC subject を混在させる。
enum SessionOwner {
    Credential(CredentialId),
    Oidc(OidcSession),
}

struct OidcSession {
    subject: String,
    client_id: String,
    workspace_id: WorkspaceId,
    capabilities: CapabilitySet,
    expires_at: u64,
}

/// credential と端末状態を同一 transaction で保存する。
pub trait AuthPersistence: Send + Sync {
    fn save(
        &self,
        credentials: &[StoredCredential],
        devices: &[StoredDevice],
    ) -> Result<(), AuthPersistenceError>;
}

#[derive(Debug)]
pub struct AuthPersistenceError;

/// Pairing、credential、session grantのライフサイクルを所有する認証サービス。
pub struct RemoteAuth<C = SystemClock, R = OsRandom> {
    clock: C,
    random: R,
    config: AuthConfig,
    policy: AuthPolicy,
    oidc: Option<Arc<OidcVerifier>>,
    oidc_client: Option<OidcClientConfig>,
    resolver: Option<Arc<dyn WorkspaceResolver>>,
    enabled: bool,
    pairings: HashMap<[u8; SECRET_BYTES], PendingPairing>,
    credentials: HashMap<CredentialId, CredentialRecord>,
    devices: HashMap<String, StoredDevice>,
    sessions: HashMap<SessionId, SessionOwner>,
    persistence: Option<Arc<dyn AuthPersistence>>,
    _marker: PhantomData<fn() -> SessionGrant>,
}

impl RemoteAuth<SystemClock, OsRandom> {
    pub fn system(config: AuthConfig, policy: AuthPolicy) -> Self {
        Self::new(SystemClock, OsRandom, config, policy)
    }
}

impl<C: Clock, R: RandomSource> RemoteAuth<C, R> {
    pub(crate) fn new(clock: C, random: R, config: AuthConfig, policy: AuthPolicy) -> Self {
        Self {
            clock,
            random,
            config,
            policy,
            oidc: None,
            oidc_client: None,
            resolver: None,
            enabled: false,
            pairings: HashMap::new(),
            credentials: HashMap::new(),
            devices: HashMap::new(),
            sessions: HashMap::new(),
            persistence: None,
            _marker: PhantomData,
        }
    }

    /// OIDC を有効化する。Token 方式は引き続き利用できる。
    pub fn enable_oidc(&mut self, verifier: Arc<OidcVerifier>, client: OidcClientConfig) {
        self.oidc = Some(verifier);
        self.oidc_client = Some(client);
    }

    /// 認証時に Workspace の既定値と存在確認を委譲する。
    pub fn set_workspace_resolver(&mut self, resolver: Arc<dyn WorkspaceResolver>) {
        self.resolver = Some(resolver);
    }

    /// 認証状態の保存先。設定時は認証処理から blocking worker 上で呼び出す。
    pub fn set_persistence(&mut self, persistence: Arc<dyn AuthPersistence>) {
        self.persistence = Some(persistence);
    }

    /// 呼出側は認証ロックを保持し、blocking worker で実行する。
    pub fn persist_state(&mut self) -> Result<(), AuthError> {
        if let Some(persistence) = &self.persistence
            && persistence
                .save(&self.export_credentials(), &self.export_devices())
                .is_err()
        {
            self.disable();
            return Err(AuthError::PersistenceUnavailable);
        }
        Ok(())
    }

    /// 接続先 Client へ提示する Host ID。
    pub fn host_id(&self) -> Option<String> {
        self.resolver
            .as_ref()
            .and_then(|resolver| resolver.host_id())
    }

    /// 現在の接続で提示する認証方式。
    pub fn methods(&self) -> Vec<AuthMethod> {
        if self.oidc.is_some() {
            vec![AuthMethod::Token, AuthMethod::Oidc]
        } else {
            vec![AuthMethod::Token]
        }
    }

    /// 接続直後に Client へ返す認証要件。
    pub fn auth_required(&self) -> AuthRequired {
        AuthRequired {
            methods: self.methods(),
            oidc: self.oidc_client.clone(),
        }
    }

    pub fn enable(&mut self) {
        self.enabled = true;
    }

    /// 新規・既存接続を停止する。PairingとSessionは破棄し、paired credentialは保持する。
    pub fn disable(&mut self) {
        self.enabled = false;
        self.pairings.clear();
        self.sessions.clear();
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn begin_pairing(&mut self, workspace_id: WorkspaceId) -> Result<PairingToken, AuthError> {
        self.begin_pairing_inner(workspace_id, PairingScope::Workspace)
    }

    /// Host所有者が明示した場合だけ、登録済みWorkspaceを切り替えられる招待を発行する。
    pub fn begin_user_session_pairing(
        &mut self,
        workspace_id: WorkspaceId,
    ) -> Result<PairingToken, AuthError> {
        self.begin_pairing_inner(workspace_id, PairingScope::User)
    }

    /// 管理Hostの起動元だけが使用する所有者用credentialの発行経路。
    pub fn begin_owner_pairing(
        &mut self,
        workspace_id: WorkspaceId,
    ) -> Result<PairingToken, AuthError> {
        self.begin_pairing_inner(workspace_id, PairingScope::Owner)
    }

    fn begin_pairing_inner(
        &mut self,
        workspace_id: WorkspaceId,
        scope: PairingScope,
    ) -> Result<PairingToken, AuthError> {
        self.require_enabled()?;
        let now = self.clock.now_unix_seconds();
        self.pairings.retain(|_, pairing| pairing.expires_at > now);
        if self.pairings.len() >= MAX_PENDING_PAIRINGS {
            return Err(AuthError::CapacityReached);
        }
        for _ in 0..4 {
            let token = PairingToken::generate(&mut self.random).map_err(AuthError::Random)?;
            let token_digest = digest(secret_from_token(&token));
            if self.pairings.contains_key(&token_digest) {
                continue;
            }
            self.pairings.insert(
                token_digest,
                PendingPairing {
                    workspace_id,
                    expires_at: self.expiry(self.config.pairing_lifetime.seconds()),
                    scope,
                },
            );
            return Ok(token);
        }
        Err(AuthError::Random(RandomError))
    }

    pub fn complete_pairing(
        &mut self,
        token: &PairingToken,
        client_id: &ClientId,
        requested_capabilities: &CapabilitySet,
    ) -> Result<CredentialHandle, AuthError> {
        self.require_enabled()?;
        let token_digest = digest(secret_from_token(token));
        let pairing = self
            .pairings
            .remove(&token_digest)
            .ok_or(AuthError::PairingUnavailable)?;
        if self.clock.now_unix_seconds() >= pairing.expires_at {
            return Err(AuthError::PairingUnavailable);
        }
        self.purge_expired_credentials();
        if self.credentials.len() >= MAX_CREDENTIALS {
            return Err(AuthError::CapacityReached);
        }
        let id = self.generate_credential_id()?;
        let mut secret = [0; SECRET_BYTES];
        self.random
            .fill_bytes(&mut secret)
            .map_err(AuthError::Random)?;
        let granted = requested_capabilities.intersection(self.policy.allowed_capabilities());
        let capabilities = CapabilitySet::try_new(
            granted
                .iter()
                .filter(|capability| match pairing.scope {
                    PairingScope::Workspace => {
                        !capability.as_str().starts_with("host.")
                            && !matches!(
                                capability.as_str(),
                                client_api::capability::WORKSPACES_SWITCH
                                    | client_api::capability::WORKSPACES_REGISTER
                            )
                    }
                    PairingScope::User => !capability.as_str().starts_with("host."),
                    PairingScope::Owner => true,
                })
                .cloned(),
        )
        .expect("intersection cannot exceed capability limit");
        // Workspace を切り替えられる credential は接続先を固定しない。
        // ゲスト等の Workspace 限定 credential は pairing 先へ固定したままにする。
        let switchable = capabilities.iter().any(|capability| {
            matches!(
                capability.as_str(),
                client_api::capability::HOST_MANAGE | client_api::capability::WORKSPACES_SWITCH
            )
        });
        let workspace_id = (!switchable).then_some(pairing.workspace_id);
        let record = CredentialRecord {
            client_id: client_id.clone(),
            workspace_id,
            capabilities,
            digest: digest(&secret),
            expires_at: self.expiry(self.config.credential_lifetime.seconds()),
        };
        self.credentials.insert(id.clone(), record);
        Ok(CredentialHandle::new(id, std::mem::take(&mut secret)))
    }

    /// Pairing tokenに結び付いたWorkspaceでcredential発行とsession確立を一体で行う。
    pub fn complete_pairing_and_authenticate(
        &mut self,
        token: &PairingToken,
        hello: &HelloRequest,
        requested_capabilities: &CapabilitySet,
    ) -> Result<(CredentialHandle, SessionGrant), AuthError> {
        let credential = self.complete_pairing(token, &hello.client_id, requested_capabilities)?;
        if let Err(error) = self.ensure_token_device(hello) {
            self.revoke(credential.id());
            return Err(error);
        }
        match self.authenticate_auto(&credential, &hello.client_id) {
            Ok(grant) => {
                if let Err(error) = self.persist_state() {
                    self.revoke(credential.id());
                    return Err(error);
                }
                Ok((credential, grant))
            }
            Err(error) => {
                self.revoke(credential.id());
                Err(error)
            }
        }
    }

    pub fn authenticate(
        &mut self,
        credential: &CredentialHandle,
        client_id: &ClientId,
        workspace_id: &WorkspaceId,
    ) -> Result<SessionGrant, AuthError> {
        self.authenticate_inner(credential, client_id, Some(workspace_id), None)
    }

    /// credential が発行された Host を Client が表明する場合の認証。
    pub fn authenticate_with_host(
        &mut self,
        credential: &CredentialHandle,
        client_id: &ClientId,
        workspace_id: &WorkspaceId,
        host_id: Option<&str>,
    ) -> Result<SessionGrant, AuthError> {
        self.authenticate_inner(credential, client_id, Some(workspace_id), host_id)
    }

    /// Workspace を指定せず、固定先または Host の primary Workspace で Session を発行する。
    pub fn authenticate_auto(
        &mut self,
        credential: &CredentialHandle,
        client_id: &ClientId,
    ) -> Result<SessionGrant, AuthError> {
        self.authenticate_inner(credential, client_id, None, None)
    }

    /// Host を表明し、Workspace を指定せずに認証する。
    pub fn authenticate_auto_with_host(
        &mut self,
        credential: &CredentialHandle,
        client_id: &ClientId,
        host_id: Option<&str>,
    ) -> Result<SessionGrant, AuthError> {
        self.authenticate_inner(credential, client_id, None, host_id)
    }

    fn authenticate_inner(
        &mut self,
        credential: &CredentialHandle,
        client_id: &ClientId,
        workspace_id: Option<&WorkspaceId>,
        expected_host_id: Option<&str>,
    ) -> Result<SessionGrant, AuthError> {
        self.require_enabled()?;
        let record = self
            .credentials
            .get(credential.id())
            .ok_or(AuthError::CredentialUnavailable)?;
        if !constant_time_digest_matches(credential.secret_bytes(), &record.digest) {
            return Err(AuthError::CredentialUnavailable);
        }
        if &record.client_id != client_id {
            return Err(AuthError::CredentialBindingMismatch);
        }
        if let Some(expected_host_id) = expected_host_id
            && let Some(host_id) = self.host_id()
            && host_id != expected_host_id
        {
            return Err(AuthError::HostMismatch);
        }
        let now = self.clock.now_unix_seconds();
        if now >= record.expires_at {
            return Err(AuthError::CredentialExpired);
        }
        let may_switch = record.capabilities.iter().any(|capability| {
            matches!(
                capability.as_str(),
                client_api::capability::HOST_MANAGE | client_api::capability::WORKSPACES_SWITCH
            )
        });
        let granted_workspace = match workspace_id {
            Some(requested) => {
                if may_switch {
                    // 切り替え可能でも、Host に登録されていない ID は受け付けない。
                    if let Some(resolver) = &self.resolver
                        && !resolver.contains(requested)
                    {
                        return Err(AuthError::WorkspaceUnavailable);
                    }
                    requested.clone()
                } else if record.workspace_id.as_ref() == Some(requested) {
                    requested.clone()
                } else {
                    return Err(AuthError::WorkspaceMismatch);
                }
            }
            None => match &record.workspace_id {
                Some(workspace_id) => workspace_id.clone(),
                None if may_switch => self
                    .resolver
                    .as_ref()
                    .and_then(|resolver| resolver.primary_workspace())
                    .ok_or(AuthError::WorkspaceUnavailable)?,
                None => return Err(AuthError::WorkspaceUnavailable),
            },
        };
        let record_client_id = record.client_id.clone();
        let granted_capabilities = record.capabilities.clone();
        self.ensure_device(&record_client_id, record_client_id.as_str(), "token", true)?;
        let session_id = self.generate_session_id()?;
        let grant = SessionGrant::from_authenticated(
            session_id.clone(),
            granted_workspace,
            granted_capabilities,
        );
        self.sessions.insert(
            session_id,
            SessionOwner::Credential(credential.id().clone()),
        );
        Ok(grant)
    }

    /// OIDC ID Token を検証し、Host の Session を発行する。
    pub fn authenticate_oidc(
        &mut self,
        id_token: &str,
        hello: &HelloRequest,
        workspace_id: &WorkspaceId,
        requested_capabilities: &CapabilitySet,
    ) -> Result<(SessionGrant, String), AuthError> {
        self.require_enabled()?;
        let claims = {
            let verifier = self.oidc.as_ref().ok_or(AuthError::OidcUnavailable)?;
            verifier
                .verify(id_token)
                .map_err(|_| AuthError::OidcUnavailable)?
        };
        let now = self.clock.now_unix_seconds();
        if claims.expires_at <= now {
            return Err(AuthError::OidcUnavailable);
        }
        let auto_approve = self.config.auto_approve_oidc_devices;
        self.ensure_device(
            &hello.client_id,
            &hello.client_name,
            &kind_label(hello.client_kind),
            auto_approve,
        )?;
        let subject = claims.subject;
        let granted = requested_capabilities.intersection(self.policy.allowed_capabilities());
        let capabilities = CapabilitySet::try_new(
            granted
                .iter()
                .filter(|capability| !capability.as_str().starts_with("host."))
                .cloned(),
        )
        .expect("intersection cannot exceed capability limit");
        let expires_at = claims
            .expires_at
            .min(self.expiry(self.config.credential_lifetime.seconds()));
        let session_id = self.generate_session_id()?;
        let grant = SessionGrant::from_authenticated(
            session_id.clone(),
            workspace_id.clone(),
            capabilities.clone(),
        );
        self.sessions.insert(
            session_id,
            SessionOwner::Oidc(OidcSession {
                subject: subject.clone(),
                client_id: hello.client_id.as_str().to_owned(),
                workspace_id: workspace_id.clone(),
                capabilities,
                expires_at,
            }),
        );
        Ok((grant, subject))
    }

    pub fn revoke(&mut self, credential_id: &CredentialId) -> bool {
        let removed = self.credentials.remove(credential_id).is_some();
        if removed {
            self.sessions.retain(
                |_, owner| !matches!(owner, SessionOwner::Credential(id) if id == credential_id),
            );
        }
        removed
    }

    pub fn disconnect(&mut self, session_id: &SessionId) -> bool {
        self.sessions.remove(session_id).is_some()
    }

    pub fn authorize_request(
        &self,
        grant: &SessionGrant,
        request: &RequestBody,
    ) -> Result<(), AccessError> {
        self.require_active_session(grant)
            .map_err(AccessError::Authentication)?;
        if let Some(workspace_id) = request.workspace_id() {
            // Workspace へ接続する要求だけは、`workspace.switch` を持つ Session に
            // 限り別 Workspace を許す。切替後の権限確認はセッション側で行う。
            let switching = matches!(request, RequestBody::WorkspaceConnect(_))
                && grant.require_workspace_switch().is_ok();
            if !switching {
                grant
                    .require_workspace(workspace_id)
                    .map_err(AccessError::Authorization)?;
            }
        }
        grant
            .require_request(request)
            .map_err(AccessError::Authorization)
    }

    /// 切替が成功した後に、Session の接続先を新しい Workspace へ確定する。
    pub fn commit_workspace_switch(&mut self, session_id: &SessionId, workspace_id: &WorkspaceId) {
        // OIDC Session は失効確認で Workspace を照合するため、切替先へ合わせる。
        if let Some(SessionOwner::Oidc(session)) = self.sessions.get_mut(session_id) {
            session.workspace_id = workspace_id.clone();
        }
    }

    fn require_enabled(&self) -> Result<(), AuthError> {
        if self.enabled {
            Ok(())
        } else {
            Err(AuthError::Disabled)
        }
    }

    fn expiry(&self, lifetime: u64) -> u64 {
        self.clock.now_unix_seconds().saturating_add(lifetime)
    }

    pub(crate) fn require_active_session(&self, grant: &SessionGrant) -> Result<(), AuthError> {
        self.require_enabled()?;
        let owner = self
            .sessions
            .get(grant.session_id())
            .ok_or(AuthError::CredentialUnavailable)?;
        match owner {
            SessionOwner::Credential(credential_id) => {
                let record = self
                    .credentials
                    .get(credential_id)
                    .ok_or(AuthError::CredentialUnavailable)?;
                if self.clock.now_unix_seconds() >= record.expires_at {
                    return Err(AuthError::CredentialExpired);
                }
            }
            SessionOwner::Oidc(session) => {
                if self.clock.now_unix_seconds() >= session.expires_at {
                    return Err(AuthError::CredentialExpired);
                }
                if &session.workspace_id != grant.workspace_id()
                    || &session.capabilities != grant.capabilities()
                {
                    return Err(AuthError::CredentialBindingMismatch);
                }
                let _ = &session.subject;
            }
        }
        Ok(())
    }

    fn purge_expired_credentials(&mut self) {
        let now = self.clock.now_unix_seconds();
        self.credentials
            .retain(|_, credential| credential.expires_at > now);
        self.sessions.retain(|_, owner| match owner {
            SessionOwner::Credential(credential_id) => self.credentials.contains_key(credential_id),
            SessionOwner::Oidc(session) => session.expires_at > now,
        });
    }

    fn generate_session_id(&mut self) -> Result<SessionId, AuthError> {
        let id = crate::credential::generate_id(&mut self.random).map_err(AuthError::Random)?;
        SessionId::try_new(format!("session-{}", id.as_str()))
            .map_err(|_| AuthError::CredentialUnavailable)
    }

    fn generate_credential_id(&mut self) -> Result<CredentialId, AuthError> {
        for _ in 0..4 {
            let id = crate::credential::generate_id(&mut self.random).map_err(AuthError::Random)?;
            if !self.credentials.contains_key(&id) {
                return Ok(id);
            }
        }
        Err(AuthError::Random(RandomError))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use client_api::{CURRENT_PROTOCOL, CapabilityId, ClientKind, ProtocolRange, capability};

    fn hello(client_id: &str) -> HelloRequest {
        HelloRequest {
            protocol: ProtocolRange::exact(CURRENT_PROTOCOL),
            client_id: ClientId::try_new(client_id).unwrap(),
            client_name: "Web".to_owned(),
            client_kind: ClientKind::Desktop,
            requested_capabilities: CapabilitySet::default(),
        }
    }

    #[derive(Clone, Debug)]
    struct TestClock(u64);

    impl Clock for TestClock {
        fn now_unix_seconds(&self) -> u64 {
            self.0
        }
    }

    struct TestRandom {
        next: u8,
    }

    impl RandomSource for TestRandom {
        fn fill_bytes(&mut self, destination: &mut [u8]) -> Result<(), RandomError> {
            for byte in destination {
                *byte = self.next;
                self.next = self.next.wrapping_add(1);
            }
            Ok(())
        }
    }

    fn setup() -> RemoteAuth<TestClock, TestRandom> {
        let files = CapabilityId::try_new(capability::FILES_READ).unwrap();
        RemoteAuth::new(
            TestClock(100),
            TestRandom { next: 1 },
            AuthConfig::new(
                PairingLifetime::new(10).unwrap(),
                CredentialLifetime::new(20).unwrap(),
            ),
            AuthPolicy::new(CapabilitySet::try_new([files]).unwrap()),
        )
    }

    fn ids() -> (ClientId, WorkspaceId) {
        (
            ClientId::try_new("client-1").unwrap(),
            WorkspaceId::try_new("workspace-1").unwrap(),
        )
    }

    struct TestResolver {
        host_id: Option<String>,
        primary: Option<WorkspaceId>,
        registered: Vec<WorkspaceId>,
    }

    impl TestResolver {
        fn new(primary: WorkspaceId, registered: Vec<WorkspaceId>) -> Self {
            Self {
                host_id: Some("host-1".to_owned()),
                primary: Some(primary),
                registered,
            }
        }
    }

    impl WorkspaceResolver for TestResolver {
        fn host_id(&self) -> Option<String> {
            self.host_id.clone()
        }

        fn primary_workspace(&self) -> Option<WorkspaceId> {
            self.primary.clone()
        }

        fn contains(&self, workspace_id: &WorkspaceId) -> bool {
            self.registered.contains(workspace_id)
        }
    }

    #[test]
    fn disabled_by_default_and_disable_preserves_paired_credentials() {
        let mut auth = setup();
        let (client, workspace) = ids();
        let requested = CapabilitySet::default();
        assert!(matches!(
            auth.begin_pairing(workspace.clone()),
            Err(AuthError::Disabled)
        ));

        auth.enable();
        let token = auth.begin_pairing(workspace.clone()).unwrap();
        let credential = auth.complete_pairing(&token, &client, &requested).unwrap();
        let old_grant = auth.authenticate(&credential, &client, &workspace).unwrap();
        auth.disable();

        assert!(!auth.is_enabled());
        assert!(matches!(
            auth.authenticate(&credential, &client, &workspace),
            Err(AuthError::Disabled)
        ));
        auth.enable();
        assert!(matches!(
            auth.require_active_session(&old_grant),
            Err(AuthError::CredentialUnavailable)
        ));
        assert!(auth.authenticate(&credential, &client, &workspace).is_ok());
    }

    #[test]
    fn pairing_is_one_time_and_credential_is_bound() {
        let mut auth = setup();
        auth.enable();
        let (client, workspace) = ids();
        let requested = CapabilitySet::default();
        let token = auth.begin_pairing(workspace.clone()).unwrap();
        let credential = auth.complete_pairing(&token, &client, &requested).unwrap();
        assert!(matches!(
            auth.complete_pairing(&token, &client, &requested),
            Err(AuthError::PairingUnavailable)
        ));
        let other_client = ClientId::try_new("client-2").unwrap();
        assert!(matches!(
            auth.authenticate(&credential, &other_client, &workspace),
            Err(AuthError::CredentialBindingMismatch)
        ));
    }

    #[test]
    fn expired_pairing_and_credential_are_rejected() {
        let mut auth = setup();
        auth.enable();
        let (client, workspace) = ids();
        let requested = CapabilitySet::default();
        let token = auth.begin_pairing(workspace.clone()).unwrap();
        auth.clock.0 = 110;
        assert!(matches!(
            auth.complete_pairing(&token, &client, &requested),
            Err(AuthError::PairingUnavailable)
        ));

        let token = auth.begin_pairing(workspace.clone()).unwrap();
        let credential = auth.complete_pairing(&token, &client, &requested).unwrap();
        auth.clock.0 = 130;
        assert!(matches!(
            auth.authenticate(&credential, &client, &workspace),
            Err(AuthError::CredentialExpired)
        ));
    }

    #[test]
    fn exported_credentials_survive_a_restart_and_expired_ones_are_dropped() {
        let mut auth = setup();
        auth.enable();
        let (client, workspace) = ids();
        let token = auth.begin_pairing(workspace.clone()).unwrap();
        let credential = auth
            .complete_pairing(&token, &client, &CapabilitySet::default())
            .unwrap();
        let exported = auth.export_credentials();
        assert_eq!(exported.len(), 1);

        let mut restarted = setup();
        restarted.enable();
        restarted.import_credentials(exported);
        assert!(
            restarted
                .authenticate(&credential, &client, &workspace)
                .is_ok()
        );

        let mut expired = setup();
        expired.clock.0 = 130;
        expired.import_credentials(restarted.export_credentials());
        assert!(expired.export_credentials().is_empty());
    }

    #[test]
    fn only_host_owner_can_authenticate_for_another_workspace() {
        let mut auth = setup();
        let owner = CapabilityId::try_new(capability::HOST_MANAGE).unwrap();
        let capabilities = CapabilitySet::try_new([
            owner,
            CapabilityId::try_new(capability::WORKSPACES_CONNECT).unwrap(),
        ])
        .unwrap();
        auth.policy = AuthPolicy::new(capabilities.clone());
        auth.enable();
        let (client, workspace) = ids();
        let other = WorkspaceId::try_new("workspace-2").unwrap();
        let token = auth.begin_pairing(workspace.clone()).unwrap();
        let guest = auth
            .complete_pairing(&token, &client, &CapabilitySet::default())
            .unwrap();
        assert!(matches!(
            auth.authenticate(&guest, &client, &other),
            Err(AuthError::WorkspaceMismatch)
        ));
        let token = auth.begin_owner_pairing(workspace).unwrap();
        let credential = auth
            .complete_pairing(&token, &client, &capabilities)
            .unwrap();
        let grant = auth.authenticate(&credential, &client, &other).unwrap();
        assert_eq!(grant.workspace_id(), &other);
        assert!(matches!(
            auth.authenticate(
                &credential,
                &ClientId::try_new("another-client").unwrap(),
                &other
            ),
            Err(AuthError::CredentialBindingMismatch)
        ));
        // 所有者も一つのsessionから別Workspaceの通常操作はできない。
        assert!(matches!(
            auth.authorize_request(
                &grant,
                &RequestBody::WorkspaceConnect(client_api::WorkspaceConnectRequest {
                    workspace_id: ids().1
                })
            ),
            Err(AccessError::Authorization(
                crate::AuthorizationError::WorkspaceDenied
            ))
        ));
        assert!(auth.revoke(credential.id()));
        assert!(auth.require_active_session(&grant).is_err());
    }

    #[test]
    fn user_session_pairing_allows_switch_without_host_management() {
        let mut auth = setup();
        let switch = CapabilityId::try_new(capability::WORKSPACES_SWITCH).unwrap();
        let register = CapabilityId::try_new(capability::WORKSPACES_REGISTER).unwrap();
        let manage = CapabilityId::try_new(capability::HOST_MANAGE).unwrap();
        let future_host_capability = CapabilityId::try_new("host.future-admin").unwrap();
        let list = CapabilityId::try_new(capability::WORKSPACES).unwrap();
        let requested = CapabilitySet::try_new([
            switch.clone(),
            register.clone(),
            manage.clone(),
            future_host_capability.clone(),
            list,
        ])
        .unwrap();
        auth.policy = AuthPolicy::new(requested.clone());
        auth.enable();
        let (client, workspace) = ids();
        let other = WorkspaceId::try_new("workspace-2").unwrap();
        let normal = auth.begin_pairing(workspace.clone()).unwrap();
        let normal = auth.complete_pairing(&normal, &client, &requested).unwrap();
        let normal_grant = auth.authenticate(&normal, &client, &workspace).unwrap();
        assert!(!normal_grant.capabilities().contains(&switch));
        assert!(!normal_grant.capabilities().contains(&register));
        assert!(!normal_grant.capabilities().contains(&manage));
        assert!(
            !normal_grant
                .capabilities()
                .contains(&future_host_capability)
        );
        assert!(matches!(
            auth.authenticate(&normal, &client, &other),
            Err(AuthError::WorkspaceMismatch)
        ));
        let user = auth.begin_user_session_pairing(workspace.clone()).unwrap();
        let user = auth.complete_pairing(&user, &client, &requested).unwrap();
        let grant = auth.authenticate(&user, &client, &other).unwrap();
        assert!(grant.capabilities().contains(&switch));
        assert!(grant.capabilities().contains(&register));
        assert!(!grant.capabilities().contains(&manage));
        assert!(!grant.capabilities().contains(&future_host_capability));
        assert!(matches!(
            grant.require_workspace(&workspace),
            Err(AuthorizationError::WorkspaceDenied)
        ));
        assert!(auth.revoke(user.id()));
        assert!(auth.require_active_session(&grant).is_err());
    }

    #[test]
    fn workspace_switch_needs_both_the_capability_and_a_closed_attachment() {
        let mut auth = setup();
        let switch = CapabilityId::try_new(capability::WORKSPACES_SWITCH).unwrap();
        let connect = CapabilityId::try_new(capability::WORKSPACES_CONNECT).unwrap();
        let requested = CapabilitySet::try_new([switch.clone(), connect]).unwrap();
        auth.policy = AuthPolicy::new(requested.clone());
        auth.enable();
        let (client, workspace) = ids();
        let other = WorkspaceId::try_new("workspace-2").unwrap();

        // 切替権限が無い Session は、別 Workspace への connect を認可されない。
        let normal = auth.begin_pairing(workspace.clone()).unwrap();
        let normal = auth.complete_pairing(&normal, &client, &requested).unwrap();
        let normal = auth.authenticate(&normal, &client, &workspace).unwrap();
        assert!(matches!(
            auth.authorize_request(
                &normal,
                &RequestBody::WorkspaceConnect(client_api::WorkspaceConnectRequest {
                    workspace_id: other.clone(),
                })
            ),
            Err(AccessError::Authorization(
                AuthorizationError::WorkspaceDenied
            ))
        ));

        // 切替権限があれば、connect 自体は別 Workspace でも認可される。
        let user = auth.begin_user_session_pairing(workspace.clone()).unwrap();
        let user = auth.complete_pairing(&user, &client, &requested).unwrap();
        let grant = auth.authenticate(&user, &client, &workspace).unwrap();
        let switching = RequestBody::WorkspaceConnect(client_api::WorkspaceConnectRequest {
            workspace_id: other.clone(),
        });
        assert!(auth.authorize_request(&grant, &switching).is_ok());

        // 切替権限があっても、connect 以外は接続中の Workspace に限定したまま。
        assert!(matches!(
            auth.authorize_request(
                &grant,
                &RequestBody::FileTree(client_api::FileTreeRequest {
                    workspace_id: other.clone(),
                    path: None,
                })
            ),
            Err(AccessError::Authorization(
                AuthorizationError::WorkspaceDenied
            ))
        ));
    }

    #[test]
    fn user_session_authenticate_auto_resolves_the_primary_workspace() {
        let mut auth = setup();
        let switch = CapabilityId::try_new(capability::WORKSPACES_SWITCH).unwrap();
        let requested = CapabilitySet::try_new([switch.clone()]).unwrap();
        auth.policy = AuthPolicy::new(requested.clone());
        auth.enable();
        let (client, workspace) = ids();
        let primary = WorkspaceId::try_new("workspace-2").unwrap();
        auth.set_workspace_resolver(Arc::new(TestResolver::new(
            primary.clone(),
            vec![workspace.clone(), primary.clone()],
        )));
        let token = auth.begin_user_session_pairing(workspace).unwrap();
        let credential = auth.complete_pairing(&token, &client, &requested).unwrap();

        // 切り替え可能な credential は Workspace に固定しない。
        assert_eq!(auth.export_credentials()[0].workspace_id, None);
        let grant = auth.authenticate_auto(&credential, &client).unwrap();
        assert_eq!(grant.workspace_id(), &primary);

        let unknown = WorkspaceId::try_new("workspace-unknown").unwrap();
        assert!(matches!(
            auth.authenticate(&credential, &client, &unknown),
            Err(AuthError::WorkspaceUnavailable)
        ));
        assert_eq!(
            auth.authenticate_auto_with_host(&credential, &client, Some("host-1"))
                .unwrap()
                .workspace_id(),
            &primary
        );
        assert!(matches!(
            auth.authenticate_auto_with_host(&credential, &client, Some("other-host")),
            Err(AuthError::HostMismatch)
        ));
    }

    #[test]
    fn auto_authentication_without_primary_workspace_is_unavailable() {
        let mut auth = setup();
        let switch = CapabilityId::try_new(capability::WORKSPACES_SWITCH).unwrap();
        let requested = CapabilitySet::try_new([switch]).unwrap();
        auth.policy = AuthPolicy::new(requested.clone());
        auth.enable();
        let (client, workspace) = ids();
        let token = auth.begin_user_session_pairing(workspace).unwrap();
        let credential = auth.complete_pairing(&token, &client, &requested).unwrap();

        assert!(matches!(
            auth.authenticate_auto(&credential, &client),
            Err(AuthError::WorkspaceUnavailable)
        ));
    }

    #[test]
    fn requested_capabilities_are_intersected_with_backend_policy() {
        let mut auth = setup();
        auth.enable();
        let (client, workspace) = ids();
        let requested = CapabilitySet::try_new([
            CapabilityId::try_new(capability::FILES_READ).unwrap(),
            CapabilityId::try_new(capability::TERMINAL_START).unwrap(),
        ])
        .unwrap();
        let token = auth.begin_pairing(workspace.clone()).unwrap();
        let credential = auth.complete_pairing(&token, &client, &requested).unwrap();
        let grant = auth.authenticate(&credential, &client, &workspace).unwrap();
        let files = CapabilityId::try_new(capability::FILES_READ).unwrap();
        let terminal = CapabilityId::try_new(capability::TERMINAL_START).unwrap();
        assert_eq!(grant.require_capability(&files), Ok(()));
        assert!(matches!(
            grant.require_capability(&terminal),
            Err(crate::AuthorizationError::CapabilityDenied(_))
        ));
        let files_request = RequestBody::FileTree(client_api::FileTreeRequest {
            workspace_id: workspace.clone(),
            path: None,
        });
        let terminal_request = RequestBody::TerminalStart(client_api::TerminalStartRequest {
            workspace_id: workspace.clone(),
            cwd: None,
            command: None,
            profile: None,
            size: client_api::TerminalSize {
                columns: 80,
                rows: 24,
            },
        });
        assert!(auth.authorize_request(&grant, &files_request).is_ok());
        assert!(matches!(
            auth.authorize_request(&grant, &terminal_request),
            Err(AccessError::Authorization(_))
        ));
        let other_workspace_request = RequestBody::FileTree(client_api::FileTreeRequest {
            workspace_id: WorkspaceId::try_new("workspace-2").unwrap(),
            path: None,
        });
        assert!(matches!(
            auth.authorize_request(&grant, &other_workspace_request),
            Err(AccessError::Authorization(
                crate::AuthorizationError::WorkspaceDenied
            ))
        ));
    }

    #[test]
    fn credential_rejects_wrong_secret_and_revocation() {
        let mut auth = setup();
        auth.enable();
        let (client, workspace) = ids();
        let requested = CapabilitySet::default();
        let token = auth.begin_pairing(workspace.clone()).unwrap();
        let credential = auth.complete_pairing(&token, &client, &requested).unwrap();
        let grant = auth.authenticate(&credential, &client, &workspace).unwrap();
        let mut wrong = [0; SECRET_BYTES];
        wrong[0] = 255;
        let forged = CredentialHandle::new(credential.id().clone(), wrong);
        assert!(matches!(
            auth.authenticate(&forged, &client, &workspace),
            Err(AuthError::CredentialUnavailable)
        ));
        assert!(auth.revoke(credential.id()));
        assert!(!auth.revoke(credential.id()));
        assert!(matches!(
            auth.authenticate(&credential, &client, &workspace),
            Err(AuthError::CredentialUnavailable)
        ));
        assert!(matches!(
            auth.require_active_session(&grant),
            Err(AuthError::CredentialUnavailable)
        ));
    }

    #[test]
    fn secret_debug_output_is_redacted() {
        let mut auth = setup();
        auth.enable();
        let (client, workspace) = ids();
        let token = auth.begin_pairing(workspace.clone()).unwrap();
        let credential = auth
            .complete_pairing(&token, &client, &CapabilitySet::default())
            .unwrap();
        assert_eq!(format!("{token:?}"), "PairingToken(REDACTED)");
        assert!(!format!("{credential:?}").contains("010203"));
        assert!(format!("{credential:?}").contains("REDACTED"));
    }

    #[test]
    fn every_authentication_creates_a_fresh_session_id() {
        let mut auth = setup();
        auth.enable();
        let (client, workspace) = ids();
        let token = auth.begin_pairing(workspace.clone()).unwrap();
        let credential = auth
            .complete_pairing(&token, &client, &CapabilitySet::default())
            .unwrap();

        let first = auth.authenticate(&credential, &client, &workspace).unwrap();
        let second = auth.authenticate(&credential, &client, &workspace).unwrap();
        assert_ne!(first.session_id(), second.session_id());
    }

    #[test]
    fn pending_pairings_are_bounded() {
        let mut auth = setup();
        auth.enable();
        let (_, workspace) = ids();
        for _ in 0..MAX_PENDING_PAIRINGS {
            auth.begin_pairing(workspace.clone()).unwrap();
        }
        assert!(matches!(
            auth.begin_pairing(workspace),
            Err(AuthError::CapacityReached)
        ));
    }

    #[test]
    fn oidc_device_requires_approval_and_revocation_blocks_reconnect() {
        let mut auth = setup();
        auth.enable();
        auth.enable_oidc(
            std::sync::Arc::new(crate::oidc::test_verifier()),
            OidcClientConfig {
                issuer: "https://idp.example".to_owned(),
                client_id: "lapis".to_owned(),
                scopes: vec!["openid".to_owned()],
            },
        );
        let hello = hello("web-1");
        let (_, workspace) = ids();
        let token = crate::oidc::test_token("alice", jsonwebtoken::get_current_timestamp() + 300);

        assert!(matches!(
            auth.authenticate_oidc(&token, &hello, &workspace, &CapabilitySet::default()),
            Err(AuthError::DeviceApprovalRequired)
        ));
        let device = auth
            .devices()
            .into_iter()
            .find(|device| device.client_id == "web-1")
            .unwrap();
        assert_eq!(device.status, DeviceStatus::Pending);

        assert!(auth.approve_device(&device.id));
        assert!(
            auth.authenticate_oidc(&token, &hello, &workspace, &CapabilitySet::default())
                .is_ok()
        );

        assert!(auth.revoke_device(&device.id));
        assert!(matches!(
            auth.authenticate_oidc(&token, &hello, &workspace, &CapabilitySet::default()),
            Err(AuthError::DeviceRevoked)
        ));
        assert!(!auth.approve_device("missing"));
        assert!(!auth.revoke_device("missing"));
    }

    #[test]
    fn token_authentication_registers_an_approved_device() {
        let mut auth = setup();
        auth.enable();
        let (client, workspace) = ids();
        let token = auth.begin_pairing(workspace.clone()).unwrap();
        let credential = auth
            .complete_pairing(&token, &client, &CapabilitySet::default())
            .unwrap();

        auth.authenticate(&credential, &client, &workspace).unwrap();

        let device = auth
            .devices()
            .into_iter()
            .find(|device| device.client_id == client.as_str())
            .unwrap();
        assert_eq!(device.status, DeviceStatus::Approved);
        assert!(device.approved_at.is_some());
    }

    #[test]
    fn revoking_a_token_device_invalidates_active_sessions_and_reconnect() {
        let mut auth = setup();
        auth.enable();
        let (client, workspace) = ids();
        let token = auth.begin_pairing(workspace.clone()).unwrap();
        let credential = auth
            .complete_pairing(&token, &client, &CapabilitySet::default())
            .unwrap();
        let session = auth.authenticate(&credential, &client, &workspace).unwrap();
        let device = auth
            .devices()
            .into_iter()
            .find(|device| device.client_id == client.as_str())
            .unwrap();

        assert!(auth.require_active_session(&session).is_ok());
        assert!(auth.revoke_device(&device.id));
        assert!(matches!(
            auth.require_active_session(&session),
            Err(AuthError::CredentialUnavailable)
        ));
        assert!(matches!(
            auth.authenticate(&credential, &client, &workspace),
            Err(AuthError::CredentialUnavailable)
        ));
    }

    #[test]
    fn rejected_pairing_does_not_leave_a_credential_for_a_revoked_device() {
        let mut auth = setup();
        let (_, workspace) = ids();
        auth.set_workspace_resolver(Arc::new(TestResolver::new(
            workspace.clone(),
            vec![workspace.clone()],
        )));
        auth.enable();
        let request = hello("revoked-web");
        let token = auth.begin_user_session_pairing(workspace.clone()).unwrap();
        auth.complete_pairing_and_authenticate(&token, &request, &CapabilitySet::default())
            .unwrap();
        let device = auth
            .devices()
            .into_iter()
            .find(|device| device.client_id == "revoked-web")
            .unwrap();
        assert!(auth.revoke_device(&device.id));
        let before = auth.export_credentials().len();
        let token = auth.begin_user_session_pairing(workspace).unwrap();
        assert!(matches!(
            auth.complete_pairing_and_authenticate(&token, &request, &CapabilitySet::default()),
            Err(AuthError::DeviceRevoked)
        ));
        assert_eq!(auth.export_credentials().len(), before);
    }

    #[test]
    fn pairing_persists_before_returning_credential_and_failure_disables_auth() {
        struct Store {
            saved: std::sync::Mutex<Option<(Vec<StoredCredential>, Vec<StoredDevice>)>>,
            fail: bool,
        }
        impl AuthPersistence for Store {
            fn save(
                &self,
                credentials: &[StoredCredential],
                devices: &[StoredDevice],
            ) -> Result<(), AuthPersistenceError> {
                if self.fail {
                    return Err(AuthPersistenceError);
                }
                *self.saved.lock().unwrap() = Some((credentials.to_vec(), devices.to_vec()));
                Ok(())
            }
        }
        let (_, workspace) = ids();
        let mut auth = setup();
        auth.set_workspace_resolver(Arc::new(TestResolver::new(
            workspace.clone(),
            vec![workspace.clone()],
        )));
        auth.enable();
        let store = Arc::new(Store {
            saved: std::sync::Mutex::new(None),
            fail: false,
        });
        auth.set_persistence(store.clone());
        let token = auth.begin_user_session_pairing(workspace.clone()).unwrap();
        let request = hello("persisted-web");
        let (credential, _) = auth
            .complete_pairing_and_authenticate(&token, &request, &CapabilitySet::default())
            .unwrap();
        let (credentials, devices) = store.saved.lock().unwrap().clone().unwrap();
        assert!(
            credentials
                .iter()
                .any(|item| item.id == credential.id().as_str())
        );
        assert!(devices.iter().any(|item| item.client_id == "persisted-web"));

        let mut restored = setup();
        restored.set_workspace_resolver(Arc::new(TestResolver::new(
            workspace.clone(),
            vec![workspace.clone()],
        )));
        restored.import_credentials(credentials);
        restored.import_devices(devices);
        restored.enable();
        assert!(
            restored
                .authenticate_auto(&credential, &request.client_id)
                .is_ok()
        );

        restored.set_persistence(store.clone());
        let device_id = restored
            .devices()
            .into_iter()
            .find(|device| device.client_id == "persisted-web")
            .unwrap()
            .id;
        assert!(restored.revoke_device_and_persist(&device_id).unwrap());
        let (credentials, devices) = store.saved.lock().unwrap().clone().unwrap();
        let mut after_revocation = setup();
        after_revocation.set_workspace_resolver(Arc::new(TestResolver::new(
            workspace.clone(),
            vec![workspace.clone()],
        )));
        after_revocation.import_credentials(credentials);
        after_revocation.import_devices(devices);
        after_revocation.enable();
        assert!(
            after_revocation
                .authenticate_auto(&credential, &request.client_id)
                .is_err()
        );

        restored.set_persistence(Arc::new(Store {
            saved: std::sync::Mutex::new(None),
            fail: true,
        }));
        let second = restored.begin_user_session_pairing(workspace).unwrap();
        assert!(matches!(
            restored.complete_pairing_and_authenticate(
                &second,
                &hello("other-web"),
                &CapabilitySet::default()
            ),
            Err(AuthError::PersistenceUnavailable)
        ));
        assert!(!restored.is_enabled());
    }

    #[test]
    fn devices_survive_restart_and_pending_remains_pending() {
        let mut auth = setup();
        auth.enable();
        auth.enable_oidc(
            std::sync::Arc::new(crate::oidc::test_verifier()),
            OidcClientConfig {
                issuer: "https://idp.example".to_owned(),
                client_id: "lapis".to_owned(),
                scopes: vec!["openid".to_owned()],
            },
        );
        let hello = hello("web-2");
        let (_, workspace) = ids();
        let token = crate::oidc::test_token("alice", jsonwebtoken::get_current_timestamp() + 300);
        assert!(
            auth.authenticate_oidc(&token, &hello, &workspace, &CapabilitySet::default())
                .is_err()
        );

        let exported = auth.export_devices();
        let mut restarted = setup();
        restarted.enable();
        restarted.import_devices(exported);

        let device = restarted
            .devices()
            .into_iter()
            .find(|device| device.client_id == "web-2")
            .unwrap();
        assert_eq!(device.status, DeviceStatus::Pending);
    }
}
