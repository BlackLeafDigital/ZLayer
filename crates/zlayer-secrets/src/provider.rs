//! Secrets provider traits and resolver for `ZLayer` secrets management.
//!
//! This module defines the core abstractions for secrets storage and retrieval:
//!
//! - [`SecretsProvider`]: Read-only access to secrets
//! - [`SecretsStore`]: Read-write access to secrets (extends `SecretsProvider`)
//! - [`SecretsResolver`]: Resolves secret references (`$S:`) in configuration values

use async_trait::async_trait;
use std::collections::HashMap;
use tracing::instrument;

use crate::{Result, RotationResult, Secret, SecretMetadata, SecretRef, SecretsError};

/// Read-only secrets provider trait.
///
/// Implementations provide access to secrets from various backends such as
/// encrypted local storage, `HashiCorp` Vault, AWS Secrets Manager, etc.
///
/// # Scoping
///
/// Secrets are organized by scope, which is typically a deployment or service
/// identifier. The scope determines the namespace for secret lookups.
///
/// # Example
///
/// ```rust,ignore
/// use zlayer_secrets::{SecretsProvider, Secret};
///
/// async fn get_database_password(provider: &impl SecretsProvider) -> Result<Secret> {
///     provider.get_secret("my-deployment", "database-password").await
/// }
/// ```
#[async_trait]
pub trait SecretsProvider: Send + Sync {
    /// Retrieve a single secret by scope and name.
    ///
    /// # Arguments
    ///
    /// * `scope` - The scope identifier (e.g., deployment name)
    /// * `name` - The secret name within the scope
    ///
    /// # Errors
    ///
    /// Returns `SecretsError::NotFound` if the secret doesn't exist,
    /// or other errors for storage/access issues.
    async fn get_secret(&self, scope: &str, name: &str) -> Result<Secret>;

    /// Retrieve multiple secrets by scope and names.
    ///
    /// This method enables efficient batch retrieval when multiple secrets
    /// are needed. Implementations may optimize this by fetching all secrets
    /// in a single request where the backend supports it.
    ///
    /// # Arguments
    ///
    /// * `scope` - The scope identifier (e.g., deployment name)
    /// * `names` - Slice of secret names to retrieve
    ///
    /// # Returns
    ///
    /// A map of secret names to their values. Secrets that don't exist
    /// are omitted from the result rather than causing an error.
    async fn get_secrets(&self, scope: &str, names: &[&str]) -> Result<HashMap<String, Secret>>;

    /// List metadata for all secrets in a scope.
    ///
    /// This returns metadata (name, version, timestamps) without exposing
    /// the actual secret values. Useful for inventory and auditing.
    ///
    /// # Arguments
    ///
    /// * `scope` - The scope identifier to list secrets from
    async fn list_secrets(&self, scope: &str) -> Result<Vec<SecretMetadata>>;

    /// Check if a secret exists in the given scope.
    ///
    /// This is more efficient than `get_secret` when you only need to
    /// verify existence without retrieving the value.
    ///
    /// # Arguments
    ///
    /// * `scope` - The scope identifier
    /// * `name` - The secret name to check
    async fn exists(&self, scope: &str, name: &str) -> Result<bool>;
}

/// Read-write secrets store trait.
///
/// Extends [`SecretsProvider`] with write operations for managing secrets.
/// Implementations handle encryption, versioning, and storage.
///
/// # Example
///
/// ```rust,ignore
/// use zlayer_secrets::{SecretsStore, Secret};
///
/// async fn store_api_key(store: &impl SecretsStore, key: &str) -> Result<()> {
///     let secret = Secret::new(key);
///     store.set_secret("my-deployment", "api-key", &secret).await
/// }
/// ```
#[async_trait]
pub trait SecretsStore: SecretsProvider {
    /// Store or update a secret.
    ///
    /// If the secret already exists, it will be updated and its version
    /// incremented. If it doesn't exist, a new secret will be created.
    ///
    /// # Arguments
    ///
    /// * `scope` - The scope identifier (e.g., deployment name)
    /// * `name` - The secret name within the scope
    /// * `value` - The secret value to store
    ///
    /// # Errors
    ///
    /// Returns an error if encryption fails or storage is unavailable.
    async fn set_secret(&self, scope: &str, name: &str, value: &Secret) -> Result<()>;

