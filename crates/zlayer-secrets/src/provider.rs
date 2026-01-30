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

use crate::{Result, Secret, SecretMetadata, SecretRef, SecretsError};

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
}

/// Resolver for secret references in configuration values.
///
/// The resolver parses `$S:` prefixed values and replaces them with actual
/// secret values from the underlying provider. This enables declarative
/// secret references in deployment configurations.
///
/// # Syntax
///
/// - `$S:name` - Deployment-level secret
/// - `$S:@service/name` - Service-level secret
/// - `$S:name/field` - Field extraction from structured (JSON) secrets
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
        }
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
        // Not a secret reference, return as-is
        if !SecretRef::is_secret_ref(value) {
            return Ok(value.to_string());
        }

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
}
