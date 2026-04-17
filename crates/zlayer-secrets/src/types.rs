//! Secret types for `ZLayer` secrets management.
//!
//! This module provides secure secret handling with proper memory cleanup
//! and redacted debug output.

use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

/// A secure secret wrapper that provides memory safety guarantees.
///
/// - Implements `Zeroize` for secure memory cleanup on drop
/// - Debug output shows `[REDACTED]` instead of the actual value
/// - Uses `SecretString` from the secrecy crate for the underlying storage
#[derive(Clone)]
pub struct Secret {
    inner: SecretString,
}

impl Secret {
    /// Create a new secret from a string value.
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            inner: SecretString::from(value.into()),
        }
    }

    /// Expose the secret value for use.
    ///
    /// This should only be called when the secret value is actually needed,
    /// such as when passing to an external service or writing to an encrypted store.
    #[must_use]
    pub fn expose(&self) -> &str {
        self.inner.expose_secret()
    }

    /// Get the underlying `SecretString` reference.
    #[must_use]
    pub fn as_secret_string(&self) -> &SecretString {
        &self.inner
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl Zeroize for Secret {
    fn zeroize(&mut self) {
        // SecretString handles its own zeroization on drop,
        // but we implement the trait for explicit zeroize calls.
        // We replace with an empty secret to trigger cleanup.
        self.inner = SecretString::from(String::new());
    }
}

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for Secret {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<SecretString> for Secret {
    fn from(value: SecretString) -> Self {
        Self { inner: value }
    }
}

/// Metadata associated with a stored secret.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SecretMetadata {
    /// The name/identifier of the secret.
    pub name: String,

    /// Unix timestamp when the secret was created.
    pub created_at: i64,

    /// Unix timestamp when the secret was last updated.
    pub updated_at: i64,

    /// Version number of the secret (incremented on each update).
    pub version: u32,
}

impl SecretMetadata {
    /// Create new metadata for a secret.
    #[allow(clippy::cast_possible_wrap)]
    pub fn new(name: impl Into<String>) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        Self {
            name: name.into(),
            created_at: now,
            updated_at: now,
            version: 1,
        }
    }

    /// Update the metadata for a secret modification.
    #[allow(clippy::cast_possible_wrap)]
    pub fn update(&mut self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        self.updated_at = now;
        self.version = self.version.saturating_add(1);
    }
}

/// The scope of a secret - determines visibility and access.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SecretScope {
    /// Deployment-level secret, accessible by all services in the deployment.
    Deployment(String),

    /// Service-level secret, accessible only by the specified service.
    Service {
        /// The deployment this service belongs to.
        deployment: String,
        /// The specific service name.
        service: String,
    },
}

impl SecretScope {
    /// Create a deployment-scoped secret scope.
    pub fn deployment(name: impl Into<String>) -> Self {
        Self::Deployment(name.into())
    }

    /// Create a service-scoped secret scope.
    pub fn service(deployment: impl Into<String>, service: impl Into<String>) -> Self {
        Self::Service {
            deployment: deployment.into(),
            service: service.into(),
        }
    }

    /// Generate a key prefix for storage lookups.
    ///
    /// Returns a path-like prefix that can be used to organize secrets
    /// in a hierarchical store.
    #[must_use]
    pub fn to_key_prefix(&self) -> String {
        match self {
            Self::Deployment(deployment) => format!("deployments/{deployment}/secrets"),
            Self::Service {
                deployment,
                service,
            } => format!("deployments/{deployment}/services/{service}/secrets"),
        }
    }

    /// Get the deployment name for this scope.
    #[must_use]
    pub fn deployment_name(&self) -> &str {
        match self {
            Self::Deployment(name) => name,
            Self::Service { deployment, .. } => deployment,
        }
    }

    /// Get the service name if this is a service-scoped secret.
    #[must_use]
    pub fn service_name(&self) -> Option<&str> {
        match self {
            Self::Deployment(_) => None,
            Self::Service { service, .. } => Some(service),
        }
    }
}

/// A reference to a secret, parsed from the `$S:` prefix syntax.
///
/// ## Formats
/// - `$S:name` - Deployment-level secret
/// - `$S:name/field` - Deployment-level secret with JSON field extraction
/// - `$S:@service/name[/field]` - Service-level secret
/// - `$S::env/name[/field]` - Environment-scoped secret (global environment)
/// - `$S:project:env/name[/field]` - Project-scoped environment secret
///
/// ## Environment-scope disambiguation
/// Environment-scoped references require a colon in the scope segment so
/// they cannot be confused with the legacy deployment-level `name/field`
/// form. The leading colon in `$S::env/name` marks "no project, scope is
/// environment `env`". The `$S:project:env/name` form is unambiguous
/// because it already contains a colon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretRef {
    /// The name of the secret.
    pub name: String,

    /// Optional service name (for service-scoped secrets).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,

    /// Optional project name (for project-scoped environment secrets).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,

    /// Optional environment name (for environment-scoped secrets).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,

    /// Optional field to extract from a structured secret (e.g., JSON).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