    /// Delete a secret from the store.
    ///
    /// # Arguments
    ///
    /// * `scope` - The scope identifier
    /// * `name` - The secret name to delete
    ///
    /// # Errors
    ///
    /// Returns `SecretsError::NotFound` if the secret doesn't exist,
    /// or other errors for storage issues.
    async fn delete_secret(&self, scope: &str, name: &str) -> Result<()>;

    /// Rotate a secret: overwrite with a new value and return the version before+after.
    ///
    /// Default impl reads current metadata, writes the new value, re-reads metadata
    /// to capture the new version. Backends MAY override for efficiency.
    ///
    /// # Arguments
    ///
    /// * `scope` - The scope identifier
    /// * `name` - The secret name
    /// * `value` - The new secret value
    ///
    /// # Errors
    ///
    /// Returns [`SecretsError::NotFound`] if the secret does not exist (use `set_secret` to create).
    /// Other storage errors as usual.
    async fn rotate_secret(
        &self,
        scope: &str,
        name: &str,
        value: &Secret,
    ) -> Result<RotationResult> {
        let previous_version = self
            .list_secrets(scope)
            .await?
            .into_iter()
            .find(|m| m.name == name)
            .map(|m| m.version);
        if previous_version.is_none() {
            return Err(SecretsError::NotFound {
                name: name.to_string(),
            });
        }
        self.set_secret(scope, name, value).await?;
        let new_version = self
            .list_secrets(scope)
            .await?
            .into_iter()
            .find(|m| m.name == name)
            .map(|m| m.version)
            .ok_or_else(|| SecretsError::NotFound {
                name: name.to_string(),
            })?;
        Ok(RotationResult {
            previous_version,
            new_version,
        })
    }
}

// ---------------------------------------------------------------------------
// Blanket impls for Arc<T> - allows shared ownership of providers/stores
// ---------------------------------------------------------------------------

#[async_trait]
impl<T: SecretsProvider + ?Sized> SecretsProvider for std::sync::Arc<T> {
    async fn get_secret(&self, scope: &str, name: &str) -> Result<Secret> {
        (**self).get_secret(scope, name).await
    }

    async fn get_secrets(&self, scope: &str, names: &[&str]) -> Result<HashMap<String, Secret>> {
        (**self).get_secrets(scope, names).await
    }

    async fn list_secrets(&self, scope: &str) -> Result<Vec<SecretMetadata>> {
        (**self).list_secrets(scope).await
    }

    async fn exists(&self, scope: &str, name: &str) -> Result<bool> {
        (**self).exists(scope, name).await
    }
}

#[async_trait]
impl<T: SecretsStore + ?Sized> SecretsStore for std::sync::Arc<T> {
    async fn set_secret(&self, scope: &str, name: &str, value: &Secret) -> Result<()> {
        (**self).set_secret(scope, name, value).await
    }

    async fn delete_secret(&self, scope: &str, name: &str) -> Result<()> {
        (**self).delete_secret(scope, name).await
    }

    async fn rotate_secret(
        &self,
        scope: &str,
        name: &str,
        value: &Secret,
    ) -> Result<RotationResult> {
        (**self).rotate_secret(scope, name, value).await
    }
}

/// Resolves an environment name-or-id to the scope string used by the
/// underlying [`SecretsStore`].
///
/// Typically backed by the API's `EnvironmentStorage` plus the `env_scope(..)`
/// helper, which produces strings of the form `env:{env_id}` (global) or
/// `project:{pid}:env:{env_id}` (project-scoped).
///
/// This trait is consumed by [`SecretsResolver`] when resolving the
/// `$secret://<env>/<KEY>` URL-like reference form.
#[async_trait]
pub trait EnvScopeProvider: Send + Sync {
    /// Return the scope string for the given env (name or id).
    ///
    /// # Errors
    ///
    /// Returns an error if the env does not exist or cannot be resolved.
    async fn resolve_env_scope(&self, name_or_id: &str) -> Result<String>;
}

