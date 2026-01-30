//! `HashiCorp` Vault secrets provider for `ZLayer`.
//!
//! Provides integration with `HashiCorp` Vault's KV2 secrets engine
//! for centralized secrets management.
//!
//! # Environment Variables
//!
//! - `VAULT_ADDR` - Vault server address (required for `from_env`)
//! - `VAULT_TOKEN` - Vault authentication token (required for `from_env`)
//! - `VAULT_MOUNT` - KV2 secrets engine mount path (default: "secret")
//!
//! # Example
//!
//! ```no_run
//! use zlayer_secrets::VaultSecretsProvider;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // From environment variables
//!     let provider = VaultSecretsProvider::from_env()?;
//!
//!     // Or with explicit configuration
//!     let provider = VaultSecretsProvider::new(
//!         "https://vault.example.com:8200",
//!         "hvs.your-token-here",
//!         "secret",
//!     )?;
//!
//!     Ok(())
//! }
//! ```

#[cfg(feature = "vault")]
use std::collections::HashMap;

#[cfg(feature = "vault")]
use async_trait::async_trait;

#[cfg(feature = "vault")]
use serde::{Deserialize, Serialize};

#[cfg(feature = "vault")]
use vaultrs::client::{VaultClient, VaultClientSettingsBuilder};

#[cfg(feature = "vault")]
use vaultrs::kv2;

#[cfg(feature = "vault")]
use crate::{Result, Secret, SecretMetadata, SecretsError, SecretsProvider, SecretsStore};

/// Environment variable for Vault server address.
#[cfg(feature = "vault")]
const ENV_VAULT_ADDR: &str = "VAULT_ADDR";

/// Environment variable for Vault authentication token.
#[cfg(feature = "vault")]
const ENV_VAULT_TOKEN: &str = "VAULT_TOKEN";

/// Environment variable for KV2 mount path.
#[cfg(feature = "vault")]
const ENV_VAULT_MOUNT: &str = "VAULT_MOUNT";

/// Default KV2 secrets engine mount path.
#[cfg(feature = "vault")]
const DEFAULT_MOUNT: &str = "secret";

/// Internal structure for storing secrets in Vault.
///
/// Secrets are stored as JSON objects with a "value" field.
#[cfg(feature = "vault")]
#[derive(Debug, Serialize, Deserialize)]
struct VaultSecretData {
    value: String,
}

/// `HashiCorp` Vault secrets provider using the KV2 secrets engine.
///
/// This provider stores secrets in Vault's KV version 2 secrets engine,
/// which supports versioning and metadata.
///
/// # Path Format
///
/// Secrets are stored at `{mount}/{scope}/{name}` where:
/// - `mount` is the KV2 secrets engine mount path (default: "secret")
/// - `scope` is the logical grouping (e.g., "deployments/myapp/secrets")
/// - `name` is the secret identifier
///
/// # Example
///
/// ```no_run
/// use zlayer_secrets::{VaultSecretsProvider, SecretsProvider};
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let provider = VaultSecretsProvider::from_env()?;
///
///     // Get a secret
///     let secret = provider.get_secret("deployments/myapp/secrets", "database-password").await?;
///     println!("Got secret!");
///
///     Ok(())
/// }
/// ```
#[cfg(feature = "vault")]
pub struct VaultSecretsProvider {
    client: VaultClient,
    mount: String,
}

#[cfg(feature = "vault")]
impl std::fmt::Debug for VaultSecretsProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultSecretsProvider")
            .field("mount", &self.mount)
            .field("client", &"VaultClient { ... }")
            .finish()
    }
}

