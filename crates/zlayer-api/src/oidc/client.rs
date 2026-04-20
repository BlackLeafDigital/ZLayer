//! Thin wrapper around the `openidconnect` crate that takes an
//! [`OidcProviderConfig`] and exposes start-authorisation + callback-exchange
//! as two focused methods.
//!
//! Discovery is lazy: the `.well-known/openid-configuration` document is
//! fetched on the first call to [`OidcClient::core`] and cached via
//! `tokio::sync::OnceCell`. A provider whose discovery URL is unreachable at
//! boot does not prevent the daemon from starting; it will fail on the first
//! user sign-in attempt instead, so healthy providers keep working.

use std::sync::Arc;

use openidconnect::core::{CoreClient, CoreProviderMetadata, CoreResponseType};
use openidconnect::reqwest::async_http_client;
use openidconnect::{
    AuthenticationFlow, AuthorizationCode, ClientId, ClientSecret, CsrfToken, IssuerUrl, Nonce,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope,
};
use thiserror::Error;
use tokio::sync::OnceCell;
use tracing::{debug, warn};

use super::config::OidcProviderConfig;
use super::state::StateTokenPayload;

/// Profile we got from the identity provider after verifying the ID token.
#[derive(Debug, Clone)]
pub struct VerifiedIdentity {
    /// The provider slug (`google`, `okta`, ...).
    pub provider: String,
    /// The `sub` claim — stable opaque identifier for this user at this
    /// provider. This is what we key user-links off.
    pub subject: String,
    /// The `email` claim, if the provider returned one. Not all providers
    /// do, and the value should be treated as informational (spoofable by
    /// misconfigured providers — we still verify via the ID token signature,
    /// but a compromised provider can lie about email).
    pub email: Option<String>,
    /// The `name` / `preferred_username` claim, for initial display-name.
    pub display_name: Option<String>,
}

#[derive(Debug, Error)]
pub enum OidcError {
    #[error("OIDC discovery failed for provider {provider}: {source}")]
    Discovery {
        provider: String,
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("invalid URL in provider config: {0}")]
    InvalidUrl(String),

    #[error("token exchange failed: {0}")]
    TokenExchange(String),

    #[error("ID token missing from response")]
    MissingIdToken,

    #[error("ID token verification failed: {0}")]
    IdTokenVerification(String),

    #[error("unknown provider: {0}")]
    UnknownProvider(String),

    #[error("state token invalid or expired")]
    InvalidState,

    #[error("CSRF state mismatch")]
    CsrfMismatch,
}

/// Per-provider OIDC client with lazy discovery.
///
/// Hold one of these per configured provider. `Clone` is cheap — the inner
/// `CoreClient` lives behind an `Arc` once discovery has run.
#[derive(Debug, Clone)]
pub struct OidcClient {
    config: OidcProviderConfig,
    core: Arc<OnceCell<Arc<CoreClient>>>,
}

impl OidcClient {
    #[must_use]
    pub fn new(config: OidcProviderConfig) -> Self {
        Self {
            config,
            core: Arc::new(OnceCell::new()),
        }
    }

    #[must_use]
    pub fn config(&self) -> &OidcProviderConfig {
        &self.config
    }

    /// Lazy discovery. On first call, fetches the provider's
    /// `.well-known/openid-configuration` + JWKS and builds a `CoreClient`.
    async fn core(&self) -> Result<Arc<CoreClient>, OidcError> {
        if let Some(c) = self.core.get() {
            return Ok(c.clone());
        }

        let issuer = IssuerUrl::new(self.config.issuer.clone())
            .map_err(|e| OidcError::InvalidUrl(format!("issuer: {e}")))?;
        let metadata = CoreProviderMetadata::discover_async(issuer, async_http_client)
            .await
            .map_err(|e| OidcError::Discovery {
                provider: self.config.name.clone(),
                source: Box::new(e),
            })?;

        let redirect = RedirectUrl::new(self.config.redirect_url.clone())
            .map_err(|e| OidcError::InvalidUrl(format!("redirect: {e}")))?;

        let client = CoreClient::from_provider_metadata(
            metadata,
            ClientId::new(self.config.client_id.clone()),
            Some(ClientSecret::new(self.config.client_secret.clone())),
        )
        .set_redirect_uri(redirect);

        let client = Arc::new(client);
        let _ = self.core.set(client.clone());
        Ok(client)
    }

    /// Build an authorize URL for this provider. The returned payload must be
    /// handed to [`super::state::StateTokenStore::put`] and then passed back
    /// to [`Self::exchange_code`] on callback.
    pub async fn start_auth(&self) -> Result<StartedAuth, OidcError> {
        let client = self.core().await?;
        let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

        let mut builder = client.authorize_url(
            AuthenticationFlow::<CoreResponseType>::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        );

        for scope in &self.config.scopes {
            builder = builder.add_scope(Scope::new(scope.clone()));
        }
        builder = builder.set_pkce_challenge(pkce_challenge);

        let (auth_url, csrf_token, nonce) = builder.url();

        debug!(provider = %self.config.name, "OIDC authorize URL minted");

        Ok(StartedAuth {
            auth_url: auth_url.to_string(),
            csrf_token: csrf_token.secret().clone(),
            nonce: nonce.secret().clone(),
            pkce_verifier: pkce_verifier.secret().clone(),
        })
    }

    /// Exchange the authorisation code for tokens and verify the ID token.
    /// `state` must be the previously-stored [`StateTokenPayload`] retrieved
    /// via [`super::state::StateTokenStore::take`] — this function does not
    /// itself look up the state store.
    pub async fn exchange_code(
        &self,
        code: String,
        state: &StateTokenPayload,
    ) -> Result<VerifiedIdentity, OidcError> {
        if state.provider != self.config.name {
            warn!(
                expected = %self.config.name,
                got = %state.provider,
                "OIDC callback provider mismatch",
            );
            return Err(OidcError::CsrfMismatch);
        }

        let client = self.core().await?;
        let pkce_verifier = PkceCodeVerifier::new(state.pkce_verifier.clone());

        let token_response = client
            .exchange_code(AuthorizationCode::new(code))
            .set_pkce_verifier(pkce_verifier)
            .request_async(async_http_client)
            .await
            .map_err(|e| OidcError::TokenExchange(e.to_string()))?;

        let id_token = token_response
            .extra_fields()
            .id_token()
            .ok_or(OidcError::MissingIdToken)?;

        let nonce = Nonce::new(state.nonce.clone());
        let claims = id_token
            .claims(&client.id_token_verifier(), &nonce)
            .map_err(|e| OidcError::IdTokenVerification(e.to_string()))?;

        let subject = claims.subject().as_str().to_string();
        let email = claims.email().map(|e| e.as_str().to_string());
        let display_name = claims
            .name()
            .and_then(|localized| localized.get(None).map(|n| n.as_str().to_string()))
            .or_else(|| claims.preferred_username().map(|u| u.as_str().to_string()));

        Ok(VerifiedIdentity {
            provider: self.config.name.clone(),
            subject,
            email,
            display_name,
        })
    }
}

/// Output of [`OidcClient::start_auth`]. Hand the `csrf_token`/`nonce`/
/// `pkce_verifier` to the state store, and return the `auth_url` as a 302
/// Location header.
#[derive(Debug, Clone)]
pub struct StartedAuth {
    pub auth_url: String,
    pub csrf_token: String,
    pub nonce: String,
    pub pkce_verifier: String,
}
