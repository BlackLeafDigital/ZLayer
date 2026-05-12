//! Identity management facade.
//!
//! Coordinates the two independent stores behind a user account: the
//! [`UserStorage`] (profile row in `users.db`) and the
//! [`zlayer_secrets::CredentialStore`] (Argon2id hash keyed by email, in the
//! encrypted secrets DB). Callers MUST go through [`IdentityManager`] for
//! lifecycle mutations — `create_user`, `delete_user`, `set_role` — so the
//! two stores never drift.
//!
//! Read paths (`get`, `list`, `count`, `get_by_email`) remain on
//! [`UserStorage`] directly; only combined writes route through this facade.

use std::sync::Arc;

use thiserror::Error;
use tracing::{error, info, warn};

use crate::storage::{
    OidcIdentity, OidcIdentityStorage, StorageError, StoredUser, UserRole, UserStorage,
};
use zlayer_secrets::{CredentialStore, PersistentSecretsStore, SecretsError};

/// Error surface for [`IdentityManager`] operations.
#[derive(Debug, Error)]
pub enum IdentityError {
    /// User store rejected the operation.
    #[error("user store: {0}")]
    UserStore(#[from] StorageError),

    /// Credential store (secrets DB) rejected the operation.
    #[error("credential store: {0}")]
    Credentials(#[from] SecretsError),

    /// Lookup returned no row for the provided id/email.
    #[error("user {0} not found")]
    NotFound(String),

    /// Uniqueness conflict (an existing row already has this email).
    #[error("email {0} already in use")]
    EmailExists(String),

    /// Caller supplied a structurally invalid argument.
    #[error("invalid argument: {0}")]
    Invalid(String),
}

/// Combined lifecycle operations across the user store and the credential
/// store. Cheap to clone — internal state is `Arc`s.
#[derive(Clone)]
pub struct IdentityManager {
    users: Arc<dyn UserStorage>,
    credentials: Arc<CredentialStore<Arc<PersistentSecretsStore>>>,
    /// OIDC identity links. `None` when OIDC is disabled (no providers
    /// configured); `find_or_create_by_external_id` returns an error in
    /// that case.
    oidc_identities: Option<Arc<dyn OidcIdentityStorage>>,
}

impl std::fmt::Debug for IdentityManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IdentityManager").finish()
    }
}

impl IdentityManager {
    /// Build a new manager from the two core stores. OIDC identity linking
    /// is disabled — use [`Self::with_oidc`] to enable it.
    #[must_use]
    pub fn new(
        users: Arc<dyn UserStorage>,
        credentials: Arc<CredentialStore<Arc<PersistentSecretsStore>>>,
    ) -> Self {
        Self {
            users,
            credentials,
            oidc_identities: None,
        }
    }

    /// Enable OIDC identity linking. `oidc_identities` is the persistence
    /// backend for `(provider, subject) → user_id` mappings.
    #[must_use]
    pub fn with_oidc(mut self, oidc_identities: Arc<dyn OidcIdentityStorage>) -> Self {
        self.oidc_identities = Some(oidc_identities);
        self
    }

    /// Read-only handle to the user store (for `list`, `get`, `get_by_email`).
    #[must_use]
    pub fn users(&self) -> &Arc<dyn UserStorage> {
        &self.users
    }

    /// Read-only handle to the credential store (for `validate`,
    /// `set_password` passthroughs — anything that touches only the
    /// credential hash, not both stores).
    #[must_use]
    pub fn credentials(&self) -> &Arc<CredentialStore<Arc<PersistentSecretsStore>>> {
        &self.credentials
    }