/// Resolver for secret references in configuration values.
///
/// The resolver parses `$S:` and `$secret://` prefixed values and replaces
/// them with actual secret values from the underlying provider. This enables
/// declarative secret references in deployment configurations.
///
/// # Syntax
///
/// - `$S:name` - Deployment-level secret
/// - `$S:@service/name` - Service-level secret
/// - `$S:name/field` - Field extraction from structured (JSON) secrets
/// - `$secret://<env>/<KEY>` - Environment-scoped secret lookup
/// - `$secret://<env>/<KEY>/<field>` - With JSON field extraction
///
/// The `$secret://` form requires an [`EnvScopeProvider`] to be wired up via
/// [`SecretsResolver::with_env_resolver`]; otherwise such references fail with
/// a clear error.
///
/// # Example
///
/// ```rust,ignore
/// use zlayer_secrets::{SecretsResolver, PersistentSecretsStore};
/// use std::collections::HashMap;
///
/// async fn resolve_env_vars(
///     store: PersistentSecretsStore,
///     env: HashMap<String, String>,
/// ) -> Result<HashMap<String, String>> {
///     let resolver = SecretsResolver::new(store, "my-deployment");
///     resolver.resolve_env(&env).await
/// }
/// ```
pub struct SecretsResolver<P: SecretsProvider> {
    provider: P,
    scope: String,
    env_resolver: Option<std::sync::Arc<dyn EnvScopeProvider>>,
}

impl<P: SecretsProvider> SecretsResolver<P> {
    /// Create a new secrets resolver.
    ///
    /// # Arguments
    ///
    /// * `provider` - The secrets provider to use for lookups
    /// * `scope` - The default scope (deployment name) for resolving secrets
    pub fn new(provider: P, scope: impl Into<String>) -> Self {
        Self {
            provider,
            scope: scope.into(),
            env_resolver: None,
        }
    }

    /// Attach an [`EnvScopeProvider`] to enable resolution of
    /// `$secret://<env>/<KEY>` references.
    ///
    /// Without this, any `$secret://` reference will fail with a clear error.
    #[must_use]
    pub fn with_env_resolver(mut self, env_resolver: std::sync::Arc<dyn EnvScopeProvider>) -> Self {
        self.env_resolver = Some(env_resolver);
        self
    }

    /// Get a reference to the underlying provider.
    pub fn provider(&self) -> &P {
        &self.provider
    }

    /// Get the configured scope.
    pub fn scope(&self) -> &str {
        &self.scope
    }

    /// Resolve a single value that may contain a secret reference.
    ///
    /// If the value starts with `$S:`, it will be parsed and replaced with
    /// the actual secret value. Otherwise, the original value is returned.
    ///
    /// # Arguments
    ///
    /// * `value` - The value to potentially resolve
    ///
    /// # Errors
    ///
    /// Returns an error if the value is a secret reference but:
    /// - The reference syntax is invalid
    /// - The secret doesn't exist
    /// - Field extraction fails (for JSON secrets)
    #[instrument(skip(self), fields(scope = %self.scope))]
    pub async fn resolve_value(&self, value: &str) -> Result<String> {
        // New URL-like form: $secret://<env>/<KEY>[/<field>]
        if let Some(rest) = value.strip_prefix("$secret://") {
            return self.resolve_secret_url(rest).await;
        }

        // Existing $S:... form
        if SecretRef::is_secret_ref(value) {
            return self.resolve_s_ref(value).await;
        }

        // Not a secret reference, return as-is
        Ok(value.to_string())
    }

    /// Resolve the existing `$S:` style reference (deployment-scoped or
    /// `@service/`-scoped, with optional `/field` JSON extraction).
    async fn resolve_s_ref(&self, value: &str) -> Result<String> {
        // Parse the reference
        let secret_ref = SecretRef::parse(value).ok_or_else(|| SecretsError::InvalidName {
            name: value.to_string(),
        })?;

        // Determine the scope based on service qualifier
        let scope = match &secret_ref.service {
            Some(service) => format!("{}/{}", self.scope, service),
            None => self.scope.clone(),
        };

        // Fetch the secret
        let secret = self.provider.get_secret(&scope, &secret_ref.name).await?;
        let secret_value = secret.expose();

        // Handle field extraction if specified
        match &secret_ref.field {
            Some(field) => Self::extract_field(secret_value, field),
            None => Ok(secret_value.to_string()),
        }
    }

