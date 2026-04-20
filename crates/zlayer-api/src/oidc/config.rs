//! OIDC provider configuration loaded from environment variables.
//!
//! Each provider is namespaced by an arbitrary `<NAME>` token, e.g. `GOOGLE`,
//! `OKTA`, `AUTHENTIK`. The loader enumerates all `ZLAYER_OIDC_*_CLIENT_ID`
//! envs and builds one [`OidcProviderConfig`] per distinct name.

use serde::{Deserialize, Serialize};
use thiserror::Error;
use utoipa::ToSchema;

/// Errors returned by the env-var loader. Fail-loud on any misconfiguration —
/// it's better to refuse to boot than to leave a provider half-configured.
#[derive(Debug, Error)]
pub enum OidcConfigError {
    #[error("{env}: required env var is unset")]
    Missing { env: String },

    #[error("{env}: invalid URL: {reason}")]
    InvalidUrl { env: String, reason: String },

    #[error("provider name cannot be empty")]
    EmptyProviderName,
}

const ENV_PREFIX: &str = "ZLAYER_OIDC_";

/// Fully resolved configuration for a single OIDC provider.
#[derive(Debug, Clone)]
pub struct OidcProviderConfig {
    /// Slug used in URLs (`/auth/oidc/<name>/start`). Always lowercase.
    pub name: String,
    /// Human-readable label for UI buttons ("Sign in with X"). Defaults
    /// to a prettified version of `name` when unset.
    pub display_name: String,
    /// OIDC issuer URL — root of the `.well-known/openid-configuration`
    /// document. E.g. `https://accounts.google.com`.
    pub issuer: String,
    /// OAuth2 client id registered with the provider.
    pub client_id: String,
    /// OAuth2 client secret. Never serialised.
    pub client_secret: String,
    /// Redirect URI registered with the provider. Must exactly match the
    /// one configured on the provider side, including scheme + path.
    pub redirect_url: String,
    /// OAuth2 scopes requested. Always includes `openid`.
    pub scopes: Vec<String>,
}

/// Public-facing projection — what the Manager UI needs to render a login
/// button. Never includes the client secret.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct OidcProviderPublic {
    pub name: String,
    pub display_name: String,
}

impl From<&OidcProviderConfig> for OidcProviderPublic {
    fn from(c: &OidcProviderConfig) -> Self {
        Self {
            name: c.name.clone(),
            display_name: c.display_name.clone(),
        }
    }
}

impl OidcProviderConfig {
    /// Enumerate all configured providers from the process environment.
    /// A provider is considered present if its `_CLIENT_ID` is set; missing
    /// companion vars then fail-loud.
    pub fn load_all() -> Result<Vec<OidcProviderConfig>, OidcConfigError> {
        let mut names: Vec<String> = std::env::vars()
            .filter_map(|(k, _)| {
                let rest = k.strip_prefix(ENV_PREFIX)?;
                let name = rest.strip_suffix("_CLIENT_ID")?;
                if name.is_empty() {
                    return None;
                }
                Some(name.to_ascii_uppercase())
            })
            .collect();
        names.sort();
        names.dedup();

        names.iter().map(|n| Self::load_one(n)).collect()
    }

    fn load_one(raw_name: &str) -> Result<OidcProviderConfig, OidcConfigError> {
        if raw_name.is_empty() {
            return Err(OidcConfigError::EmptyProviderName);
        }
        let env = |suffix: &str| format!("{ENV_PREFIX}{raw_name}_{suffix}");
        let require = |suffix: &str| -> Result<String, OidcConfigError> {
            let k = env(suffix);
            std::env::var(&k).map_err(|_| OidcConfigError::Missing { env: k })
        };
        let optional = |suffix: &str| -> Option<String> { std::env::var(env(suffix)).ok() };

        let issuer = require("ISSUER")?;
        if url::Url::parse(&issuer).is_err() {
            return Err(OidcConfigError::InvalidUrl {
                env: env("ISSUER"),
                reason: "not a valid URL".to_string(),
            });
        }

        let client_id = require("CLIENT_ID")?;
        let client_secret = require("CLIENT_SECRET")?;
        let redirect_url = require("REDIRECT_URL")?;
        if url::Url::parse(&redirect_url).is_err() {
            return Err(OidcConfigError::InvalidUrl {
                env: env("REDIRECT_URL"),
                reason: "not a valid URL".to_string(),
            });
        }

        let display_name = optional("DISPLAY_NAME").unwrap_or_else(|| prettify(raw_name));

        let scopes = optional("SCOPES").map_or_else(
            || {
                vec![
                    "openid".to_string(),
                    "email".to_string(),
                    "profile".to_string(),
                ]
            },
            |s| {
                s.split(|c: char| c == ',' || c.is_whitespace())
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            },
        );
        let scopes = ensure_openid(scopes);

        Ok(OidcProviderConfig {
            name: raw_name.to_ascii_lowercase(),
            display_name,
            issuer,
            client_id,
            client_secret,
            redirect_url,
            scopes,
        })
    }
}