impl SecretRef {
    /// The prefix used to identify secret references in configuration values.
    pub const PREFIX: &'static str = "$S:";

    /// Check if a string value is a secret reference.
    #[must_use]
    pub fn is_secret_ref(value: &str) -> bool {
        value.starts_with(Self::PREFIX)
    }

    /// Parse a secret reference from a string.
    ///
    /// ## Formats
    /// - `$S:name` - Deployment-level secret
    /// - `$S:name/field` - Deployment-level secret with JSON field extraction
    /// - `$S:@service/name[/field]` - Service-level secret
    /// - `$S::env/name[/field]` - Environment-scoped secret (global environment)
    /// - `$S:project:env/name[/field]` - Project-scoped environment secret
    ///
    /// Returns `None` if the string is not a valid secret reference.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        // Must start with the prefix
        let rest = value.strip_prefix(Self::PREFIX)?;

        // Empty reference is invalid
        if rest.is_empty() {
            return None;
        }

        // Service-scoped: $S:@service/name[/field]
        if let Some(service_rest) = rest.strip_prefix('@') {
            let mut parts = service_rest.splitn(3, '/');

            let service = parts.next()?;
            if service.is_empty() {
                return None;
            }

            let name = parts.next()?;
            if name.is_empty() {
                return None;
            }

            let field = parts
                .next()
                .map(ToString::to_string)
                .filter(|s| !s.is_empty());

            return Some(Self {
                name: name.to_string(),
                service: Some(service.to_string()),
                project: None,
                environment: None,
                field,
            });
        }

        // Reject leading '/' (e.g. "$S:/name"): no scope/name segment.
        if rest.starts_with('/') {
            return None;
        }

        // Environment-scoped forms: the scope segment (everything before the
        // first '/') must contain a ':'. This disambiguates against the
        // legacy deployment-level `name/field` form, which contains no ':'.
        // Supported:
        //   $S::env/name[/field]        -> environment-only (empty project)
        //   $S:project:env/name[/field] -> project-scoped environment
        if let Some((scope, tail)) = rest.split_once('/') {
            if scope.contains(':') {
                return Self::parse_env_scoped(scope, tail);
            }
            // Legacy deployment-level with field: name/field
            if scope.is_empty() {
                return None;
            }
            // Reject stray ':' inside a plain name (e.g. "$S:foo:bar/baz")
            // is handled above (scope contains ':'); also reject the "name
            // only" form when it contains colons further down.
            if tail.is_empty() {
                return None;
            }
            // Tail itself must not contain further '/' after the field
            // (legacy grammar is exactly `name/field`).
            if tail.contains('/') {
                return None;
            }
            return Some(Self {
                name: scope.to_string(),
                service: None,
                project: None,
                environment: None,
                field: Some(tail.to_string()),
            });
        }