    /// Resolve a `$secret://<env>/<KEY>[/<field>]` reference.
    ///
    /// `rest` is the portion of the value after the `$secret://` prefix.
    async fn resolve_secret_url(&self, rest: &str) -> Result<String> {
        // Split off the env component.
        let (env_name, after_env) =
            rest.split_once('/')
                .ok_or_else(|| SecretsError::InvalidName {
                    name: format!("$secret://{rest}"),
                })?;

        if env_name.is_empty() {
            return Err(SecretsError::InvalidName {
                name: format!("$secret://{rest}"),
            });
        }

        // Remainder is either "KEY" or "KEY/field".
        let (key, field) = match after_env.split_once('/') {
            Some((k, f)) => (k, Some(f.to_string())),
            None => (after_env, None),
        };

        if key.is_empty() {
            return Err(SecretsError::InvalidName {
                name: format!("$secret://{rest}"),
            });
        }

        let env_resolver = self.env_resolver.as_ref().ok_or_else(|| {
            SecretsError::Provider(
                "SecretsResolver has no env resolver; `$secret://` not supported".to_string(),
            )
        })?;

        let scope = env_resolver.resolve_env_scope(env_name).await?;
        let secret = self.provider.get_secret(&scope, key).await?;
        let secret_value = secret.expose();

        match field {
            Some(f) => Self::extract_field(secret_value, &f),
            None => Ok(secret_value.to_string()),
        }
    }

    /// Resolve all secret references in a map of environment variables.
    ///
    /// This method efficiently batches secret lookups by:
    /// 1. Scanning all values to identify secret references
    /// 2. Grouping references by scope
    /// 3. Fetching secrets in batches per scope
    /// 4. Resolving all values with the fetched secrets
    ///
    /// # Arguments
    ///
    /// * `env` - Map of environment variable names to values
    ///
    /// # Returns
    ///
    /// A new map with all secret references replaced by their actual values.
    /// Non-secret values are passed through unchanged.
    ///
    /// # Errors
    ///
    /// Returns an error if any secret reference is invalid, or if a referenced
    /// secret cannot be found.
    #[instrument(skip(self, env), fields(scope = %self.scope, env_count = env.len()))]
    pub async fn resolve_env(
        &self,
        env: &HashMap<String, String>,
    ) -> Result<HashMap<String, String>> {
        // First pass: identify all secret references and group by scope
        let mut refs_by_scope: HashMap<String, Vec<(String, SecretRef)>> = HashMap::new();
        let mut non_secret_entries: Vec<(String, String)> = Vec::new();

        for (key, value) in env {
            if SecretRef::is_secret_ref(value) {
                if let Some(secret_ref) = SecretRef::parse(value) {
                    let scope = match &secret_ref.service {
                        Some(service) => format!("{}/{}", self.scope, service),
                        None => self.scope.clone(),
                    };
                    refs_by_scope
                        .entry(scope)
                        .or_default()
                        .push((key.clone(), secret_ref));
                } else {
                    return Err(SecretsError::InvalidName {
                        name: value.clone(),
                    });
                }
            } else {
                non_secret_entries.push((key.clone(), value.clone()));
            }
        }

        // Batch fetch secrets by scope
        let mut secrets_by_scope: HashMap<String, HashMap<String, Secret>> = HashMap::new();

        for (scope, refs) in &refs_by_scope {
            let names: Vec<&str> = refs
                .iter()
                .map(|(_, secret_ref)| secret_ref.name.as_str())
                .collect();

            // Deduplicate names for the batch request
            let unique_names: Vec<&str> = names
                .iter()
                .copied()
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();

            let secrets = self.provider.get_secrets(scope, &unique_names).await?;
            secrets_by_scope.insert(scope.clone(), secrets);
        }

        // Build the resolved environment map
        let mut resolved = HashMap::with_capacity(env.len());

        // Add non-secret entries directly
        for (key, value) in non_secret_entries {
            resolved.insert(key, value);
        }

        // Resolve secret references
        for (scope, refs) in refs_by_scope {
            let scope_secrets = secrets_by_scope.get(&scope).ok_or_else(|| {
                SecretsError::Provider(format!("missing secrets for scope: {scope}"))
            })?;

            for (env_key, secret_ref) in refs {
                let secret =
                    scope_secrets
                        .get(&secret_ref.name)
                        .ok_or_else(|| SecretsError::NotFound {
                            name: secret_ref.name.clone(),
                        })?;

                let value = match &secret_ref.field {
                    Some(field) => Self::extract_field(secret.expose(), field)?,
                    None => secret.expose().to_string(),
                };

                resolved.insert(env_key, value);
            }
        }

        Ok(resolved)
    }

