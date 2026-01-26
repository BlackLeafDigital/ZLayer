//! Variable expansion for Dockerfile ARG and ENV values
//!
//! This module provides functionality to expand variables in Dockerfile strings,
//! supporting the following syntax:
//!
//! - `$VAR` - Simple variable reference
//! - `${VAR}` - Explicit variable reference
//! - `${VAR:-default}` - Use default if VAR is unset or empty
//! - `${VAR:+alternate}` - Use alternate if VAR is set and non-empty
//! - `${VAR-default}` - Use default if VAR is unset (empty string is valid)
//! - `${VAR+alternate}` - Use alternate if VAR is set (including empty)

use std::collections::HashMap;

/// Expand variables in a string using the provided ARG and ENV maps.
///
/// Variables are expanded in the following order of precedence:
/// 1. Build-time ARGs (provided at build time)
/// 2. ENV variables (from Dockerfile ENV instructions)
/// 3. Default ARG values (from Dockerfile ARG instructions)
///
/// # Arguments
///
/// * `input` - The string containing variable references
/// * `args` - ARG variable values (build-time and defaults)
/// * `env` - ENV variable values
///
/// # Returns
///
/// The string with all variables expanded
pub fn expand_variables(
    input: &str,
    args: &HashMap<String, String>,
    env: &HashMap<String, String>,
) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '$' {
            if let Some(&next) = chars.peek() {
                if next == '{' {
                    // ${VAR} or ${VAR:-default} or ${VAR:+alternate} format
                    chars.next(); // consume '{'
                    let (expanded, _) = expand_braced_variable(&mut chars, args, env);
                    result.push_str(&expanded);
                } else if next == '$' {
                    // Escaped dollar sign
                    chars.next();
                    result.push('$');
                } else if next.is_ascii_alphabetic() || next == '_' {
                    // $VAR format
                    let var_name = consume_var_name(&mut chars);
                    if let Some(value) = lookup_variable(&var_name, args, env) {
                        result.push_str(&value);
                    }
                    // If variable not found, expand to empty string (Docker behavior)
                } else {
                    // Just a dollar sign
                    result.push(c);
                }
            } else {
                result.push(c);
            }
        } else if c == '\\' {
            // Check for escaped dollar sign
            if let Some(&next) = chars.peek() {
                if next == '$' {
                    chars.next();
                    result.push('$');
                } else {
                    result.push(c);
                }
            } else {
                result.push(c);
            }
        } else {
            result.push(c);
        }
    }

    result
}

/// Consume a variable name from the character stream (for $VAR format)
fn consume_var_name(chars: &mut std::iter::Peekable<std::str::Chars>) -> String {
    let mut name = String::new();

    while let Some(&c) = chars.peek() {
        if c.is_ascii_alphanumeric() || c == '_' {
            name.push(c);
            chars.next();
        } else {
            break;
        }
    }

    name
}

/// Expand a braced variable expression ${...}
fn expand_braced_variable(
    chars: &mut std::iter::Peekable<std::str::Chars>,
    args: &HashMap<String, String>,
    env: &HashMap<String, String>,
) -> (String, bool) {
    let mut var_name = String::new();
    let mut operator = None;
    let mut default_value = String::new();
    let mut in_default = false;
    let mut brace_depth = 1;

    while let Some(c) = chars.next() {
        if c == '}' {
            brace_depth -= 1;
            if brace_depth == 0 {
                break;
            }
            if in_default {
                default_value.push(c);
            }
        } else if c == '{' {
            brace_depth += 1;
            if in_default {
                default_value.push(c);
            }
        } else if !in_default && (c == ':' || c == '-' || c == '+') {
            // Check for modifier operators
            if c == ':' {
                if let Some(&next) = chars.peek() {
                    if next == '-' || next == '+' {
                        chars.next();
                        operator = Some(format!(":{}", next));
                        in_default = true;
                        continue;
                    }
                }
                var_name.push(c);
            } else if c == '-' || c == '+' {
                operator = Some(c.to_string());
                in_default = true;
            } else {
                var_name.push(c);
            }
        } else if in_default {
            default_value.push(c);
        } else {
            var_name.push(c);
        }
    }

    // Look up the variable
    let value = lookup_variable(&var_name, args, env);

    match operator.as_deref() {
        Some(":-") => {
            // ${VAR:-default} - use default if unset OR empty
            match value {
                Some(v) if !v.is_empty() => (v, true),
                _ => {
                    // Recursively expand the default value
                    (expand_variables(&default_value, args, env), false)
                }
            }
        }
        Some("-") => {
            // ${VAR-default} - use default only if unset (empty is valid)
            match value {
                Some(v) => (v, true),
                None => (expand_variables(&default_value, args, env), false),
            }
        }
        Some(":+") => {
            // ${VAR:+alternate} - use alternate if set AND non-empty
            match value {
                Some(v) if !v.is_empty() => (expand_variables(&default_value, args, env), true),
                _ => (String::new(), false),
            }
        }
        Some("+") => {
            // ${VAR+alternate} - use alternate if set (including empty)
            match value {
                Some(_) => (expand_variables(&default_value, args, env), true),
                None => (String::new(), false),
            }
        }
        None | Some(_) => {
            // Simple ${VAR}
            let is_set = value.is_some();
            (value.unwrap_or_default(), is_set)
        }
    }
}