    /// Create a new user account: writes the profile row, then the credential.
    /// On credential failure, the profile row is rolled back so the two stores
    /// stay consistent. `email` is lowercased and trimmed.
    ///
    /// # Errors
    ///
    /// * [`IdentityError::Invalid`] when email or password is empty.
    /// * [`IdentityError::EmailExists`] when a user with this email exists.
    /// * [`IdentityError::UserStore`] / [`IdentityError::Credentials`] on
    ///   backing-store failures.
    pub async fn create_user(
        &self,
        email: &str,
        display_name: impl Into<String>,
        role: UserRole,
        password: &str,
    ) -> Result<StoredUser, IdentityError> {
        let email_lc = email.trim().to_lowercase();
        if email_lc.is_empty() {
            return Err(IdentityError::Invalid("email is required".to_string()));
        }
        if password.is_empty() {
            return Err(IdentityError::Invalid("password is required".to_string()));
        }

        if self.users.get_by_email(&email_lc).await?.is_some() {
            return Err(IdentityError::EmailExists(email_lc));
        }

        let user = StoredUser::new(email_lc.clone(), display_name, role);
        self.users.store(&user).await?;

        if let Err(e) = self
            .credentials
            .create_api_key(&email_lc, password, &[role.as_str()])
            .await
        {
            // Roll back the user row so we don't leave an orphan that blocks
            // future re-creation of the same email.
            if let Err(rollback) = self.users.delete(&user.id).await {
                error!(
                    user_id = %user.id,
                    email = %email_lc,
                    credential_err = %e,
                    rollback_err = %rollback,
                    "IdentityManager::create_user: credential write failed AND rollback failed — user row orphaned"
                );
            } else {
                warn!(
                    user_id = %user.id,
                    email = %email_lc,
                    error = %e,
                    "IdentityManager::create_user: credential write failed; rolled back user row"
                );
            }
            return Err(IdentityError::Credentials(e));
        }

        info!(user_id = %user.id, email = %email_lc, role = %role, "IdentityManager: user created");
        Ok(user)
    }

    /// Remove a user account: deletes the credential hash (idempotent — skips
    /// if absent so stale rows don't block cleanup), then the profile row.
    /// Idempotent — returns `Ok(None)` if no row has this id; returns
    /// `Ok(Some(user))` when a row was actually deleted.
    ///
    /// # Errors
    ///
    /// * [`IdentityError::UserStore`] / [`IdentityError::Credentials`] on
    ///   backing-store failures. Credential deletion happens first — if it
    ///   fails, the user row is left intact so the caller can retry.
    pub async fn delete_user(&self, id: &str) -> Result<Option<StoredUser>, IdentityError> {
        let Some(user) = self.users.get(id).await? else {
            return Ok(None);
        };

        if self.credentials.exists(&user.email).await? {
            self.credentials.delete_api_key(&user.email).await?;
        }

        let deleted = self.users.delete(id).await?;
        if !deleted {
            return Ok(None);
        }

        info!(user_id = %user.id, email = %user.email, "IdentityManager: user deleted");
        Ok(Some(user))
    }

    /// Change a user's role in both the profile row AND the credential's
    /// roles array, so login-time role checks and token issuance stay
    /// consistent. Returns the updated user.
    ///
    /// # Errors
    ///
    /// * [`IdentityError::NotFound`] when no row has this id.
    /// * [`IdentityError::UserStore`] / [`IdentityError::Credentials`] on
    ///   backing-store failures. User row is updated first; if credential
    ///   role update fails, the user row has already committed — log the
    ///   drift loudly so operators can detect it.
    pub async fn set_role(
        &self,
        id: &str,
        new_role: UserRole,
    ) -> Result<StoredUser, IdentityError> {
        let mut user = self
            .users
            .get(id)
            .await?
            .ok_or_else(|| IdentityError::NotFound(id.to_string()))?;

        user.role = new_role;
        user.updated_at = chrono::Utc::now();
        self.users.store(&user).await?;

        // The credential may not exist if this user was created out-of-band
        // (shouldn't happen through IdentityManager, but handle it).
        if self.credentials.exists(&user.email).await? {
            if let Err(e) = self
                .credentials
                .set_roles(&user.email, &[new_role.as_str()])
                .await
            {
                error!(
                    user_id = %user.id,
                    email = %user.email,
                    error = %e,
                    "IdentityManager::set_role: user row committed but credential roles update failed — roles have drifted"
                );
                return Err(IdentityError::Credentials(e));
            }
        } else {
            warn!(
                user_id = %user.id,
                email = %user.email,
                "IdentityManager::set_role: no credential row for this user; only user store updated"
            );
        }

        info!(user_id = %user.id, email = %user.email, role = %new_role, "IdentityManager: role updated");
        Ok(user)
    }