fn prettify(raw: &str) -> String {
    raw.split('_')
        .filter(|s| !s.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(c) => {
                    c.to_ascii_uppercase().to_string() + &chars.as_str().to_ascii_lowercase()
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn ensure_openid(mut scopes: Vec<String>) -> Vec<String> {
    if !scopes.iter().any(|s| s == "openid") {
        scopes.insert(0, "openid".to_string());
    }
    scopes
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    fn clear(name: &str) {
        for suffix in [
            "ISSUER",
            "CLIENT_ID",
            "CLIENT_SECRET",
            "REDIRECT_URL",
            "DISPLAY_NAME",
            "SCOPES",
        ] {
            std::env::remove_var(format!("{ENV_PREFIX}{name}_{suffix}"));
        }
    }

    fn set_complete(name: &str) {
        clear(name);
        std::env::set_var(format!("{ENV_PREFIX}{name}_ISSUER"), "https://example.com");
        std::env::set_var(format!("{ENV_PREFIX}{name}_CLIENT_ID"), "cid");
        std::env::set_var(format!("{ENV_PREFIX}{name}_CLIENT_SECRET"), "csecret");
        std::env::set_var(
            format!("{ENV_PREFIX}{name}_REDIRECT_URL"),
            "https://app.example.com/auth/oidc/cb",
        );
    }

    #[test]
    #[serial]
    fn load_one_happy_path_defaults() {
        set_complete("GOOGLE");
        let p = OidcProviderConfig::load_one("GOOGLE").unwrap();
        assert_eq!(p.name, "google");
        assert_eq!(p.display_name, "Google");
        assert_eq!(p.scopes, vec!["openid", "email", "profile"]);
        clear("GOOGLE");
    }

    #[test]
    #[serial]
    fn load_one_custom_display_and_scopes() {
        set_complete("OKTA");
        std::env::set_var(format!("{ENV_PREFIX}OKTA_DISPLAY_NAME"), "Corp SSO");
        std::env::set_var(format!("{ENV_PREFIX}OKTA_SCOPES"), "email,groups");
        let p = OidcProviderConfig::load_one("OKTA").unwrap();
        assert_eq!(p.display_name, "Corp SSO");
        // openid is auto-prepended
        assert_eq!(p.scopes, vec!["openid", "email", "groups"]);
        clear("OKTA");
    }

    #[test]
    #[serial]
    fn load_one_missing_client_secret_fails() {
        set_complete("AZURE");
        std::env::remove_var(format!("{ENV_PREFIX}AZURE_CLIENT_SECRET"));
        let err = OidcProviderConfig::load_one("AZURE").unwrap_err();
        assert!(matches!(err, OidcConfigError::Missing { .. }));
        clear("AZURE");
    }

    #[test]
    #[serial]
    fn load_one_bad_issuer_url_fails() {
        set_complete("BAD");
        std::env::set_var(format!("{ENV_PREFIX}BAD_ISSUER"), "not a url");
        let err = OidcProviderConfig::load_one("BAD").unwrap_err();
        assert!(matches!(err, OidcConfigError::InvalidUrl { .. }));
        clear("BAD");
    }

    #[test]
    #[serial]
    fn load_all_enumerates_multiple() {
        set_complete("ALPHA");
        set_complete("BETA");
        let providers = OidcProviderConfig::load_all().unwrap();
        let names: Vec<_> = providers.iter().map(|p| p.name.clone()).collect();
        assert!(names.contains(&"alpha".to_string()));
        assert!(names.contains(&"beta".to_string()));
        clear("ALPHA");
        clear("BETA");
    }

    #[test]
    fn prettify_underscore_names() {
        assert_eq!(prettify("CORP_SSO"), "Corp Sso");
        assert_eq!(prettify("G"), "G");
    }

    #[test]
    fn ensure_openid_prepends_if_missing() {
        let scopes = ensure_openid(vec!["email".to_string()]);
        assert_eq!(scopes[0], "openid");
    }
}
