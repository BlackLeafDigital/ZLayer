//! Environment variable resolution for $E: prefix syntax
//!
//! Per V1_SPEC.md Section 7, values prefixed with `$E:` are resolved
//! from the runtime environment at container start time.

use std::collections::HashMap;
use thiserror::Error;

/// Prefix that indicates a value should be resolved from host environment
const ENV_REF_PREFIX: &str = "$E:";

/// Errors that can occur during environment variable resolution
#[derive(Error, Debug, Clone)]
pub enum EnvResolutionError {
    #[error("environment variable '{var}' referenced by $E:{var} is not set")]
    MissingEnvVar { var: String },
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
/// use agent::env::resolve_env_vars;
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
}