    /// Resolve an OIDC `(provider, subject)` pair to a `ZLayer` user. On first
    /// sign-in, creates a passwordless user row (no credential-store entry)
    /// and the OIDC link row in a single atomic pair of writes. On repeat
    /// sign-ins, returns the linked user.
    ///
    /// `email_hint` / `display_name_hint` are used only when creating a new
    /// user. Existing users keep their ZLayer-side profile — provider-side
    /// email changes do not overwrite the local record.
    ///
    /// # Errors
    ///
    /// * [`IdentityError::Invalid`] when OIDC is not configured on this
    ///   manager (no [`OidcIdentityStorage`] was supplied).
    pub async fn find_or_create_by_external_id(
        &self,
        provider: &str,
        subject: &str,
        email_hint: Option<&str>,
        display_name_hint: Option<&str>,
    ) -> Result<StoredUser, IdentityError> {
        let oidc = self.oidc_identities.as_ref().ok_or_else(|| {
            IdentityError::Invalid(
                "OIDC identity storage not configured on this IdentityManager".to_string(),
            )
        })?;

        if let Some(link) = oidc.get_by_external(provider, subject).await? {
            return self
                .users
                .get(&link.user_id)
                .await?
                .ok_or_else(|| IdentityError::NotFound(link.user_id.clone()));
        }

        // Best-effort: if the provider's email already matches an existing
        // local user, link to that account instead of creating a duplicate.
        // Local-first policy — we do NOT silently downgrade the local user's
        // credentials or role.
        let email_lc = email_hint
            .map(|e| e.trim().to_lowercase())
            .filter(|e| !e.is_empty());
        if let Some(ref email) = email_lc {
            if let Some(existing) = self.users.get_by_email(email).await? {
                let link =
                    OidcIdentity::new(existing.id.clone(), provider, subject, email_lc.clone());
                oidc.store(&link).await?;
                info!(
                    user_id = %existing.id,
                    provider = %provider,
                    "IdentityManager: linked OIDC identity to existing user by email",
                );
                return Ok(existing);
            }
        }

        // Fresh user. OIDC-originated accounts start as `UserRole::User` —
        // promote via `set_role` if needed.
        let provisional_email = email_lc
            .clone()
            .unwrap_or_else(|| format!("{subject}@{provider}.oidc.local"));
        let display_name = display_name_hint
            .map(str::to_string)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| {
                provisional_email
                    .split('@')
                    .next()
                    .unwrap_or(subject)
                    .to_string()
            });

        if self.users.get_by_email(&provisional_email).await?.is_some() {
            return Err(IdentityError::EmailExists(provisional_email));
        }

        let user = StoredUser::new(provisional_email.clone(), display_name, UserRole::User);
        self.users.store(&user).await?;

        let link = OidcIdentity::new(user.id.clone(), provider, subject, email_lc);
        if let Err(e) = oidc.store(&link).await {
            // Roll back the user row — the link is what makes the row
            // reachable; a row with no link and no credential is orphaned.
            if let Err(rollback) = self.users.delete(&user.id).await {
                error!(
                    user_id = %user.id,
                    provider = %provider,
                    link_err = %e,
                    rollback_err = %rollback,
                    "IdentityManager::find_or_create_by_external_id: link write failed AND rollback failed — user row orphaned",
                );
            } else {
                warn!(
                    user_id = %user.id,
                    provider = %provider,
                    error = %e,
                    "IdentityManager::find_or_create_by_external_id: link write failed; rolled back user row",
                );
            }
            return Err(IdentityError::UserStore(e));
        }