/// Look up a variable in the provided maps.
/// Checks ENV first (higher precedence), then ARGs.
fn lookup_variable(
    name: &str,
    args: &HashMap<String, String>,
    env: &HashMap<String, String>,
) -> Option<String> {
    // ENV takes precedence over ARG
    env.get(name).cloned().or_else(|| args.get(name).cloned())
}

/// Expands variables in a list of strings
pub fn expand_variables_in_list(
    inputs: &[String],
    args: &HashMap<String, String>,
    env: &HashMap<String, String>,
) -> Vec<String> {
    inputs
        .iter()
        .map(|s| expand_variables(s, args, env))
        .collect()
}

/// Builder for tracking variable state during Dockerfile processing
#[derive(Debug, Default, Clone)]
pub struct VariableContext {
    /// Build-time ARG values (from --build-arg)
    pub build_args: HashMap<String, String>,

    /// ARG default values from Dockerfile
    pub arg_defaults: HashMap<String, String>,

    /// ENV values accumulated during processing
    pub env_vars: HashMap<String, String>,
}

impl VariableContext {
    /// Create a new variable context
    pub fn new() -> Self {
        Self::default()
    }

    /// Create with build-time arguments
    pub fn with_build_args(build_args: HashMap<String, String>) -> Self {
        Self {
            build_args,
            ..Default::default()
        }
    }

    /// Register an ARG with optional default
    pub fn add_arg(&mut self, name: impl Into<String>, default: Option<String>) {
        let name = name.into();
        if let Some(default) = default {
            self.arg_defaults.insert(name, default);
        }
    }

    /// Set an ENV variable
    pub fn set_env(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.env_vars.insert(name.into(), value.into());
    }

    /// Get the effective ARG values (build-time overrides defaults)
    pub fn effective_args(&self) -> HashMap<String, String> {
        let mut result = self.arg_defaults.clone();
        for (k, v) in &self.build_args {
            result.insert(k.clone(), v.clone());
        }
        result
    }

    /// Expand variables in a string using the current context
    pub fn expand(&self, input: &str) -> String {
        expand_variables(input, &self.effective_args(), &self.env_vars)
    }

