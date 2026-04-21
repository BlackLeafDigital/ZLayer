//! In-memory state store for in-flight OIDC authorisation requests.
//!
//! Between `/auth/oidc/:provider/start` (which mints a CSRF + nonce + PKCE
//! verifier and redirects the user to the provider) and the provider's
//! callback hitting `/auth/oidc/:provider/callback`, we need to remember
//! those values so we can verify the returned `state` parameter and exchange
//! the PKCE verifier for tokens.
//!
//! Entries expire after [`DEFAULT_TTL`] (default: 10 minutes). Each `take`
//! consumes the entry so replays are rejected.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use tracing::debug;

/// Default lifetime for a pending OIDC authorisation. 10 minutes matches
/// typical user attention span between clicking "Sign in with X" and finishing
/// the consent flow.
pub const DEFAULT_TTL: Duration = Duration::from_secs(600);

/// Captured data for a single in-flight authorisation.
#[derive(Debug, Clone)]
pub struct StateTokenPayload {
    /// Provider slug the user is authenticating against.
    pub provider: String,
    /// CSRF token (same value as the `state` query param returned in the
    /// redirect). Kept alongside the payload as a defence-in-depth check.
    pub csrf_token: String,
    /// Nonce to verify in the ID token's `nonce` claim — binds the id_token
    /// to this specific auth request.
    pub nonce: String,
    /// PKCE code verifier — sent with the token exchange to prove this
    /// request originated the authorize_url.
    pub pkce_verifier: String,
    /// Wall-clock expiry derived from TTL.
    expires_at: Instant,
}

impl StateTokenPayload {
    #[must_use]
    pub fn is_expired(&self) -> bool {
        Instant::now() >= self.expires_at
    }
}

/// Thread-safe store keyed by the CSRF/state token.
///
/// Cheap to clone (internal `Arc`). Clone it freely into handler state.
#[derive(Debug, Clone, Default)]
pub struct StateTokenStore {
    inner: Arc<Mutex<HashMap<String, StateTokenPayload>>>,
    ttl: Duration,
}

impl StateTokenStore {
    #[must_use]
    pub fn new() -> Self {
        Self::with_ttl(DEFAULT_TTL)
    }

    #[must_use]
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            ttl,
        }
    }

    /// Store a new pending authorisation. Returns the state token string —
    /// this is the value that must go into the `state` query parameter of the
    /// authorize URL.
    pub async fn put(
        &self,
        provider: impl Into<String>,
        csrf_token: impl Into<String>,
        nonce: impl Into<String>,
        pkce_verifier: impl Into<String>,
    ) -> String {
        let csrf = csrf_token.into();
        let payload = StateTokenPayload {
            provider: provider.into(),
            csrf_token: csrf.clone(),
            nonce: nonce.into(),
            pkce_verifier: pkce_verifier.into(),
            expires_at: Instant::now() + self.ttl,
        };

        let mut map = self.inner.lock().await;
        Self::evict_expired(&mut map);
        map.insert(csrf.clone(), payload);
        debug!(state_token = %csrf, "OIDC state stored");
        csrf
    }

    /// Consume the entry for `state_token`. Returns `None` if missing or
    /// expired. A successful `take` removes the entry so a replay of the same
    /// `state` value cannot succeed.
    pub async fn take(&self, state_token: &str) -> Option<StateTokenPayload> {
        let mut map = self.inner.lock().await;
        Self::evict_expired(&mut map);
        let payload = map.remove(state_token)?;
        if payload.is_expired() {
            debug!(state_token = %state_token, "OIDC state expired at take");
            return None;
        }
        Some(payload)
    }

    /// Number of live entries. Primarily for tests / introspection.
    pub async fn len(&self) -> usize {
        let mut map = self.inner.lock().await;
        Self::evict_expired(&mut map);
        map.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }

    fn evict_expired(map: &mut HashMap<String, StateTokenPayload>) {
        let now = Instant::now();
        map.retain(|_, v| v.expires_at > now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn put_then_take_round_trips() {
        let store = StateTokenStore::new();
        let token = store
            .put("google", "csrf-123", "nonce-abc", "pkce-xyz")
            .await;
        assert_eq!(token, "csrf-123");

        let payload = store.take(&token).await.expect("present");
        assert_eq!(payload.provider, "google");
        assert_eq!(payload.nonce, "nonce-abc");
        assert_eq!(payload.pkce_verifier, "pkce-xyz");
    }

    #[tokio::test]
    async fn take_twice_returns_none_on_replay() {
        let store = StateTokenStore::new();
        let token = store.put("g", "csrf1", "n1", "p1").await;
        assert!(store.take(&token).await.is_some());
        assert!(
            store.take(&token).await.is_none(),
            "replay must be rejected"
        );
    }

    #[tokio::test]
    async fn expired_entry_is_not_returned() {
        let store = StateTokenStore::with_ttl(Duration::from_millis(10));
        let token = store.put("g", "csrf2", "n2", "p2").await;
        tokio::time::sleep(Duration::from_millis(25)).await;
        assert!(store.take(&token).await.is_none());
        assert_eq!(store.len().await, 0, "expired entries are evicted");
    }

    #[tokio::test]
    async fn unknown_token_returns_none() {
        let store = StateTokenStore::new();
        assert!(store.take("bogus").await.is_none());
    }
}