    /// Extract a field from a JSON-formatted secret value.
    fn extract_field(secret_value: &str, field: &str) -> Result<String> {
        let json: serde_json::Value = serde_json::from_str(secret_value)
            .map_err(|e| SecretsError::Decryption(e.to_string()))?;

        match json.get(field) {
            Some(serde_json::Value::String(s)) => Ok(s.clone()),
            Some(serde_json::Value::Number(n)) => Ok(n.to_string()),
            Some(serde_json::Value::Bool(b)) => Ok(b.to_string()),
            Some(serde_json::Value::Null) => Ok(String::new()),
            Some(v) => Ok(v.to_string()), // For arrays/objects, return JSON string
            None => Err(SecretsError::NotFound {
                name: format!("field '{field}' in secret"),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// Mock provider for testing
    struct MockProvider {
        secrets: Mutex<HashMap<String, HashMap<String, Secret>>>,
    }

    impl MockProvider {
        fn new() -> Self {
            Self {
                secrets: Mutex::new(HashMap::new()),
            }
        }

        fn add_secret(&self, scope: &str, name: &str, value: &str) {
            let mut secrets = self.secrets.lock().unwrap();
            secrets
                .entry(scope.to_string())
                .or_default()
                .insert(name.to_string(), Secret::new(value));
        }
    }

    #[async_trait]
    impl SecretsProvider for MockProvider {
        async fn get_secret(&self, scope: &str, name: &str) -> Result<Secret> {
            let secrets = self.secrets.lock().unwrap();
            secrets
                .get(scope)
                .and_then(|s| s.get(name))
                .cloned()
                .ok_or_else(|| SecretsError::NotFound {
                    name: name.to_string(),
                })
        }

        async fn get_secrets(
            &self,
            scope: &str,
            names: &[&str],
        ) -> Result<HashMap<String, Secret>> {
            let secrets = self.secrets.lock().unwrap();
            let scope_secrets = secrets.get(scope);

            let mut result = HashMap::new();
            if let Some(scope_secrets) = scope_secrets {
                for name in names {
                    if let Some(secret) = scope_secrets.get(*name) {
                        result.insert((*name).to_string(), secret.clone());
                    }
                }
            }
            Ok(result)
        }

        async fn list_secrets(&self, scope: &str) -> Result<Vec<SecretMetadata>> {
            let secrets = self.secrets.lock().unwrap();
            Ok(secrets
                .get(scope)
                .map(|s| s.keys().map(SecretMetadata::new).collect())
                .unwrap_or_default())
        }

        async fn exists(&self, scope: &str, name: &str) -> Result<bool> {
            let secrets = self.secrets.lock().unwrap();
            Ok(secrets.get(scope).is_some_and(|s| s.contains_key(name)))
        }
    }

    #[tokio::test]
    async fn test_resolve_non_secret_value() {
        let provider = MockProvider::new();
        let resolver = SecretsResolver::new(provider, "test-deployment");

        let result = resolver.resolve_value("plain-value").await.unwrap();
        assert_eq!(result, "plain-value");
    }

    #[tokio::test]
    async fn test_resolve_secret_value() {
        let provider = MockProvider::new();
        provider.add_secret("test-deployment", "api-key", "secret-api-key-123");

        let resolver = SecretsResolver::new(provider, "test-deployment");

        let result = resolver.resolve_value("$S:api-key").await.unwrap();
        assert_eq!(result, "secret-api-key-123");
    }

    #[tokio::test]
    async fn test_resolve_service_scoped_secret() {
        let provider = MockProvider::new();
        provider.add_secret("test-deployment/api", "db-password", "service-specific-pwd");

        let resolver = SecretsResolver::new(provider, "test-deployment");

        let result = resolver.resolve_value("$S:@api/db-password").await.unwrap();
        assert_eq!(result, "service-specific-pwd");
    }

    #[tokio::test]
    async fn test_resolve_secret_with_field() {
        let provider = MockProvider::new();
        provider.add_secret(
            "test-deployment",
            "database",
            r#"{"host":"localhost","port":5432,"password":"db-secret"}"#,
        );

        let resolver = SecretsResolver::new(provider, "test-deployment");

        let result = resolver
            .resolve_value("$S:database/password")
            .await
            .unwrap();
        assert_eq!(result, "db-secret");

        // Test numeric field
        let provider = MockProvider::new();
        provider.add_secret(
            "test-deployment",
            "database",
            r#"{"host":"localhost","port":5432,"password":"db-secret"}"#,
        );
        let resolver = SecretsResolver::new(provider, "test-deployment");

        let result = resolver.resolve_value("$S:database/port").await.unwrap();
        assert_eq!(result, "5432");
    }

    #[tokio::test]
    async fn test_resolve_missing_secret() {
        let provider = MockProvider::new();
        let resolver = SecretsResolver::new(provider, "test-deployment");

        let result = resolver.resolve_value("$S:nonexistent").await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), SecretsError::NotFound { .. }));
    }

    #[tokio::test]
    async fn test_resolve_env() {
        let provider = MockProvider::new();
        provider.add_secret("test-deployment", "api-key", "secret-key");
        provider.add_secret("test-deployment", "db-password", "secret-pwd");
        provider.add_secret("test-deployment/worker", "worker-token", "worker-secret");

        let resolver = SecretsResolver::new(provider, "test-deployment");

        let mut env = HashMap::new();
        env.insert("API_KEY".to_string(), "$S:api-key".to_string());
        env.insert("DB_PASSWORD".to_string(), "$S:db-password".to_string());
        env.insert(
            "WORKER_TOKEN".to_string(),
            "$S:@worker/worker-token".to_string(),
        );
        env.insert("PLAIN_VAR".to_string(), "plain-value".to_string());

        let resolved_env = resolver.resolve_env(&env).await.unwrap();

        assert_eq!(resolved_env.get("API_KEY").unwrap(), "secret-key");
        assert_eq!(resolved_env.get("DB_PASSWORD").unwrap(), "secret-pwd");
        assert_eq!(resolved_env.get("WORKER_TOKEN").unwrap(), "worker-secret");
        assert_eq!(resolved_env.get("PLAIN_VAR").unwrap(), "plain-value");
    }

    #[tokio::test]
    async fn test_resolve_env_with_missing_secret() {
        let provider = MockProvider::new();
        provider.add_secret("test-deployment", "exists", "value");

        let resolver = SecretsResolver::new(provider, "test-deployment");

        let mut env = HashMap::new();
        env.insert("EXISTS".to_string(), "$S:exists".to_string());
        env.insert("MISSING".to_string(), "$S:does-not-exist".to_string());

        let result = resolver.resolve_env(&env).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_provider_exists() {
        let provider = MockProvider::new();
        provider.add_secret("scope", "exists", "value");

        assert!(provider.exists("scope", "exists").await.unwrap());
        assert!(!provider.exists("scope", "missing").await.unwrap());
        assert!(!provider.exists("other-scope", "exists").await.unwrap());
    }

    #[tokio::test]
    async fn test_provider_list_secrets() {
        let provider = MockProvider::new();
        provider.add_secret("scope", "secret1", "value1");
        provider.add_secret("scope", "secret2", "value2");
        provider.add_secret("other", "secret3", "value3");

        let list = provider.list_secrets("scope").await.unwrap();
        assert_eq!(list.len(), 2);

        let names: Vec<&str> = list.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"secret1"));
        assert!(names.contains(&"secret2"));
    }

