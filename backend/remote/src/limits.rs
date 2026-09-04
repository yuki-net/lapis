use std::{error::Error, fmt, time::Duration};

use crate::AuthenticationRateLimit;

const MIN_FRAME_SIZE_BYTES: usize = 1024;
const DEFAULT_FRAME_SIZE_BYTES: usize = 1024 * 1024;
const MAX_FRAME_SIZE_BYTES: usize = 16 * 1024 * 1024;

const MIN_MESSAGE_SIZE_BYTES: usize = 1024;
const DEFAULT_MESSAGE_SIZE_BYTES: usize = 16 * 1024 * 1024;
const MAX_MESSAGE_SIZE_BYTES: usize = 64 * 1024 * 1024;

const MIN_AUTHENTICATION_TIMEOUT: Duration = Duration::from_secs(1);
const DEFAULT_AUTHENTICATION_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_AUTHENTICATION_TIMEOUT: Duration = Duration::from_secs(60);

const MIN_CONCURRENT_REQUESTS: usize = 1;
const DEFAULT_CONCURRENT_REQUESTS: usize = 1;
const MAX_CONCURRENT_REQUESTS: usize = 1;

const MIN_IDLE_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const MAX_IDLE_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);

const MIN_REQUEST_TIMEOUT: Duration = Duration::from_secs(1);
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// 1 frameあたりに受け付けるpayloadの上限。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaxFrameSize(usize);

impl MaxFrameSize {
    pub const MIN_BYTES: usize = MIN_FRAME_SIZE_BYTES;
    pub const DEFAULT_BYTES: usize = DEFAULT_FRAME_SIZE_BYTES;
    pub const MAX_BYTES: usize = MAX_FRAME_SIZE_BYTES;

    pub fn new(bytes: usize) -> Result<Self, LimitError> {
        if (Self::MIN_BYTES..=Self::MAX_BYTES).contains(&bytes) {
            Ok(Self(bytes))
        } else {
            Err(LimitError::FrameSizeOutOfRange)
        }
    }

    pub const fn bytes(self) -> usize {
        self.0
    }
}

impl Default for MaxFrameSize {
    fn default() -> Self {
        Self(Self::DEFAULT_BYTES)
    }
}

/// 複数frameへ分割された1 message全体の上限。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaxMessageSize(usize);

impl MaxMessageSize {
    pub const MIN_BYTES: usize = MIN_MESSAGE_SIZE_BYTES;
    pub const DEFAULT_BYTES: usize = DEFAULT_MESSAGE_SIZE_BYTES;
    pub const MAX_BYTES: usize = MAX_MESSAGE_SIZE_BYTES;

    pub fn new(bytes: usize) -> Result<Self, LimitError> {
        if (Self::MIN_BYTES..=Self::MAX_BYTES).contains(&bytes) {
            Ok(Self(bytes))
        } else {
            Err(LimitError::MessageSizeOutOfRange)
        }
    }

    pub const fn bytes(self) -> usize {
        self.0
    }
}

impl Default for MaxMessageSize {
    fn default() -> Self {
        Self(Self::DEFAULT_BYTES)
    }
}

/// 認証処理を完了させるまでの上限時間。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthenticationTimeout(Duration);

impl AuthenticationTimeout {
    pub const MIN: Duration = MIN_AUTHENTICATION_TIMEOUT;
    pub const DEFAULT: Duration = DEFAULT_AUTHENTICATION_TIMEOUT;
    pub const MAX: Duration = MAX_AUTHENTICATION_TIMEOUT;

    pub fn new(duration: Duration) -> Result<Self, LimitError> {
        if (Self::MIN..=Self::MAX).contains(&duration) {
            Ok(Self(duration))
        } else {
            Err(LimitError::AuthenticationTimeoutOutOfRange)
        }
    }

    pub const fn duration(self) -> Duration {
        self.0
    }
}

impl Default for AuthenticationTimeout {
    fn default() -> Self {
        Self(Self::DEFAULT)
    }
}

/// 1 clientで処理中にできるrequest数の上限。Phase 1 transportは順序保証のため直列処理する。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaxConcurrentRequests(usize);

impl MaxConcurrentRequests {
    pub const MIN: usize = MIN_CONCURRENT_REQUESTS;
    pub const DEFAULT: usize = DEFAULT_CONCURRENT_REQUESTS;
    pub const MAX: usize = MAX_CONCURRENT_REQUESTS;

    pub fn new(count: usize) -> Result<Self, LimitError> {
        if (Self::MIN..=Self::MAX).contains(&count) {
            Ok(Self(count))
        } else {
            Err(LimitError::ConcurrentRequestsOutOfRange)
        }
    }

    pub const fn get(self) -> usize {
        self.0
    }
}

impl Default for MaxConcurrentRequests {
    fn default() -> Self {
        Self(Self::DEFAULT)
    }
}

/// 接続を維持するために通信がない状態を許容する上限時間。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IdleTimeout(Duration);

