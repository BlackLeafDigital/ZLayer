//! Environment variable resolution for $E: and $S: prefix syntax
//!
//! Per V1_SPEC.md Section 7, values prefixed with `$E:` are resolved
//! from the runtime environment at container start time.
//!
//! Additionally, values prefixed with `$S:` are resolved from the secrets
//! provider at container start time.

use std::collections::HashMap;
use thiserror::Error;
use zlayer_secrets::{SecretRef, SecretsError, SecretsProvider};

/// Prefix that indicates a value should be resolved from host environment
const ENV_REF_PREFIX: &str = "$E:";

/// Prefix that indicates a value should be resolved from secrets provider
const SECRET_REF_PREFIX: &str = "$S:";

/// Errors that can occur during environment variable resolution
#[derive(Error, Debug, Clone)]
pub enum EnvResolutionError {
    #[error("environment variable '{var}' referenced by $E:{var} is not set")]
    MissingEnvVar { var: String },

    #[error("secret '{name}' referenced by $S:{name} was not found")]
    SecretNotFound { name: String },

    #[error("secret resolution error: {message}")]
    SecretResolution { message: String },
}

/// Result of resolving environment variables
pub struct ResolvedEnv {
    /// Successfully resolved environment variables (KEY=VALUE format for OCI)
    pub vars: Vec<String>,
    /// Any warnings (e.g., empty values)
    pub warnings: Vec<String>,
}

/// Resolve a single environment variable value
///
/// If the value starts with `$E:`, look up the remainder in `std::env::var()`.
/// Otherwise, return the value unchanged.
///
/// # Arguments
/// * `value` - The value to resolve (possibly with $E: prefix)
///
/// # Returns
/// * `Ok(String)` - The resolved value
/// * `Err(EnvResolutionError)` - If a $E: reference cannot be resolved
pub fn resolve_env_value(value: &str) -> Result<String, EnvResolutionError> {
    if let Some(var_name) = value.strip_prefix(ENV_REF_PREFIX) {
        match std::env::var(var_name) {
            Ok(val) => Ok(val),
            Err(std::env::VarError::NotPresent) => Err(EnvResolutionError::MissingEnvVar {
                var: var_name.to_string(),
            }),
            Err(std::env::VarError::NotUnicode(_)) => Err(EnvResolutionError::MissingEnvVar {
                var: var_name.to_string(),
            }),
        }
    } else {
        Ok(value.to_string())
    }
}

/// Resolve environment variables from a ServiceSpec env HashMap
///
/// For each entry:
/// - If value starts with `$E:`, look up the remainder in `std::env::var()`
/// - Otherwise, pass the value through unchanged
///
/// # Arguments
/// * `env` - HashMap of environment variable names to values (possibly with $E: prefix)
///
/// # Returns
/// * `Ok(HashMap<String, String>)` - Resolved key-value pairs
/// * `Err(EnvResolutionError)` - If a required $E: reference cannot be resolved
///
/// # Example
/// ```
/// use std::collections::HashMap;
/// use zlayer_agent::env::resolve_env_vars;
///
/// std::env::set_var("MY_SECRET", "secret_value");
///
/// let mut env = HashMap::new();
/// env.insert("NODE_ENV".to_string(), "production".to_string());
/// env.insert("DATABASE_URL".to_string(), "$E:MY_SECRET".to_string());
///
/// let resolved = resolve_env_vars(&env).unwrap();
/// assert_eq!(resolved.get("NODE_ENV").unwrap(), "production");
/// assert_eq!(resolved.get("DATABASE_URL").unwrap(), "secret_value");
///
/// std::env::remove_var("MY_SECRET");
/// ```
pub fn resolve_env_vars(
    env: &HashMap<String, String>,
) -> Result<HashMap<String, String>, EnvResolutionError> {
    let mut resolved = HashMap::with_capacity(env.len());

    for (key, value) in env {
        let resolved_value = resolve_env_value(value)?;
        resolved.insert(key.clone(), resolved_value);
    }

    Ok(resolved)
}