    #[tokio::test]
    async fn test_resolver_accessors() {
        let provider = MockProvider::new();
        let resolver = SecretsResolver::new(provider, "my-scope");

        assert_eq!(resolver.scope(), "my-scope");
        // Verify provider is accessible
        let _ = resolver.provider();
    }

    /// Mock store that tracks versioned metadata, for exercising `SecretsStore`
    /// default methods like `rotate_secret`.
    type MockStoreData = Mutex<HashMap<String, HashMap<String, (Secret, u32)>>>;

    struct MockStore {
        // scope -> name -> (Secret, version)
        data: MockStoreData,
    }

    impl MockStore {
        fn new() -> Self {
            Self {
                data: Mutex::new(HashMap::new()),
            }
        }
    }

    #[async_trait]
    impl SecretsProvider for MockStore {
        async fn get_secret(&self, scope: &str, name: &str) -> Result<Secret> {
            let data = self.data.lock().unwrap();
            data.get(scope)
                .and_then(|s| s.get(name))
                .map(|(secret, _)| secret.clone())
                .ok_or_else(|| SecretsError::NotFound {
                    name: name.to_string(),
                })
        }

        async fn get_secrets(
            &self,
            scope: &str,
            names: &[&str],
        ) -> Result<HashMap<String, Secret>> {
            let data = self.data.lock().unwrap();
            let mut result = HashMap::new();
            if let Some(scope_data) = data.get(scope) {
                for name in names {
                    if let Some((secret, _)) = scope_data.get(*name) {
                        result.insert((*name).to_string(), secret.clone());
                    }
                }
            }
            Ok(result)
        }