impl IdleTimeout {
    pub const MIN: Duration = MIN_IDLE_TIMEOUT;
    pub const DEFAULT: Duration = DEFAULT_IDLE_TIMEOUT;
    pub const MAX: Duration = MAX_IDLE_TIMEOUT;

    pub fn new(duration: Duration) -> Result<Self, LimitError> {
        if (Self::MIN..=Self::MAX).contains(&duration) {
            Ok(Self(duration))
        } else {
            Err(LimitError::IdleTimeoutOutOfRange)
        }
    }

    pub const fn duration(self) -> Duration {
        self.0
    }
}

impl Default for IdleTimeout {
    fn default() -> Self {
        Self(Self::DEFAULT)
    }
}

/// backendが1 requestを処理する上限時間。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RequestTimeout(Duration);

impl RequestTimeout {
    pub const MIN: Duration = MIN_REQUEST_TIMEOUT;
    pub const DEFAULT: Duration = DEFAULT_REQUEST_TIMEOUT;
    pub const MAX: Duration = MAX_REQUEST_TIMEOUT;

    pub fn new(duration: Duration) -> Result<Self, LimitError> {
        if (Self::MIN..=Self::MAX).contains(&duration) {
            Ok(Self(duration))
        } else {
            Err(LimitError::RequestTimeoutOutOfRange)
        }
    }

    pub const fn duration(self) -> Duration {
        self.0
    }
}

impl Default for RequestTimeout {
    fn default() -> Self {
        Self(Self::DEFAULT)
    }
}

/// remote transportが共有する制限値。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RemoteLimits {
    max_frame_size: MaxFrameSize,
    max_message_size: MaxMessageSize,
    authentication_timeout: AuthenticationTimeout,
    max_concurrent_requests: MaxConcurrentRequests,
    idle_timeout: IdleTimeout,
    request_timeout: RequestTimeout,
    authentication_rate_limit: AuthenticationRateLimit,
}

impl RemoteLimits {
    pub fn new(
        max_frame_size: MaxFrameSize,
        max_message_size: MaxMessageSize,
        authentication_timeout: AuthenticationTimeout,
        max_concurrent_requests: MaxConcurrentRequests,
        idle_timeout: IdleTimeout,
        request_timeout: RequestTimeout,
        authentication_rate_limit: AuthenticationRateLimit,
    ) -> Result<Self, LimitError> {
        if max_frame_size.bytes() > max_message_size.bytes() {
            return Err(LimitError::FrameExceedsMessage);
        }
        Ok(Self {
            max_frame_size,
            max_message_size,
            authentication_timeout,
            max_concurrent_requests,
            idle_timeout,
            request_timeout,
            authentication_rate_limit,
        })
    }

    pub const fn max_frame_size(self) -> MaxFrameSize {
        self.max_frame_size
    }

    pub const fn max_message_size(self) -> MaxMessageSize {
        self.max_message_size
    }

    pub const fn authentication_timeout(self) -> AuthenticationTimeout {
        self.authentication_timeout
    }

    pub const fn max_concurrent_requests(self) -> MaxConcurrentRequests {
        self.max_concurrent_requests
    }

    pub const fn idle_timeout(self) -> IdleTimeout {
        self.idle_timeout
    }

    pub const fn request_timeout(self) -> RequestTimeout {
        self.request_timeout
    }

    pub const fn authentication_rate_limit(self) -> AuthenticationRateLimit {
        self.authentication_rate_limit
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimitError {
    FrameSizeOutOfRange,
    MessageSizeOutOfRange,
    AuthenticationTimeoutOutOfRange,
    ConcurrentRequestsOutOfRange,
    IdleTimeoutOutOfRange,
    RequestTimeoutOutOfRange,
    FrameExceedsMessage,
}

impl fmt::Display for LimitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::FrameSizeOutOfRange => "maximum frame size is outside the allowed range",
            Self::MessageSizeOutOfRange => "maximum message size is outside the allowed range",
            Self::AuthenticationTimeoutOutOfRange => {
                "authentication timeout is outside the allowed range"
            }
            Self::ConcurrentRequestsOutOfRange => {
                "maximum concurrent requests is outside the allowed range"
            }
            Self::IdleTimeoutOutOfRange => "idle timeout is outside the allowed range",
            Self::RequestTimeoutOutOfRange => "request timeout is outside the allowed range",
            Self::FrameExceedsMessage => "maximum frame size exceeds maximum message size",
        };
        formatter.write_str(message)
    }
}