        info!(
            user_id = %user.id,
            email = %provisional_email,
            provider = %provider,
            "IdentityManager: new OIDC-provisioned user",
        );
        Ok(user)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::InMemoryUserStore;
    use zlayer_secrets::{EncryptionKey, PersistentSecretsStore};

    async fn mk() -> (
        Arc<IdentityManager>,
        Arc<dyn UserStorage>,
        Arc<CredentialStore<Arc<PersistentSecretsStore>>>,
        tempfile::TempDir,
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let users: Arc<dyn UserStorage> = Arc::new(InMemoryUserStore::new());
        let secrets = Arc::new(
            PersistentSecretsStore::open(
                tmp.path().join("secrets.sqlite"),
                EncryptionKey::generate(),
            )
            .await
            .unwrap(),
        );
        let creds = Arc::new(CredentialStore::new(secrets));
        let identity = Arc::new(IdentityManager::new(users.clone(), creds.clone()));
        (identity, users, creds, tmp)
    }

    #[tokio::test]
    async fn create_user_writes_both_stores() {
        let (identity, users, creds, _tmp) = mk().await;
        let user = identity
            .create_user(
                "alice@example.com",
                "Alice",
                UserRole::Admin,
                "correcthorsebatterystaple",
            )
            .await
            .unwrap();
        assert_eq!(user.email, "alice@example.com");
        assert_eq!(users.count().await.unwrap(), 1);
        let roles = creds
            .validate("alice@example.com", "correcthorsebatterystaple")
            .await
            .unwrap()
            .expect("validate");
        assert_eq!(roles, vec!["admin".to_string()]);
    }

    #[tokio::test]
    async fn create_user_rejects_duplicate_email() {
        let (identity, _users, _creds, _tmp) = mk().await;
        identity
            .create_user("a@b.com", "A", UserRole::User, "pw123456789012")
            .await
            .unwrap();
        let err = identity
            .create_user("a@b.com", "A", UserRole::User, "pw123456789012")
            .await
            .unwrap_err();
        assert!(matches!(err, IdentityError::EmailExists(_)), "got: {err}");
    }

    #[tokio::test]
    async fn create_user_rejects_empty_email_or_password() {
        let (identity, _users, _creds, _tmp) = mk().await;
        let err = identity
            .create_user("   ", "A", UserRole::User, "pw")
            .await
            .unwrap_err();
        assert!(matches!(err, IdentityError::Invalid(_)));

        let err = identity
            .create_user("a@b.com", "A", UserRole::User, "")
            .await
            .unwrap_err();
        assert!(matches!(err, IdentityError::Invalid(_)));
    }

    #[tokio::test]
    async fn delete_user_removes_both_stores() {
        let (identity, users, creds, _tmp) = mk().await;
        let user = identity
            .create_user("bob@example.com", "Bob", UserRole::User, "pw123456789012")
            .await
            .unwrap();

        let deleted = identity.delete_user(&user.id).await.unwrap();
        assert!(deleted.is_some());

        assert_eq!(users.count().await.unwrap(), 0);
        assert!(!creds.exists("bob@example.com").await.unwrap());
    }