        async fn list_secrets(&self, scope: &str) -> Result<Vec<SecretMetadata>> {
            let data = self.data.lock().unwrap();
            Ok(data
                .get(scope)
                .map(|s| {
                    s.iter()
                        .map(|(name, (_, version))| {
                            let mut meta = SecretMetadata::new(name);
                            meta.version = *version;
                            meta
                        })
                        .collect()
                })
                .unwrap_or_default())
        }

        async fn exists(&self, scope: &str, name: &str) -> Result<bool> {
            let data = self.data.lock().unwrap();
            Ok(data.get(scope).is_some_and(|s| s.contains_key(name)))
        }
    }

    #[async_trait]
    impl SecretsStore for MockStore {
        async fn set_secret(&self, scope: &str, name: &str, value: &Secret) -> Result<()> {
            let mut data = self.data.lock().unwrap();
            let scope_data = data.entry(scope.to_string()).or_default();
            let next_version = scope_data
                .get(name)
                .map_or(1, |(_, version)| version.saturating_add(1));
            scope_data.insert(name.to_string(), (value.clone(), next_version));
            Ok(())
        }

        async fn delete_secret(&self, scope: &str, name: &str) -> Result<()> {
            let mut data = self.data.lock().unwrap();
            let scope_data = data.get_mut(scope).ok_or_else(|| SecretsError::NotFound {
                name: name.to_string(),
            })?;
            scope_data
                .remove(name)
                .ok_or_else(|| SecretsError::NotFound {
                    name: name.to_string(),
                })?;
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_rotate_secret_default_impl() {
        let store = MockStore::new();
        let scope = "test-scope";
        let name = "test-key";

        // Initial write: version 1
        store
            .set_secret(scope, name, &Secret::new("v1"))
            .await
            .unwrap();

        // Rotate to v2
        let result = store
            .rotate_secret(scope, name, &Secret::new("v2"))
            .await
            .unwrap();

        assert_eq!(result.previous_version, Some(1));
        assert_eq!(result.new_version, 2);

        // Verify the stored value is now v2
        let current = store.get_secret(scope, name).await.unwrap();
        assert_eq!(current.expose(), "v2");
    }

    #[tokio::test]
    async fn test_rotate_secret_missing_returns_not_found() {
        let store = MockStore::new();
        let result = store
            .rotate_secret("scope", "does-not-exist", &Secret::new("v1"))
            .await;
        assert!(matches!(result, Err(SecretsError::NotFound { .. })));
    }

    // -----------------------------------------------------------------------
    // `$secret://` URL-form reference tests
    // -----------------------------------------------------------------------

    /// Simple in-memory env-scope resolver used by the `$secret://` tests.
    ///
    /// Maps an env name-or-id to the scope string used by the underlying
    /// provider (e.g. `"bootstrap"` -> `"env:abc"`).
    struct MockEnvScope {
        map: HashMap<String, String>,
    }

    impl MockEnvScope {
        fn new(pairs: &[(&str, &str)]) -> Self {
            Self {
                map: pairs
                    .iter()
                    .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                    .collect(),
            }
        }
    }

    #[async_trait]
    impl EnvScopeProvider for MockEnvScope {
        async fn resolve_env_scope(&self, name_or_id: &str) -> Result<String> {
            self.map
                .get(name_or_id)
                .cloned()
                .ok_or_else(|| SecretsError::NotFound {
                    name: format!("env:{name_or_id}"),
                })
        }
    }

    #[tokio::test]
    async fn test_secret_url_resolves_via_env_resolver() {
        let provider = MockProvider::new();
        provider.add_secret("env:abc", "PWD", "xyz");

        let env_resolver = std::sync::Arc::new(MockEnvScope::new(&[("bootstrap", "env:abc")]));

        let resolver =
            SecretsResolver::new(provider, "ignored-scope").with_env_resolver(env_resolver);

        let result = resolver
            .resolve_value("$secret://bootstrap/PWD")
            .await
            .unwrap();
        assert_eq!(result, "xyz");
    }

    #[tokio::test]
    async fn test_secret_url_without_env_resolver_errors() {
        let provider = MockProvider::new();
        provider.add_secret("env:abc", "PWD", "xyz");

        // No with_env_resolver() call.
        let resolver = SecretsResolver::new(provider, "ignored-scope");

        let err = resolver
            .resolve_value("$secret://bootstrap/PWD")
            .await
            .unwrap_err();

        match err {
            SecretsError::Provider(msg) => {
                assert!(
                    msg.contains("$secret://"),
                    "expected error to mention `$secret://`, got: {msg}"
                );
            }
            other => panic!("expected SecretsError::Provider, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_secret_url_with_json_field_extraction() {
        let provider = MockProvider::new();
        provider.add_secret(
            "env:abc",
            "database",
            r#"{"host":"localhost","port":5432,"password":"db-secret"}"#,
        );

        let env_resolver = std::sync::Arc::new(MockEnvScope::new(&[("bootstrap", "env:abc")]));

        let resolver =
            SecretsResolver::new(provider, "ignored-scope").with_env_resolver(env_resolver);

        // String field
        let pwd = resolver
            .resolve_value("$secret://bootstrap/database/password")
            .await
            .unwrap();
        assert_eq!(pwd, "db-secret");

        // Numeric field coerced to its string form
        let port = resolver
            .resolve_value("$secret://bootstrap/database/port")
            .await
            .unwrap();
        assert_eq!(port, "5432");
    }

    #[tokio::test]
    async fn test_secret_url_malformed_missing_key_errors() {
        let provider = MockProvider::new();
        let env_resolver = std::sync::Arc::new(MockEnvScope::new(&[("bootstrap", "env:abc")]));
        let resolver =
            SecretsResolver::new(provider, "ignored-scope").with_env_resolver(env_resolver);

        // No `/KEY` component at all.
        let err = resolver
            .resolve_value("$secret://bootstrap")
            .await
            .unwrap_err();
        assert!(matches!(err, SecretsError::InvalidName { .. }));

        // Trailing slash, empty key.
        let err = resolver
            .resolve_value("$secret://bootstrap/")
            .await
            .unwrap_err();
        assert!(matches!(err, SecretsError::InvalidName { .. }));
    }

    #[tokio::test]
    async fn test_secret_url_unknown_env_propagates_not_found() {
        let provider = MockProvider::new();
        let env_resolver = std::sync::Arc::new(MockEnvScope::new(&[("bootstrap", "env:abc")]));
        let resolver =
            SecretsResolver::new(provider, "ignored-scope").with_env_resolver(env_resolver);

        let err = resolver
            .resolve_value("$secret://unknown-env/PWD")
            .await
            .unwrap_err();
        assert!(matches!(err, SecretsError::NotFound { .. }));
    }
}
