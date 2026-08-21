use std::{collections::HashMap, error::Error, fmt, marker::PhantomData};

use lapis_client_api::{CapabilitySet, ClientId, RequestBody, SessionId, WorkspaceId};

use crate::{
    authorization::{AuthorizationError, SessionGrant},
    clock::{Clock, SystemClock},
    credential::{CredentialHandle, CredentialId, CredentialRecord},
    pairing::{PairingToken, secret_from_token},
    random::{OsRandom, RandomError, RandomSource},
    secret::{SECRET_BYTES, constant_time_digest_matches, digest},
};

const MAX_PENDING_PAIRINGS: usize = 8;
const MAX_CREDENTIALS: usize = 64;

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
}

impl AuthConfig {
    pub fn new(pairing_lifetime: PairingLifetime, credential_lifetime: CredentialLifetime) -> Self {
        Self {
            pairing_lifetime,
            credential_lifetime,
        }
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
    CapacityReached,
    Random(RandomError),
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
            Self::CapacityReached => formatter.write_str("authentication capacity reached"),
            Self::Random(_) => formatter.write_str("authentication secret generation failed"),
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
}

/// Pairing、credential、session grantのライフサイクルを所有する認証サービス。
pub struct RemoteAuth<C = SystemClock, R = OsRandom> {
    clock: C,
    random: R,
    config: AuthConfig,
    policy: AuthPolicy,
    enabled: bool,
    pairings: HashMap<[u8; SECRET_BYTES], PendingPairing>,
    credentials: HashMap<CredentialId, CredentialRecord>,
    sessions: HashMap<SessionId, CredentialId>,
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
            enabled: false,
            pairings: HashMap::new(),
            credentials: HashMap::new(),
            sessions: HashMap::new(),
            _marker: PhantomData,
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
        let record = CredentialRecord {
            client_id: client_id.clone(),
            workspace_id: pairing.workspace_id,
            capabilities: requested_capabilities.intersection(self.policy.allowed_capabilities()),
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
        client_id: &ClientId,
        requested_capabilities: &CapabilitySet,
    ) -> Result<(CredentialHandle, SessionGrant), AuthError> {
        let credential = self.complete_pairing(token, client_id, requested_capabilities)?;
        let workspace_id = self
            .credentials
            .get(credential.id())
            .ok_or(AuthError::CredentialUnavailable)?
            .workspace_id
            .clone();
        match self.authenticate(&credential, client_id, &workspace_id) {
            Ok(grant) => Ok((credential, grant)),
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
        self.require_enabled()?;
        let record = self
            .credentials
            .get(credential.id())
            .ok_or(AuthError::CredentialUnavailable)?;
        if !constant_time_digest_matches(credential.secret_bytes(), &record.digest) {
            return Err(AuthError::CredentialUnavailable);
        }
        if &record.client_id != client_id || &record.workspace_id != workspace_id {
            return Err(AuthError::CredentialBindingMismatch);
        }
        let now = self.clock.now_unix_seconds();
        if now >= record.expires_at {
            return Err(AuthError::CredentialExpired);
        }
        let granted_workspace = record.workspace_id.clone();
        let granted_capabilities = record.capabilities.clone();
        let session_id = self.generate_session_id()?;
        let grant = SessionGrant::from_authenticated(
            session_id.clone(),
            granted_workspace,
            granted_capabilities,
        );
        self.sessions.insert(session_id, credential.id().clone());
        Ok(grant)
    }

    pub fn revoke(&mut self, credential_id: &CredentialId) -> bool {
        let removed = self.credentials.remove(credential_id).is_some();
        if removed {
            self.sessions
                .retain(|_, session_credential| session_credential != credential_id);
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
            grant
                .require_workspace(workspace_id)
                .map_err(AccessError::Authorization)?;
        }
        grant
            .require_request(request)
            .map_err(AccessError::Authorization)
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

    fn require_active_session(&self, grant: &SessionGrant) -> Result<(), AuthError> {
        self.require_enabled()?;
        let credential_id = self
            .sessions
            .get(grant.session_id())
            .ok_or(AuthError::CredentialUnavailable)?;
        let record = self
            .credentials
            .get(credential_id)
            .ok_or(AuthError::CredentialUnavailable)?;
        if self.clock.now_unix_seconds() >= record.expires_at {
            return Err(AuthError::CredentialExpired);
        }
        Ok(())
    }

    fn purge_expired_credentials(&mut self) {
        let now = self.clock.now_unix_seconds();
        self.credentials
            .retain(|_, credential| credential.expires_at > now);
        self.sessions
            .retain(|_, credential_id| self.credentials.contains_key(credential_id));
    }

    fn generate_session_id(&mut self) -> Result<SessionId, AuthError> {
        let id = CredentialId::generate(&mut self.random).map_err(AuthError::Random)?;
        SessionId::try_new(format!("session-{}", id.as_str()))
            .map_err(|_| AuthError::CredentialUnavailable)
    }

    fn generate_credential_id(&mut self) -> Result<CredentialId, AuthError> {
        for _ in 0..4 {
            let id = CredentialId::generate(&mut self.random).map_err(AuthError::Random)?;
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
    use lapis_client_api::{CapabilityId, capability};

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
        let files_request = RequestBody::FileTree(lapis_client_api::FileTreeRequest {
            workspace_id: workspace.clone(),
            path: None,
        });
        let terminal_request = RequestBody::TerminalStart(lapis_client_api::TerminalStartRequest {
            workspace_id: workspace.clone(),
            cwd: None,
            command: None,
            size: lapis_client_api::TerminalSize {
                columns: 80,
                rows: 24,
            },
        });
        assert!(auth.authorize_request(&grant, &files_request).is_ok());
        assert!(matches!(
            auth.authorize_request(&grant, &terminal_request),
            Err(AccessError::Authorization(_))
        ));
        let other_workspace_request = RequestBody::FileTree(lapis_client_api::FileTreeRequest {
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
}