impl Error for LimitError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_size_accepts_boundaries_and_rejects_invalid_values() {
        assert_eq!(
            MaxFrameSize::new(MaxFrameSize::MIN_BYTES).unwrap().bytes(),
            MaxFrameSize::MIN_BYTES
        );
        assert_eq!(
            MaxFrameSize::new(MaxFrameSize::MAX_BYTES).unwrap().bytes(),
            MaxFrameSize::MAX_BYTES
        );
        assert_eq!(
            MaxFrameSize::new(MaxFrameSize::MIN_BYTES - 1),
            Err(LimitError::FrameSizeOutOfRange)
        );
        assert_eq!(
            MaxFrameSize::new(MaxFrameSize::MAX_BYTES + 1),
            Err(LimitError::FrameSizeOutOfRange)
        );
    }

    #[test]
    fn message_size_accepts_boundaries_and_rejects_invalid_values() {
        assert!(MaxMessageSize::new(MaxMessageSize::MIN_BYTES).is_ok());
        assert!(MaxMessageSize::new(MaxMessageSize::MAX_BYTES).is_ok());
        assert_eq!(
            MaxMessageSize::new(MaxMessageSize::MIN_BYTES - 1),
            Err(LimitError::MessageSizeOutOfRange)
        );
        assert_eq!(
            MaxMessageSize::new(MaxMessageSize::MAX_BYTES + 1),
            Err(LimitError::MessageSizeOutOfRange)
        );
    }

    #[test]
    fn authentication_timeout_accepts_boundaries_and_rejects_invalid_values() {
        assert!(AuthenticationTimeout::new(AuthenticationTimeout::MIN).is_ok());
        assert!(AuthenticationTimeout::new(AuthenticationTimeout::MAX).is_ok());
        assert_eq!(
            AuthenticationTimeout::new(Duration::ZERO),
            Err(LimitError::AuthenticationTimeoutOutOfRange)
        );
        assert_eq!(
            AuthenticationTimeout::new(AuthenticationTimeout::MAX + Duration::from_nanos(1)),
            Err(LimitError::AuthenticationTimeoutOutOfRange)
        );
    }

    #[test]
    fn concurrent_requests_accepts_boundaries_and_rejects_invalid_values() {
        assert_eq!(
            MaxConcurrentRequests::new(MaxConcurrentRequests::MIN)
                .unwrap()
                .get(),
            MaxConcurrentRequests::MIN
        );
        assert_eq!(
            MaxConcurrentRequests::new(MaxConcurrentRequests::MAX)
                .unwrap()
                .get(),
            MaxConcurrentRequests::MAX
        );
        assert_eq!(
            MaxConcurrentRequests::new(0),
            Err(LimitError::ConcurrentRequestsOutOfRange)
        );
        assert_eq!(
            MaxConcurrentRequests::new(MaxConcurrentRequests::MAX + 1),
            Err(LimitError::ConcurrentRequestsOutOfRange)
        );
    }

    #[test]
    fn idle_and_request_timeouts_accept_boundaries_and_reject_invalid_values() {
        assert!(IdleTimeout::new(IdleTimeout::MIN).is_ok());
        assert!(IdleTimeout::new(IdleTimeout::MAX).is_ok());
        assert_eq!(
            IdleTimeout::new(IdleTimeout::MIN - Duration::from_nanos(1)),
            Err(LimitError::IdleTimeoutOutOfRange)
        );
        assert_eq!(
            IdleTimeout::new(IdleTimeout::MAX + Duration::from_nanos(1)),
            Err(LimitError::IdleTimeoutOutOfRange)
        );

        assert!(RequestTimeout::new(RequestTimeout::MIN).is_ok());
        assert!(RequestTimeout::new(RequestTimeout::MAX).is_ok());
        assert_eq!(
            RequestTimeout::new(RequestTimeout::MIN - Duration::from_nanos(1)),
            Err(LimitError::RequestTimeoutOutOfRange)
        );
        assert_eq!(
            RequestTimeout::new(RequestTimeout::MAX + Duration::from_nanos(1)),
            Err(LimitError::RequestTimeoutOutOfRange)
        );
    }

    #[test]
    fn defaults_form_a_valid_remote_limit_set() {
        let limits = RemoteLimits::default();

        assert_eq!(limits.max_frame_size().bytes(), MaxFrameSize::DEFAULT_BYTES);
        assert_eq!(
            limits.max_message_size().bytes(),
            MaxMessageSize::DEFAULT_BYTES
        );
        assert_eq!(
            limits.authentication_timeout().duration(),
            AuthenticationTimeout::DEFAULT
        );
        assert_eq!(
            limits.max_concurrent_requests().get(),
            MaxConcurrentRequests::DEFAULT
        );
        assert_eq!(limits.idle_timeout().duration(), IdleTimeout::DEFAULT);
        assert_eq!(limits.request_timeout().duration(), RequestTimeout::DEFAULT);
        assert_eq!(
            limits.authentication_rate_limit(),
            AuthenticationRateLimit::default()
        );
    }

    #[test]
    fn frame_must_not_exceed_complete_message_size() {
        assert_eq!(
            RemoteLimits::new(
                MaxFrameSize::new(2048).unwrap(),
                MaxMessageSize::new(1024).unwrap(),
                AuthenticationTimeout::default(),
                MaxConcurrentRequests::default(),
                IdleTimeout::default(),
                RequestTimeout::default(),
                AuthenticationRateLimit::default(),
            ),
            Err(LimitError::FrameExceedsMessage)
        );
    }
}