/// Resolve environment variables and return in OCI format with warnings
///
/// This is the full resolution function that also tracks warnings for empty values.
///
/// # Arguments
/// * `env` - HashMap of environment variable names to values (possibly with $E: prefix)
///
/// # Returns
/// * `Ok(ResolvedEnv)` - Resolved variables in KEY=VALUE format and any warnings
/// * `Err(EnvResolutionError)` - If a required $E: reference cannot be resolved
pub fn resolve_env_vars_with_warnings(
    env: &HashMap<String, String>,
) -> Result<ResolvedEnv, EnvResolutionError> {
    let mut vars = Vec::with_capacity(env.len());
    let mut warnings = Vec::new();

    for (key, value) in env {
        let resolved_value = if let Some(var_name) = value.strip_prefix(ENV_REF_PREFIX) {
            match std::env::var(var_name) {
                Ok(val) => {
                    if val.is_empty() {
                        warnings.push(format!(
                            "environment variable '{}' is set but empty",
                            var_name
                        ));
                    }
                    val
                }
                Err(std::env::VarError::NotPresent) => {
                    return Err(EnvResolutionError::MissingEnvVar {
                        var: var_name.to_string(),
                    });
                }
                Err(std::env::VarError::NotUnicode(_)) => {
                    return Err(EnvResolutionError::MissingEnvVar {
                        var: var_name.to_string(),
                    });
                }
            }
        } else {
            value.clone()
        };

        vars.push(format!("{}={}", key, resolved_value));
    }

    Ok(ResolvedEnv { vars, warnings })
}

/// Check if any environment variables use $E: references
///
/// Useful for validation and debugging to see which vars will be resolved at runtime.
pub fn has_env_references(env: &HashMap<String, String>) -> bool {
    env.values().any(|v| v.starts_with(ENV_REF_PREFIX))
}

/// Get list of $E: referenced variable names
///
/// Returns the names of host environment variables that will be looked up.
pub fn get_env_references(env: &HashMap<String, String>) -> Vec<&str> {
    env.values()
        .filter_map(|v| v.strip_prefix(ENV_REF_PREFIX))
        .collect()
}

/// Check if any environment variables use $S: secret references
///
/// Useful for validation and debugging to see which vars will be resolved from secrets.
pub fn has_secret_references(env: &HashMap<String, String>) -> bool {
    env.values().any(|v| v.starts_with(SECRET_REF_PREFIX))
}

/// Get list of $S: referenced secret names
///
/// Returns the raw secret reference strings (without the $S: prefix) that will be looked up.
pub fn get_secret_references(env: &HashMap<String, String>) -> Vec<&str> {
    env.values()
        .filter_map(|v| v.strip_prefix(SECRET_REF_PREFIX))
        .collect()
}

/// Extended environment resolution with secrets support
///
/// Resolves environment variables from multiple sources:
/// - `$S:` prefixed values are resolved from the secrets provider
/// - `$E:` prefixed values are resolved from host environment variables
/// - Other values are returned as-is
///
/// # Arguments
/// * `env` - HashMap of environment variable names to values (possibly with $S: or $E: prefix)
/// * `secrets_provider` - The secrets provider to use for $S: lookups
/// * `scope` - The scope identifier (e.g., deployment name) for secret lookups
///
/// # Returns
/// * `Ok(HashMap<String, String>)` - Resolved key-value pairs
/// * `Err(EnvResolutionError)` - If a required $S: or $E: reference cannot be resolved
///
/// # Example
/// ```rust,ignore
/// use std::collections::HashMap;
/// use zlayer_agent::env::resolve_env_with_secrets;
/// use zlayer_secrets::PersistentSecretsStore;
///
/// async fn example(provider: &PersistentSecretsStore) {
///     let mut env = HashMap::new();
///     env.insert("NODE_ENV".to_string(), "production".to_string());
///     env.insert("DATABASE_URL".to_string(), "$S:database-url".to_string());
///     env.insert("HOST_VAR".to_string(), "$E:MY_HOST_VAR".to_string());
///
///     let resolved = resolve_env_with_secrets(&env, provider, "my-deployment").await.unwrap();
/// }
/// ```
pub async fn resolve_env_with_secrets<P: SecretsProvider>(
    env: &HashMap<String, String>,
    secrets_provider: &P,
    scope: &str,
) -> Result<HashMap<String, String>, EnvResolutionError> {
    let mut resolved = HashMap::with_capacity(env.len());

    for (key, value) in env {
        let resolved_value = resolve_value_with_secrets(value, secrets_provider, scope).await?;
        resolved.insert(key.clone(), resolved_value);
    }

    Ok(resolved)
}