#[cfg(feature = "vault")]
impl VaultSecretsProvider {
    /// Creates a new Vault secrets provider with explicit configuration.
    ///
    /// # Arguments
    ///
    /// * `address` - The Vault server address (e.g., `https://vault.example.com:8200`)
    /// * `token` - The Vault authentication token
    /// * `mount` - The KV2 secrets engine mount path
    ///
    /// # Errors
    ///
    /// Returns `SecretsError::Provider` if the Vault client cannot be created.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use zlayer_secrets::VaultSecretsProvider;
    ///
    /// let provider = VaultSecretsProvider::new(
    ///     "https://vault.example.com:8200",
    ///     "hvs.your-token-here",
    ///     "secret",
    /// )?;
    /// # Ok::<(), zlayer_secrets::SecretsError>(())
    /// ```
    pub fn new(address: &str, token: &str, mount: &str) -> Result<Self> {
        let settings = VaultClientSettingsBuilder::default()
            .address(address)
            .token(token)
            .build()
            .map_err(|e| {
                SecretsError::Provider(format!("Failed to build Vault client settings: {e}"))
            })?;

        let client = VaultClient::new(settings)
            .map_err(|e| SecretsError::Provider(format!("Failed to create Vault client: {e}")))?;

        Ok(Self {
            client,
            mount: mount.to_string(),
        })
    }

    /// Creates a new Vault secrets provider from environment variables.
    ///
    /// Reads configuration from:
    /// - `VAULT_ADDR` - Vault server address (required)
    /// - `VAULT_TOKEN` - Vault authentication token (required)
    /// - `VAULT_MOUNT` - KV2 mount path (optional, defaults to "secret")
    ///
    /// # Errors
    ///
    /// Returns `SecretsError::Provider` if required environment variables
    /// are missing or if the Vault client cannot be created.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use zlayer_secrets::VaultSecretsProvider;
    ///
    /// // Requires VAULT_ADDR and VAULT_TOKEN environment variables
    /// let provider = VaultSecretsProvider::from_env()?;
    /// # Ok::<(), zlayer_secrets::SecretsError>(())
    /// ```
    pub fn from_env() -> Result<Self> {
        let address = std::env::var(ENV_VAULT_ADDR).map_err(|_| {
            SecretsError::Provider(format!(
                "Missing required environment variable: {ENV_VAULT_ADDR}"
            ))
        })?;

        let token = std::env::var(ENV_VAULT_TOKEN).map_err(|_| {
            SecretsError::Provider(format!(
                "Missing required environment variable: {ENV_VAULT_TOKEN}"
            ))
        })?;

        let mount = std::env::var(ENV_VAULT_MOUNT).unwrap_or_else(|_| DEFAULT_MOUNT.to_string());

        Self::new(&address, &token, &mount)
    }

    /// Builds the full path for a secret in Vault.
    ///
    /// Combines the scope and name to form the path within the KV2 mount.
    #[allow(clippy::unused_self)]
    fn build_path(&self, scope: &str, name: &str) -> String {
        format!("{scope}/{name}")
    }
}

#[cfg(feature = "vault")]
#[async_trait]
impl SecretsProvider for VaultSecretsProvider {
    async fn get_secret(&self, scope: &str, name: &str) -> Result<Secret> {
        let path = self.build_path(scope, name);

        match kv2::read::<VaultSecretData>(&self.client, &self.mount, &path).await {
            Ok(data) => Ok(Secret::new(data.value)),
            Err(e) => {
                // Check if it's a "not found" error
                let error_string = e.to_string();
                if error_string.contains("404") || error_string.contains("not found") {
                    Err(SecretsError::NotFound {
                        name: format!("{scope}/{name}"),
                    })
                } else {
                    Err(SecretsError::Provider(format!(
                        "Failed to read secret from Vault at {path}: {e}"
                    )))
                }
            }
        }
    }

    async fn get_secrets(&self, scope: &str, names: &[&str]) -> Result<HashMap<String, Secret>> {
        let mut results = HashMap::with_capacity(names.len());

        for name in names {
            match self.get_secret(scope, name).await {
                Ok(secret) => {
                    results.insert((*name).to_string(), secret);
                }
                Err(SecretsError::NotFound { .. }) => {
                    // Skip secrets that don't exist, per trait contract
                }
                Err(e) => return Err(e),
            }
        }

        Ok(results)
    }