        // No '/' in rest: it's either a plain deployment name or malformed.
        // A name containing ':' is treated as malformed (e.g. "$S:::name").
        if rest.contains(':') {
            return None;
        }
        Some(Self {
            name: rest.to_string(),
            service: None,
            project: None,
            environment: None,
            field: None,
        })
    }

    /// Parse the environment-scoped forms (scope segment contains ':').
    ///
    /// `scope` is the pre-'/' segment, `tail` is everything after the first
    /// '/', which must contain at least a non-empty name, optionally
    /// followed by `/field`.
    fn parse_env_scoped(scope: &str, tail: &str) -> Option<Self> {
        // Extract the project/env split.
        let (project_raw, environment) = scope.split_once(':')?;
        // Reject environments containing further colons (e.g. "a:b:c").
        if environment.is_empty() || environment.contains(':') {
            return None;
        }
        let project = if project_raw.is_empty() {
            None
        } else {
            Some(project_raw.to_string())
        };

        // Split tail into name and optional field.
        let (name, field) = match tail.split_once('/') {
            Some((name, field)) => {
                if field.is_empty() || field.contains('/') {
                    return None;
                }
                (name, Some(field.to_string()))
            }
            None => (tail, None),
        };

        if name.is_empty() {
            return None;
        }

        Some(Self {
            name: name.to_string(),
            service: None,
            project,
            environment: Some(environment.to_string()),
            field,
        })
    }

    /// Convert this reference to a `SecretScope` for a given deployment.
    ///
    /// Environment/project scopes are not representable by `SecretScope`
    /// (which only models deployment vs service); those references fall
    /// back to a deployment-level scope and must be resolved by a higher
    /// layer that understands environment and project scoping.
    #[must_use]
    pub fn to_scope(&self, deployment: &str) -> SecretScope {
        match &self.service {
            Some(service) => SecretScope::Service {
                deployment: deployment.to_string(),
                service: service.clone(),
            },
            None => SecretScope::Deployment(deployment.to_string()),
        }
    }

    /// Check if this is a deployment-level secret reference.
    #[must_use]
    pub fn is_deployment_level(&self) -> bool {
        self.service.is_none() && self.project.is_none() && self.environment.is_none()
    }

    /// Check if this is a service-level secret reference.
    #[must_use]
    pub fn is_service_level(&self) -> bool {
        self.service.is_some()
    }

    /// Check if this reference is environment-scoped (with or without a project).
    #[must_use]
    pub fn is_environment_level(&self) -> bool {
        self.environment.is_some()
    }

    /// Check if this reference is scoped to a project environment.
    #[must_use]
    pub fn is_project_environment_level(&self) -> bool {
        self.project.is_some() && self.environment.is_some()
    }

    /// Check if this reference includes field extraction.
    #[must_use]
    pub fn has_field(&self) -> bool {
        self.field.is_some()
    }
}