/// Resolve a single environment variable value with secrets support
///
/// If the value starts with `$S:`, look up the secret from the provider.
/// If the value starts with `$E:`, look up the remainder in `std::env::var()`.
/// Otherwise, return the value unchanged.
///
/// # Arguments
/// * `value` - The value to resolve (possibly with $S: or $E: prefix)
/// * `secrets_provider` - The secrets provider to use for $S: lookups
/// * `scope` - The scope identifier (e.g., deployment name) for secret lookups
///
/// # Returns
/// * `Ok(String)` - The resolved value
/// * `Err(EnvResolutionError)` - If a $S: or $E: reference cannot be resolved
pub async fn resolve_value_with_secrets<P: SecretsProvider>(
    value: &str,
    secrets_provider: &P,
    scope: &str,
) -> Result<String, EnvResolutionError> {
    // Check for secret reference first
    if SecretRef::is_secret_ref(value) {
        let secret_ref =
            SecretRef::parse(value).ok_or_else(|| EnvResolutionError::SecretResolution {
                message: format!("invalid secret reference syntax: {}", value),
            })?;

        // Determine the scope based on service qualifier
        let effective_scope = match &secret_ref.service {
            Some(service) => format!("{}/{}", scope, service),
            None => scope.to_string(),
        };

        // Fetch the secret
        let secret = secrets_provider
            .get_secret(&effective_scope, &secret_ref.name)
            .await
            .map_err(|e| match e {
                SecretsError::NotFound { name } => EnvResolutionError::SecretNotFound { name },
                other => EnvResolutionError::SecretResolution {
                    message: other.to_string(),
                },
            })?;

        let secret_value = secret.expose();

        // Handle field extraction if specified
        match &secret_ref.field {
            Some(field) => extract_json_field(secret_value, field),
            None => Ok(secret_value.to_string()),
        }
    } else if let Some(var_name) = value.strip_prefix(ENV_REF_PREFIX) {
        // Host environment variable reference
        match std::env::var(var_name) {
            Ok(val) => Ok(val),
            Err(std::env::VarError::NotPresent) => Err(EnvResolutionError::MissingEnvVar {
                var: var_name.to_string(),
            }),
            Err(std::env::VarError::NotUnicode(_)) => Err(EnvResolutionError::MissingEnvVar {
                var: var_name.to_string(),
            }),
        }
    } else {
        // Plain value, return as-is
        Ok(value.to_string())
    }
}