    async fn list_secrets(&self, scope: &str) -> Result<Vec<SecretMetadata>> {
        match kv2::list(&self.client, &self.mount, scope).await {
            Ok(keys) => {
                let metadata: Vec<SecretMetadata> = keys
                    .into_iter()
                    .filter(|key| !key.ends_with('/')) // Filter out directories
                    .map(SecretMetadata::new)
                    .collect();

                Ok(metadata)
            }
            Err(e) => {
                // Check if it's a "not found" error (empty scope)
                let error_string = e.to_string();
                if error_string.contains("404") || error_string.contains("not found") {
                    Ok(Vec::new())
                } else {
                    Err(SecretsError::Provider(format!(
                        "Failed to list secrets from Vault at {scope}: {e}"
                    )))
                }
            }
        }
    }

    async fn exists(&self, scope: &str, name: &str) -> Result<bool> {
        match self.get_secret(scope, name).await {
            Ok(_) => Ok(true),
            Err(SecretsError::NotFound { .. }) => Ok(false),
            Err(e) => Err(e),
        }
    }
}

#[cfg(feature = "vault")]
#[async_trait]
impl SecretsStore for VaultSecretsProvider {
    async fn set_secret(&self, scope: &str, name: &str, secret: &Secret) -> Result<()> {
        let path = self.build_path(scope, name);
        let data = VaultSecretData {
            value: secret.expose().to_string(),
        };

        kv2::set(&self.client, &self.mount, &path, &data)
            .await
            .map_err(|e| {
                SecretsError::Provider(format!("Failed to write secret to Vault at {path}: {e}"))
            })?;

        Ok(())
    }

    async fn delete_secret(&self, scope: &str, name: &str) -> Result<()> {
        let path = self.build_path(scope, name);

        // First check if the secret exists
        if !self.exists(scope, name).await? {
            return Err(SecretsError::NotFound {
                name: format!("{scope}/{name}"),
            });
        }

        kv2::delete_latest(&self.client, &self.mount, &path)
            .await
            .map_err(|e| {
                SecretsError::Provider(format!("Failed to delete secret from Vault at {path}: {e}"))
            })?;

        Ok(())
    }
}

#[cfg(all(test, feature = "vault"))]
mod tests {
    use super::*;

    #[test]
    fn test_build_path() {
        let provider = VaultSecretsProvider {
            client: create_test_client(),
            mount: "secret".to_string(),
        };

        assert_eq!(
            provider.build_path("deployments/myapp/secrets", "database-password"),
            "deployments/myapp/secrets/database-password"
        );

        assert_eq!(
            provider.build_path("services/api", "api-key"),
            "services/api/api-key"
        );
    }

    #[test]
    fn test_from_env_missing_addr() {
        // Ensure VAULT_ADDR is not set
        std::env::remove_var(ENV_VAULT_ADDR);
        std::env::remove_var(ENV_VAULT_TOKEN);

        let result = VaultSecretsProvider::from_env();
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(err.to_string().contains("VAULT_ADDR"));
    }

    #[test]
    fn test_from_env_missing_token() {
        std::env::set_var(ENV_VAULT_ADDR, "https://localhost:8200");
        std::env::remove_var(ENV_VAULT_TOKEN);

        let result = VaultSecretsProvider::from_env();
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(err.to_string().contains("VAULT_TOKEN"));

        // Cleanup
        std::env::remove_var(ENV_VAULT_ADDR);
    }

    fn create_test_client() -> VaultClient {
        let settings = VaultClientSettingsBuilder::default()
            .address("https://localhost:8200")
            .token("test-token")
            .build()
            .unwrap();

        VaultClient::new(settings).unwrap()
    }
}