impl std::fmt::Display for SecretRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(Self::PREFIX)?;
        if let Some(service) = &self.service {
            write!(f, "@{service}/{}", self.name)?;
            if let Some(field) = &self.field {
                write!(f, "/{field}")?;
            }
        } else if let Some(environment) = &self.environment {
            if let Some(project) = &self.project {
                write!(f, "{project}:{environment}/{}", self.name)?;
            } else {
                write!(f, ":{environment}/{}", self.name)?;
            }
            if let Some(field) = &self.field {
                write!(f, "/{field}")?;
            }
        } else {
            // Deployment-level: `name` or `name/field`.
            f.write_str(&self.name)?;
            if let Some(field) = &self.field {
                write!(f, "/{field}")?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_secret_debug_redacted() {
        let secret = Secret::new("super-secret-value");
        let debug_output = format!("{secret:?}");
        assert_eq!(debug_output, "[REDACTED]");
        assert!(!debug_output.contains("super-secret-value"));
    }

    #[test]
    fn test_secret_expose() {
        let secret = Secret::new("my-secret");
        assert_eq!(secret.expose(), "my-secret");
    }

    #[test]
    fn test_secret_from_string() {
        let secret: Secret = "test-secret".into();
        assert_eq!(secret.expose(), "test-secret");

        let secret: Secret = String::from("another-secret").into();
        assert_eq!(secret.expose(), "another-secret");
    }

    #[test]
    fn test_secret_zeroize() {
        let mut secret = Secret::new("sensitive-data");
        secret.zeroize();
        // After zeroize, the secret should be empty
        assert_eq!(secret.expose(), "");
    }

    #[test]
    fn test_secret_metadata_new() {
        let metadata = SecretMetadata::new("test-secret");
        assert_eq!(metadata.name, "test-secret");
        assert_eq!(metadata.version, 1);
        assert!(metadata.created_at > 0);
        assert_eq!(metadata.created_at, metadata.updated_at);
    }

    #[test]
    fn test_secret_metadata_update() {
        let mut metadata = SecretMetadata::new("test-secret");
        let original_created = metadata.created_at;
        let original_version = metadata.version;

        // Simulate time passing
        std::thread::sleep(std::time::Duration::from_millis(10));
        metadata.update();

        assert_eq!(metadata.created_at, original_created);
        assert!(metadata.updated_at >= original_created);
        assert_eq!(metadata.version, original_version + 1);
    }

    #[test]
    fn test_secret_scope_deployment() {
        let scope = SecretScope::deployment("my-deployment");
        assert_eq!(scope.deployment_name(), "my-deployment");
        assert!(scope.service_name().is_none());
        assert_eq!(scope.to_key_prefix(), "deployments/my-deployment/secrets");
    }

    #[test]
    fn test_secret_scope_service() {
        let scope = SecretScope::service("my-deployment", "my-service");
        assert_eq!(scope.deployment_name(), "my-deployment");
        assert_eq!(scope.service_name(), Some("my-service"));
        assert_eq!(
            scope.to_key_prefix(),
            "deployments/my-deployment/services/my-service/secrets"
        );
    }

    #[test]
    fn test_secret_ref_is_secret_ref() {
        assert!(SecretRef::is_secret_ref("$S:my-secret"));
        assert!(SecretRef::is_secret_ref("$S:@service/secret"));
        assert!(!SecretRef::is_secret_ref("my-secret"));
        assert!(!SecretRef::is_secret_ref("S:my-secret"));
        assert!(!SecretRef::is_secret_ref("$:my-secret"));
    }

    #[test]
    fn test_secret_ref_parse_deployment_level() {
        let secret_ref = SecretRef::parse("$S:database-password").unwrap();
        assert_eq!(secret_ref.name, "database-password");
        assert!(secret_ref.service.is_none());
        assert!(secret_ref.project.is_none());
        assert!(secret_ref.environment.is_none());
        assert!(secret_ref.field.is_none());
        assert!(secret_ref.is_deployment_level());
    }

    #[test]
    fn test_secret_ref_parse_service_level() {
        let secret_ref = SecretRef::parse("$S:@api/database-password").unwrap();
        assert_eq!(secret_ref.name, "database-password");
        assert_eq!(secret_ref.service, Some("api".to_string()));
        assert!(secret_ref.project.is_none());
        assert!(secret_ref.environment.is_none());
        assert!(secret_ref.field.is_none());
        assert!(secret_ref.is_service_level());
    }

    #[test]
    fn test_secret_ref_parse_service_level_with_field() {
        let secret_ref = SecretRef::parse("$S:@api/database/password").unwrap();
        assert_eq!(secret_ref.name, "database");
        assert_eq!(secret_ref.service, Some("api".to_string()));
        assert_eq!(secret_ref.field, Some("password".to_string()));
        assert!(secret_ref.has_field());
    }

    #[test]
    fn test_secret_ref_parse_deployment_with_field_legacy() {
        // Legacy deployment-level with field: `name/field`.
        let secret_ref = SecretRef::parse("$S:database/password").unwrap();
        assert_eq!(secret_ref.name, "database");
        assert!(secret_ref.service.is_none());
        assert!(secret_ref.project.is_none());
        assert!(secret_ref.environment.is_none());
        assert_eq!(secret_ref.field, Some("password".to_string()));
        assert!(secret_ref.has_field());
    }

    #[test]
    fn test_secret_ref_parse_environment_level() {
        let secret_ref = SecretRef::parse("$S::staging/db-password").unwrap();
        assert_eq!(secret_ref.name, "db-password");
        assert_eq!(secret_ref.environment, Some("staging".to_string()));
        assert!(secret_ref.project.is_none());
        assert!(secret_ref.service.is_none());
        assert!(secret_ref.field.is_none());
        assert!(secret_ref.is_environment_level());
        assert!(!secret_ref.is_project_environment_level());
        assert!(!secret_ref.is_deployment_level());
    }

    #[test]
    fn test_secret_ref_parse_environment_level_with_field() {
        let secret_ref = SecretRef::parse("$S::staging/db-creds/password").unwrap();
        assert_eq!(secret_ref.name, "db-creds");
        assert_eq!(secret_ref.environment, Some("staging".to_string()));
        assert!(secret_ref.project.is_none());
        assert_eq!(secret_ref.field, Some("password".to_string()));
    }

    #[test]
    fn test_secret_ref_parse_project_environment_level() {
        let secret_ref = SecretRef::parse("$S:myproj:staging/db-password").unwrap();
        assert_eq!(secret_ref.name, "db-password");
        assert_eq!(secret_ref.project, Some("myproj".to_string()));
        assert_eq!(secret_ref.environment, Some("staging".to_string()));
        assert!(secret_ref.service.is_none());
        assert!(secret_ref.field.is_none());
        assert!(secret_ref.is_environment_level());
        assert!(secret_ref.is_project_environment_level());
    }

    #[test]
    fn test_secret_ref_parse_project_environment_with_field() {
        let secret_ref = SecretRef::parse("$S:myproj:prod/creds/api_key").unwrap();
        assert_eq!(secret_ref.name, "creds");
        assert_eq!(secret_ref.project, Some("myproj".to_string()));
        assert_eq!(secret_ref.environment, Some("prod".to_string()));
        assert_eq!(secret_ref.field, Some("api_key".to_string()));
    }

    #[test]
    fn test_secret_ref_parse_invalid() {
        // No prefix
        assert!(SecretRef::parse("database-password").is_none());

        // Empty after prefix
        assert!(SecretRef::parse("$S:").is_none());

        // Empty service name
        assert!(SecretRef::parse("$S:@/secret").is_none());

        // Empty secret name after service
        assert!(SecretRef::parse("$S:@service/").is_none());

        // Just @ with no service
        assert!(SecretRef::parse("$S:@").is_none());

        // Leading slash (empty scope/name)
        assert!(SecretRef::parse("$S:/name").is_none());

        // Empty name in legacy `name/field` form
        assert!(SecretRef::parse("$S:database/").is_none());

        // Triple-colon flat form (no '/'): colons disallowed in bare names
        assert!(SecretRef::parse("$S:::name").is_none());

        // Empty environment in env-only form ("$S::/name")
        assert!(SecretRef::parse("$S::/name").is_none());

        // Empty environment with project ("proj:/name")
        assert!(SecretRef::parse("$S:proj:/name").is_none());

        // Extra colon in scope ("a:b:c/name")
        assert!(SecretRef::parse("$S:a:b:c/name").is_none());

        // Env-scoped with empty name
        assert!(SecretRef::parse("$S::env/").is_none());

        // Extra trailing segment on legacy deployment form
        assert!(SecretRef::parse("$S:name/field/extra").is_none());
    }

    #[test]
    fn test_secret_ref_display_roundtrip() {
        let cases = [
            "$S:database-password",
            "$S:database/password",
            "$S:@api/database-password",
            "$S:@api/database/password",
            "$S::staging/db-password",
            "$S::staging/db-creds/password",
            "$S:myproj:staging/db-password",
            "$S:myproj:prod/creds/api_key",
        ];

        for input in cases {
            let parsed =
                SecretRef::parse(input).unwrap_or_else(|| panic!("failed to parse {input}"));
            let rendered = parsed.to_string();
            assert_eq!(rendered, input, "round-trip mismatch for {input}");
            let reparsed = SecretRef::parse(&rendered)
                .unwrap_or_else(|| panic!("failed to re-parse {rendered}"));
            assert_eq!(parsed, reparsed);
        }
    }

    #[test]
    fn test_secret_ref_serde_backcompat() {
        // Pre-existing JSON without project/environment fields must still deserialize.
        let json = r#"{"name":"db","service":"api","field":"password"}"#;
        let parsed: SecretRef = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.name, "db");
        assert_eq!(parsed.service, Some("api".to_string()));
        assert_eq!(parsed.field, Some("password".to_string()));
        assert!(parsed.project.is_none());
        assert!(parsed.environment.is_none());

        // Minimal form (just a name).
        let minimal = r#"{"name":"db"}"#;
        let parsed: SecretRef = serde_json::from_str(minimal).unwrap();
        assert_eq!(parsed.name, "db");
        assert!(parsed.service.is_none());
        assert!(parsed.project.is_none());
        assert!(parsed.environment.is_none());
        assert!(parsed.field.is_none());
    }

    #[test]
    fn test_secret_ref_to_scope() {
        // Deployment-level
        let secret_ref = SecretRef::parse("$S:my-secret").unwrap();
        let scope = secret_ref.to_scope("prod");
        assert_eq!(scope, SecretScope::Deployment("prod".to_string()));

        // Service-level
        let secret_ref = SecretRef::parse("$S:@api/my-secret").unwrap();
        let scope = secret_ref.to_scope("prod");
        assert_eq!(
            scope,
            SecretScope::Service {
                deployment: "prod".to_string(),
                service: "api".to_string(),
            }
        );
    }

    #[test]
    fn test_secret_metadata_serialization() {
        let metadata = SecretMetadata {
            name: "test".to_string(),
            created_at: 1_234_567_890,
            updated_at: 1_234_567_900,
            version: 5,
        };

        let json = serde_json::to_string(&metadata).unwrap();
        let deserialized: SecretMetadata = serde_json::from_str(&json).unwrap();

        assert_eq!(metadata, deserialized);
    }

    #[test]
    fn test_secret_scope_serialization() {
        let deployment_scope = SecretScope::deployment("my-deploy");
        let json = serde_json::to_string(&deployment_scope).unwrap();
        let deserialized: SecretScope = serde_json::from_str(&json).unwrap();
        assert_eq!(deployment_scope, deserialized);

        let service_scope = SecretScope::service("my-deploy", "my-service");
        let json = serde_json::to_string(&service_scope).unwrap();
        let deserialized: SecretScope = serde_json::from_str(&json).unwrap();
        assert_eq!(service_scope, deserialized);
    }
}