    #[tokio::test]
    async fn delete_user_is_idempotent_on_missing_id() {
        let (identity, _users, _creds, _tmp) = mk().await;
        let result = identity.delete_user("nope-nope-nope").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn delete_user_tolerates_missing_credential_row() {
        let (identity, users, _creds, _tmp) = mk().await;
        // User row with no credential row (shouldn't happen in practice but
        // don't block cleanup if it does).
        let orphan = StoredUser::new(
            "orphan@example.com".to_string(),
            "Orphan".to_string(),
            UserRole::User,
        );
        users.store(&orphan).await.unwrap();
        let deleted = identity.delete_user(&orphan.id).await.unwrap();
        assert!(deleted.is_some());
        assert_eq!(users.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn set_role_updates_both_stores() {
        let (identity, users, creds, _tmp) = mk().await;
        let user = identity
            .create_user(
                "carol@example.com",
                "Carol",
                UserRole::User,
                "pw123456789012",
            )
            .await
            .unwrap();

        identity.set_role(&user.id, UserRole::Admin).await.unwrap();

        let fetched = users.get(&user.id).await.unwrap().unwrap();
        assert_eq!(fetched.role, UserRole::Admin);
        let roles = creds
            .validate("carol@example.com", "pw123456789012")
            .await
            .unwrap()
            .expect("validate");
        assert_eq!(roles, vec!["admin".to_string()]);
    }

    #[tokio::test]
    async fn set_role_errors_on_missing_user() {
        let (identity, _users, _creds, _tmp) = mk().await;
        let err = identity
            .set_role("nope-nope-nope", UserRole::Admin)
            .await
            .unwrap_err();
        assert!(matches!(err, IdentityError::NotFound(_)));
    }

    async fn mk_with_oidc() -> (
        Arc<IdentityManager>,
        Arc<dyn UserStorage>,
        Arc<dyn OidcIdentityStorage>,
        tempfile::TempDir,
    ) {
        let (_i, users, creds, tmp) = mk().await;
        let oidc: Arc<dyn OidcIdentityStorage> =
            Arc::new(crate::storage::InMemoryOidcIdentityStore::new());
        let mgr = Arc::new(IdentityManager::new(users.clone(), creds).with_oidc(oidc.clone()));
        (mgr, users, oidc, tmp)
    }

    #[tokio::test]
    async fn oidc_first_signin_creates_user_and_link() {
        let (identity, users, oidc, _tmp) = mk_with_oidc().await;
        let user = identity
            .find_or_create_by_external_id(
                "google",
                "sub-abc",
                Some("alice@example.com"),
                Some("Alice"),
            )
            .await
            .unwrap();
        assert_eq!(user.email, "alice@example.com");
        assert_eq!(users.count().await.unwrap(), 1);
        let link = oidc.get_by_external("google", "sub-abc").await.unwrap();
        assert_eq!(link.unwrap().user_id, user.id);
    }

    #[tokio::test]
    async fn oidc_repeat_signin_returns_existing_user() {
        let (identity, _users, _oidc, _tmp) = mk_with_oidc().await;
        let first = identity
            .find_or_create_by_external_id("google", "sub-abc", Some("a@b"), Some("A"))
            .await
            .unwrap();
        let second = identity
            .find_or_create_by_external_id("google", "sub-abc", Some("changed@b"), None)
            .await
            .unwrap();
        assert_eq!(first.id, second.id);
        // Local email is NOT overwritten by provider-side changes
        assert_eq!(second.email, "a@b");
    }

    #[tokio::test]
    async fn oidc_links_to_existing_local_user_by_email() {
        let (identity, users, oidc, _tmp) = mk_with_oidc().await;
        // Pre-existing local user (say, created via password signup)
        let local = identity
            .create_user(
                "bob@example.com",
                "Bob",
                UserRole::Admin,
                "correcthorsebatterystaple",
            )
            .await
            .unwrap();

        let via_oidc = identity
            .find_or_create_by_external_id(
                "google",
                "sub-999",
                Some("bob@example.com"),
                Some("Robert"),
            )
            .await
            .unwrap();
        assert_eq!(
            local.id, via_oidc.id,
            "must link to existing user, not create new"
        );
        assert_eq!(users.count().await.unwrap(), 1);
        let link = oidc.get_by_external("google", "sub-999").await.unwrap();
        assert_eq!(link.unwrap().user_id, local.id);
    }

    #[tokio::test]
    async fn oidc_disabled_errors_loudly() {
        let (identity, _users, _creds, _tmp) = mk().await;
        let err = identity
            .find_or_create_by_external_id("google", "sub", None, None)
            .await
            .unwrap_err();
        assert!(matches!(err, IdentityError::Invalid(_)));
    }
}