    /// Expand variables in a list of strings
    pub fn expand_list(&self, inputs: &[String]) -> Vec<String> {
        expand_variables_in_list(inputs, &self.effective_args(), &self.env_vars)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_variable() {
        let mut args = HashMap::new();
        args.insert("VERSION".to_string(), "1.0".to_string());
        let env = HashMap::new();

        assert_eq!(expand_variables("$VERSION", &args, &env), "1.0");
        assert_eq!(expand_variables("${VERSION}", &args, &env), "1.0");
        assert_eq!(expand_variables("v$VERSION", &args, &env), "v1.0");
        assert_eq!(
            expand_variables("v${VERSION}-release", &args, &env),
            "v1.0-release"
        );
    }

    #[test]
    fn test_undefined_variable() {
        let args = HashMap::new();
        let env = HashMap::new();

        assert_eq!(expand_variables("$UNDEFINED", &args, &env), "");
        assert_eq!(expand_variables("${UNDEFINED}", &args, &env), "");
    }

    #[test]
    fn test_default_value_colon_minus() {
        let args = HashMap::new();
        let env = HashMap::new();

        // Unset variable with default
        assert_eq!(expand_variables("${VERSION:-1.0}", &args, &env), "1.0");

        // Set variable ignores default
        let mut args = HashMap::new();
        args.insert("VERSION".to_string(), "2.0".to_string());
        assert_eq!(expand_variables("${VERSION:-1.0}", &args, &env), "2.0");

        // Empty variable uses default (colon variant)
        let mut args = HashMap::new();
        args.insert("VERSION".to_string(), "".to_string());
        assert_eq!(expand_variables("${VERSION:-1.0}", &args, &env), "1.0");
    }

    #[test]
    fn test_default_value_minus() {
        let args = HashMap::new();
        let env = HashMap::new();

        // Unset variable with default
        assert_eq!(expand_variables("${VERSION-1.0}", &args, &env), "1.0");

        // Empty variable keeps empty (non-colon variant)
        let mut args = HashMap::new();
        args.insert("VERSION".to_string(), "".to_string());
        assert_eq!(expand_variables("${VERSION-1.0}", &args, &env), "");
    }

    #[test]
    fn test_alternate_value_colon_plus() {
        let mut args = HashMap::new();
        let env = HashMap::new();

        // Unset variable - no alternate
        assert_eq!(expand_variables("${VERSION:+set}", &args, &env), "");

        // Set non-empty variable - use alternate
        args.insert("VERSION".to_string(), "1.0".to_string());
        assert_eq!(expand_variables("${VERSION:+set}", &args, &env), "set");

        // Set empty variable - no alternate (colon variant)
        args.insert("VERSION".to_string(), "".to_string());
        assert_eq!(expand_variables("${VERSION:+set}", &args, &env), "");
    }

    #[test]
    fn test_alternate_value_plus() {
        let mut args = HashMap::new();
        let env = HashMap::new();

        // Unset variable - no alternate
        assert_eq!(expand_variables("${VERSION+set}", &args, &env), "");

        // Set empty variable - use alternate (non-colon variant)
        args.insert("VERSION".to_string(), "".to_string());
        assert_eq!(expand_variables("${VERSION+set}", &args, &env), "set");
    }

    #[test]
    fn test_env_takes_precedence() {
        let mut args = HashMap::new();
        args.insert("VAR".to_string(), "from_arg".to_string());

        let mut env = HashMap::new();
        env.insert("VAR".to_string(), "from_env".to_string());

        assert_eq!(expand_variables("$VAR", &args, &env), "from_env");
    }

    #[test]
    fn test_escaped_dollar() {
        let args = HashMap::new();
        let env = HashMap::new();

        assert_eq!(expand_variables("\\$VAR", &args, &env), "$VAR");
        assert_eq!(expand_variables("$$", &args, &env), "$");
    }

    #[test]
    fn test_nested_default() {
        let mut args = HashMap::new();
        args.insert("DEFAULT".to_string(), "nested".to_string());
        let env = HashMap::new();

        // Default value contains a variable reference
        assert_eq!(
            expand_variables("${UNSET:-$DEFAULT}", &args, &env),
            "nested"
        );
    }

    #[test]
    fn test_variable_context() {
        let mut ctx = VariableContext::with_build_args({
            let mut m = HashMap::new();
            m.insert("BUILD_TYPE".to_string(), "release".to_string());
            m
        });

        ctx.add_arg("VERSION", Some("1.0".to_string()));
        ctx.set_env("HOME", "/app".to_string());

        assert_eq!(ctx.expand("$BUILD_TYPE"), "release");
        assert_eq!(ctx.expand("$VERSION"), "1.0");
        assert_eq!(ctx.expand("$HOME"), "/app");
    }

    #[test]
    fn test_build_arg_overrides_default() {
        let mut ctx = VariableContext::with_build_args({
            let mut m = HashMap::new();
            m.insert("VERSION".to_string(), "2.0".to_string());
            m
        });

        ctx.add_arg("VERSION", Some("1.0".to_string()));

        // Build arg should override default
        assert_eq!(ctx.expand("$VERSION"), "2.0");
    }

    #[test]
    fn test_complex_string() {
        let mut args = HashMap::new();
        args.insert("APP".to_string(), "myapp".to_string());
        args.insert("VERSION".to_string(), "1.2.3".to_string());
        let env = HashMap::new();

        let input = "FROM registry.example.com/${APP}:${VERSION:-latest}";
        assert_eq!(
            expand_variables(input, &args, &env),
            "FROM registry.example.com/myapp:1.2.3"
        );
    }
}
