use std::{
    collections::HashMap,
    net::IpAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const MAX_TRACKED_ADDRESSES: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthenticationRateLimit {
    max_failures: u32,
    failure_window: Duration,
    block_duration: Duration,
}

impl AuthenticationRateLimit {
    pub fn new(
        max_failures: u32,
        failure_window: Duration,
        block_duration: Duration,
    ) -> Result<Self, AuthenticationRateLimitError> {
        if max_failures == 0 || failure_window.is_zero() || block_duration.is_zero() {
            return Err(AuthenticationRateLimitError);
        }
        Ok(Self {
            max_failures,
            failure_window,
            block_duration,
        })
    }

    pub const fn max_failures(self) -> u32 {
        self.max_failures
    }

    pub const fn failure_window(self) -> Duration {
        self.failure_window
    }

    pub const fn block_duration(self) -> Duration {
        self.block_duration
    }
}

impl Default for AuthenticationRateLimit {
    fn default() -> Self {
        Self {
            max_failures: 5,
            failure_window: Duration::from_secs(60),
            block_duration: Duration::from_secs(5 * 60),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthenticationRateLimitError;

impl std::fmt::Display for AuthenticationRateLimitError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("authentication rate limit values must be non-zero")
    }
}

impl std::error::Error for AuthenticationRateLimitError {}

#[derive(Clone)]
pub(crate) struct AuthenticationAttemptLimiter {
    config: AuthenticationRateLimit,
    attempts: Arc<Mutex<HashMap<IpAddr, FailureState>>>,
}

impl AuthenticationAttemptLimiter {
    pub(crate) fn new(config: AuthenticationRateLimit) -> Self {
        Self {
            config,
            attempts: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(crate) fn allows(&self, address: IpAddr, now: Instant) -> bool {
        let Ok(mut attempts) = self.attempts.lock() else {
            return false;
        };
        purge_inactive(&mut attempts, self.config, now);
        attempts
            .get(&address)
            .is_none_or(|state| state.blocked_until.is_none_or(|until| now >= until))
    }

    pub(crate) fn record_failure(&self, address: IpAddr, now: Instant) {
        let Ok(mut attempts) = self.attempts.lock() else {
            return;
        };
        purge_inactive(&mut attempts, self.config, now);
        if !attempts.contains_key(&address) && attempts.len() >= MAX_TRACKED_ADDRESSES {
            let oldest = attempts
                .iter()
                .min_by_key(|(_, state)| state.window_started)
                .map(|(address, _)| *address);
            if let Some(oldest) = oldest {
                attempts.remove(&oldest);
            }
        }
        let state = attempts.entry(address).or_insert(FailureState {
            window_started: now,
            failures: 0,
            blocked_until: None,
        });
        if now.duration_since(state.window_started) >= self.config.failure_window {
            state.window_started = now;
            state.failures = 0;
            state.blocked_until = None;
        }
        state.failures = state.failures.saturating_add(1);
        if state.failures >= self.config.max_failures {
            state.blocked_until = Some(now + self.config.block_duration);
        }
    }

    pub(crate) fn record_success(&self, address: IpAddr) {
        if let Ok(mut attempts) = self.attempts.lock() {
            attempts.remove(&address);
        }
    }
}

struct FailureState {
    window_started: Instant,
    failures: u32,
    blocked_until: Option<Instant>,
}

fn purge_inactive(
    attempts: &mut HashMap<IpAddr, FailureState>,
    config: AuthenticationRateLimit,
    now: Instant,
) {
    attempts.retain(|_, state| {
        state.blocked_until.is_some_and(|until| now < until)
            || now.duration_since(state.window_started) < config.failure_window
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_block_only_the_source_address_until_cooldown() {
        let config =
            AuthenticationRateLimit::new(2, Duration::from_secs(10), Duration::from_secs(30))
                .unwrap();
        let limiter = AuthenticationAttemptLimiter::new(config);
        let now = Instant::now();
        let first: IpAddr = "192.0.2.1".parse().unwrap();
        let second: IpAddr = "192.0.2.2".parse().unwrap();

        limiter.record_failure(first, now);
        assert!(limiter.allows(first, now));
        limiter.record_failure(first, now + Duration::from_secs(1));
        assert!(!limiter.allows(first, now + Duration::from_secs(2)));
        assert!(limiter.allows(second, now + Duration::from_secs(2)));
        assert!(limiter.allows(first, now + Duration::from_secs(32)));
    }

    #[test]
    fn success_and_elapsed_window_clear_failures() {
        let config =
            AuthenticationRateLimit::new(2, Duration::from_secs(10), Duration::from_secs(30))
                .unwrap();
        let limiter = AuthenticationAttemptLimiter::new(config);
        let now = Instant::now();
        let address: IpAddr = "192.0.2.1".parse().unwrap();

        limiter.record_failure(address, now);
        limiter.record_success(address);
        limiter.record_failure(address, now + Duration::from_secs(1));
        assert!(limiter.allows(address, now + Duration::from_secs(1)));
        assert!(limiter.allows(address, now + Duration::from_secs(12)));
    }

    #[test]
    fn zero_configuration_is_rejected() {
        assert!(
            AuthenticationRateLimit::new(0, Duration::from_secs(1), Duration::from_secs(1))
                .is_err()
        );
        assert!(AuthenticationRateLimit::new(1, Duration::ZERO, Duration::from_secs(1)).is_err());
        assert!(AuthenticationRateLimit::new(1, Duration::from_secs(1), Duration::ZERO).is_err());
    }
}