/// Extract a field from a JSON-formatted secret value.
fn extract_json_field(secret_value: &str, field: &str) -> Result<String, EnvResolutionError> {
    let json: serde_json::Value =
        serde_json::from_str(secret_value).map_err(|e| EnvResolutionError::SecretResolution {
            message: format!("failed to parse secret as JSON: {}", e),
        })?;

    match json.get(field) {
        Some(serde_json::Value::String(s)) => Ok(s.clone()),
        Some(serde_json::Value::Number(n)) => Ok(n.to_string()),
        Some(serde_json::Value::Bool(b)) => Ok(b.to_string()),
        Some(serde_json::Value::Null) => Ok(String::new()),
        Some(v) => Ok(v.to_string()), // For arrays/objects, return JSON string
        None => Err(EnvResolutionError::SecretNotFound {
            name: format!("field '{}' in secret", field),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_env_value_plain() {
        let result = resolve_env_value("plain_value").unwrap();
        assert_eq!(result, "plain_value");
    }

    #[test]
    fn test_resolve_env_value_reference() {
        std::env::set_var("TEST_RESOLVE_SINGLE", "resolved_value");

        let result = resolve_env_value("$E:TEST_RESOLVE_SINGLE").unwrap();
        assert_eq!(result, "resolved_value");

        std::env::remove_var("TEST_RESOLVE_SINGLE");
    }

    #[test]
    fn test_resolve_env_value_missing() {
        let result = resolve_env_value("$E:DEFINITELY_NOT_SET_SINGLE_12345");

        assert!(result.is_err());
        match result {
            Err(EnvResolutionError::MissingEnvVar { var }) => {
                assert_eq!(var, "DEFINITELY_NOT_SET_SINGLE_12345");
            }
            _ => panic!("Expected MissingEnvVar error"),
        }
    }

    #[test]
    fn test_resolve_plain_vars() {
        let mut env = HashMap::new();
        env.insert("NODE_ENV".to_string(), "production".to_string());
        env.insert("PORT".to_string(), "8080".to_string());

        let result = resolve_env_vars(&env).unwrap();

        assert_eq!(result.get("NODE_ENV").unwrap(), "production");
        assert_eq!(result.get("PORT").unwrap(), "8080");
    }

    #[test]
    fn test_resolve_env_reference() {
        std::env::set_var("TEST_RESOLVE_VAR", "test_value");

        let mut env = HashMap::new();
        env.insert("MY_VAR".to_string(), "$E:TEST_RESOLVE_VAR".to_string());

        let result = resolve_env_vars(&env).unwrap();

        assert_eq!(result.get("MY_VAR").unwrap(), "test_value");

        std::env::remove_var("TEST_RESOLVE_VAR");
    }

    #[test]
    fn test_missing_env_reference_fails() {
        let mut env = HashMap::new();
        env.insert(
            "MY_VAR".to_string(),
            "$E:DEFINITELY_NOT_SET_12345".to_string(),
        );

        let result = resolve_env_vars(&env);

        assert!(result.is_err());
        match result {
            Err(EnvResolutionError::MissingEnvVar { var }) => {
                assert_eq!(var, "DEFINITELY_NOT_SET_12345");
            }
            _ => panic!("Expected MissingEnvVar error"),
        }
    }

    #[test]
    fn test_mixed_vars() {
        std::env::set_var("TEST_DB_URL", "postgres://localhost/test");

        let mut env = HashMap::new();
        env.insert("NODE_ENV".to_string(), "production".to_string());
        env.insert("DATABASE_URL".to_string(), "$E:TEST_DB_URL".to_string());

        let result = resolve_env_vars(&env).unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result.get("NODE_ENV").unwrap(), "production");
        assert_eq!(
            result.get("DATABASE_URL").unwrap(),
            "postgres://localhost/test"
        );

        std::env::remove_var("TEST_DB_URL");
    }

    #[test]
    fn test_resolve_with_warnings_empty_value() {
        std::env::set_var("TEST_EMPTY_VAR", "");

        let mut env = HashMap::new();
        env.insert("EMPTY".to_string(), "$E:TEST_EMPTY_VAR".to_string());

        let result = resolve_env_vars_with_warnings(&env).unwrap();

        assert!(result.vars.iter().any(|v| v == "EMPTY="));
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].contains("TEST_EMPTY_VAR"));

        std::env::remove_var("TEST_EMPTY_VAR");
    }

    #[test]
    fn test_resolve_with_warnings_no_warnings() {
        std::env::set_var("TEST_NONEMPTY_VAR", "value");

        let mut env = HashMap::new();
        env.insert("VAR".to_string(), "$E:TEST_NONEMPTY_VAR".to_string());

        let result = resolve_env_vars_with_warnings(&env).unwrap();

        assert!(result.vars.iter().any(|v| v == "VAR=value"));
        assert!(result.warnings.is_empty());

        std::env::remove_var("TEST_NONEMPTY_VAR");
    }

    #[test]
    fn test_has_env_references() {
        let mut env = HashMap::new();
        env.insert("PLAIN".to_string(), "value".to_string());
        assert!(!has_env_references(&env));

        env.insert("REF".to_string(), "$E:SOME_VAR".to_string());
        assert!(has_env_references(&env));
    }

    #[test]
    fn test_get_env_references() {
        let mut env = HashMap::new();
        env.insert("PLAIN".to_string(), "value".to_string());
        env.insert("DB".to_string(), "$E:DATABASE_URL".to_string());
        env.insert("SECRET".to_string(), "$E:API_KEY".to_string());

        let refs = get_env_references(&env);

        assert_eq!(refs.len(), 2);
        assert!(refs.contains(&"DATABASE_URL"));
        assert!(refs.contains(&"API_KEY"));
    }

    #[test]
    fn test_empty_env_map() {
        let env = HashMap::new();

        let result = resolve_env_vars(&env).unwrap();
        assert!(result.is_empty());

        let result_with_warnings = resolve_env_vars_with_warnings(&env).unwrap();
        assert!(result_with_warnings.vars.is_empty());
        assert!(result_with_warnings.warnings.is_empty());
    }

    #[test]
    fn test_dollar_e_not_at_start() {
        // "$E:" must be at the start of the value
        let mut env = HashMap::new();
        env.insert("VAR".to_string(), "prefix$E:SOMETHING".to_string());

        let result = resolve_env_vars(&env).unwrap();
        // Should pass through unchanged
        assert_eq!(result.get("VAR").unwrap(), "prefix$E:SOMETHING");
    }

    #[test]
    fn test_partial_prefix() {
        // "$E" without colon should pass through
        let mut env = HashMap::new();
        env.insert("VAR".to_string(), "$E".to_string());

        let result = resolve_env_vars(&env).unwrap();
        assert_eq!(result.get("VAR").unwrap(), "$E");
    }

    #[test]
    fn test_has_secret_references() {
        let mut env = HashMap::new();
        env.insert("PLAIN".to_string(), "value".to_string());
        assert!(!has_secret_references(&env));

        env.insert("SECRET".to_string(), "$S:api-key".to_string());
        assert!(has_secret_references(&env));
    }

    #[test]
    fn test_get_secret_references() {
        let mut env = HashMap::new();
        env.insert("PLAIN".to_string(), "value".to_string());
        env.insert("SECRET1".to_string(), "$S:api-key".to_string());
        env.insert("SECRET2".to_string(), "$S:database-url".to_string());
        env.insert("ENV_VAR".to_string(), "$E:HOST_VAR".to_string());

        let refs = get_secret_references(&env);

        assert_eq!(refs.len(), 2);
        assert!(refs.contains(&"api-key"));
        assert!(refs.contains(&"database-url"));
    }

    // Tests for secrets resolution require async runtime and a mock provider
    mod secrets_tests {
        use super::*;
        use async_trait::async_trait;
        use std::sync::Mutex;
        use zlayer_secrets::{Secret, SecretMetadata, SecretsProvider};

        /// Mock provider for testing
        struct MockSecretsProvider {
            secrets: Mutex<HashMap<String, HashMap<String, Secret>>>,
        }

        impl MockSecretsProvider {
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
        impl SecretsProvider for MockSecretsProvider {
            async fn get_secret(&self, scope: &str, name: &str) -> zlayer_secrets::Result<Secret> {
                let secrets = self.secrets.lock().unwrap();
                secrets
                    .get(scope)
                    .and_then(|s| s.get(name))
                    .cloned()
                    .ok_or_else(|| zlayer_secrets::SecretsError::NotFound {
                        name: name.to_string(),
                    })
            }

            async fn get_secrets(
                &self,
                scope: &str,
                names: &[&str],
            ) -> zlayer_secrets::Result<HashMap<String, Secret>> {
                let secrets = self.secrets.lock().unwrap();
                let scope_secrets = secrets.get(scope);

                let mut result = HashMap::new();
                if let Some(scope_secrets) = scope_secrets {
                    for name in names {
                        if let Some(secret) = scope_secrets.get(*name) {
                            result.insert(name.to_string(), secret.clone());
                        }
                    }
                }
                Ok(result)
            }

            async fn list_secrets(
                &self,
                scope: &str,
            ) -> zlayer_secrets::Result<Vec<SecretMetadata>> {
                let secrets = self.secrets.lock().unwrap();
                Ok(secrets
                    .get(scope)
                    .map(|s| s.keys().map(SecretMetadata::new).collect())
                    .unwrap_or_default())
            }

            async fn exists(&self, scope: &str, name: &str) -> zlayer_secrets::Result<bool> {
                let secrets = self.secrets.lock().unwrap();
                Ok(secrets
                    .get(scope)
                    .map(|s| s.contains_key(name))
                    .unwrap_or(false))
            }
        }

        #[tokio::test]
        async fn test_resolve_value_with_secrets_plain() {
            let provider = MockSecretsProvider::new();
            let result = resolve_value_with_secrets("plain_value", &provider, "test-scope")
                .await
                .unwrap();
            assert_eq!(result, "plain_value");
        }

        #[tokio::test]
        async fn test_resolve_value_with_secrets_env_ref() {
            std::env::set_var("TEST_SECRETS_ENV_VAR", "env_value");

            let provider = MockSecretsProvider::new();
            let result =
                resolve_value_with_secrets("$E:TEST_SECRETS_ENV_VAR", &provider, "test-scope")
                    .await
                    .unwrap();
            assert_eq!(result, "env_value");

            std::env::remove_var("TEST_SECRETS_ENV_VAR");
        }

        #[tokio::test]
        async fn test_resolve_value_with_secrets_secret_ref() {
            let provider = MockSecretsProvider::new();
            provider.add_secret("test-deployment", "api-key", "secret-api-key-123");

            let result = resolve_value_with_secrets("$S:api-key", &provider, "test-deployment")
                .await
                .unwrap();
            assert_eq!(result, "secret-api-key-123");
        }

        #[tokio::test]
        async fn test_resolve_value_with_secrets_service_scoped() {
            let provider = MockSecretsProvider::new();
            provider.add_secret("test-deployment/api", "db-password", "service-specific-pwd");

            let result =
                resolve_value_with_secrets("$S:@api/db-password", &provider, "test-deployment")
                    .await
                    .unwrap();
            assert_eq!(result, "service-specific-pwd");
        }

        #[tokio::test]
        async fn test_resolve_value_with_secrets_field_extraction() {
            let provider = MockSecretsProvider::new();
            provider.add_secret(
                "test-deployment",
                "database",
                r#"{"host":"localhost","port":5432,"password":"db-secret"}"#,
            );

            let result =
                resolve_value_with_secrets("$S:database/password", &provider, "test-deployment")
                    .await
                    .unwrap();
            assert_eq!(result, "db-secret");

            // Test numeric field
            let result =
                resolve_value_with_secrets("$S:database/port", &provider, "test-deployment")
                    .await
                    .unwrap();
            assert_eq!(result, "5432");
        }

        #[tokio::test]
        async fn test_resolve_value_with_secrets_missing_secret() {
            let provider = MockSecretsProvider::new();

            let result =
                resolve_value_with_secrets("$S:nonexistent", &provider, "test-deployment").await;
            assert!(result.is_err());
            match result {
                Err(EnvResolutionError::SecretNotFound { name }) => {
                    assert_eq!(name, "nonexistent");
                }
                _ => panic!("Expected SecretNotFound error"),
            }
        }

        #[tokio::test]
        async fn test_resolve_env_with_secrets_mixed() {
            std::env::set_var("TEST_MIXED_HOST_VAR", "host_value");

            let provider = MockSecretsProvider::new();
            provider.add_secret("test-deployment", "api-key", "secret-key");
            provider.add_secret("test-deployment", "db-password", "secret-pwd");

            let mut env = HashMap::new();
            env.insert("API_KEY".to_string(), "$S:api-key".to_string());
            env.insert("DB_PASSWORD".to_string(), "$S:db-password".to_string());
            env.insert("HOST_VAR".to_string(), "$E:TEST_MIXED_HOST_VAR".to_string());
            env.insert("PLAIN_VAR".to_string(), "plain-value".to_string());

            let resolved = resolve_env_with_secrets(&env, &provider, "test-deployment")
                .await
                .unwrap();

            assert_eq!(resolved.get("API_KEY").unwrap(), "secret-key");
            assert_eq!(resolved.get("DB_PASSWORD").unwrap(), "secret-pwd");
            assert_eq!(resolved.get("HOST_VAR").unwrap(), "host_value");
            assert_eq!(resolved.get("PLAIN_VAR").unwrap(), "plain-value");

            std::env::remove_var("TEST_MIXED_HOST_VAR");
        }

        #[tokio::test]
        async fn test_resolve_env_with_secrets_missing_env_var() {
            let provider = MockSecretsProvider::new();

            let mut env = HashMap::new();
            env.insert(
                "MISSING".to_string(),
                "$E:DEFINITELY_NOT_SET_SECRETS_12345".to_string(),
            );

            let result = resolve_env_with_secrets(&env, &provider, "test-deployment").await;
            assert!(result.is_err());
            match result {
                Err(EnvResolutionError::MissingEnvVar { var }) => {
                    assert_eq!(var, "DEFINITELY_NOT_SET_SECRETS_12345");
                }
                _ => panic!("Expected MissingEnvVar error"),
            }
        }

        #[tokio::test]
        async fn test_resolve_env_with_secrets_missing_secret() {
            let provider = MockSecretsProvider::new();
            provider.add_secret("test-deployment", "exists", "value");

            let mut env = HashMap::new();
            env.insert("EXISTS".to_string(), "$S:exists".to_string());
            env.insert("MISSING".to_string(), "$S:does-not-exist".to_string());

            let result = resolve_env_with_secrets(&env, &provider, "test-deployment").await;
            assert!(result.is_err());
            match result {
                Err(EnvResolutionError::SecretNotFound { name }) => {
                    assert_eq!(name, "does-not-exist");
                }
                _ => panic!("Expected SecretNotFound error"),
            }
        }

        #[tokio::test]
        async fn test_extract_json_field_missing_field() {
            let provider = MockSecretsProvider::new();
            provider.add_secret(
                "test-deployment",
                "database",
                r#"{"host":"localhost","port":5432}"#,
            );

            let result =
                resolve_value_with_secrets("$S:database/nonexistent", &provider, "test-deployment")
                    .await;
            assert!(result.is_err());
            match result {
                Err(EnvResolutionError::SecretNotFound { name }) => {
                    assert!(name.contains("nonexistent"));
                }
                _ => panic!("Expected SecretNotFound error for missing field"),
            }
        }

        #[tokio::test]
        async fn test_extract_json_field_invalid_json() {
            let provider = MockSecretsProvider::new();
            provider.add_secret("test-deployment", "not-json", "this is not json");

            let result =
                resolve_value_with_secrets("$S:not-json/field", &provider, "test-deployment").await;
            assert!(result.is_err());
            match result {
                Err(EnvResolutionError::SecretResolution { message }) => {
                    assert!(message.contains("JSON"));
                }
                _ => panic!("Expected SecretResolution error for invalid JSON"),
            }
        }
    }
}
