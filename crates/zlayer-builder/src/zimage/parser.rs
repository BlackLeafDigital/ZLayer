//! ZImagefile parser — YAML deserialization + semantic validation.
//!
//! The entry point is [`parse_zimagefile`], which takes raw YAML content,
//! deserializes it into a [`ZImage`], and then runs validation rules that
//! cannot be expressed through serde alone.

use super::types::{ZImage, ZStep};
use crate::error::{BuildError, Result};

/// Parse and validate a ZImagefile from its YAML content.
///
/// This performs two phases:
/// 1. **Deserialization** — YAML string into [`ZImage`] via `serde_yaml`.
/// 2. **Validation** — semantic rules that serde annotations cannot enforce.
///
/// # Errors
///
/// Returns [`BuildError::ZImagefileParse`] for malformed YAML and
/// [`BuildError::ZImagefileValidation`] for semantic rule violations.
pub fn parse_zimagefile(content: &str) -> Result<ZImage> {
    // Phase 1: Deserialize YAML into the ZImage struct.
    let image: ZImage =
        serde_yaml::from_str(content).map_err(|e| BuildError::zimagefile_parse(e.to_string()))?;

    // Phase 2: Semantic validation.
    validate_version(&image)?;
    validate_mode_exclusivity(&image)?;
    validate_steps(&image)?;

    Ok(image)
}

// ---------------------------------------------------------------------------
// Version validation
// ---------------------------------------------------------------------------

/// The version field, if present, must be `"1"`.
fn validate_version(image: &ZImage) -> Result<()> {
    if let Some(ref v) = image.version {
        if v != "1" {
            return Err(BuildError::zimagefile_validation(format!(
                "unsupported version '{v}', only version \"1\" is supported"
            )));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Mode exclusivity
// ---------------------------------------------------------------------------

/// Exactly one of `runtime`, `wasm`, `stages`, or `base` must be set.
fn validate_mode_exclusivity(image: &ZImage) -> Result<()> {
    let modes_present: Vec<&str> = [
        image.runtime.as_ref().map(|_| "runtime"),
        image.wasm.as_ref().map(|_| "wasm"),
        image.stages.as_ref().map(|_| "stages"),
        image.base.as_ref().map(|_| "base"),
    ]
    .into_iter()
    .flatten()
    .collect();

    match modes_present.len() {
        0 => Err(BuildError::zimagefile_validation(
            "exactly one of 'runtime', 'wasm', 'stages', or 'base' must be set, but none were found"
                .to_string(),
        )),
        1 => Ok(()),
        _ => Err(BuildError::zimagefile_validation(format!(
            "exactly one of 'runtime', 'wasm', 'stages', or 'base' must be set, \
             but multiple were found: {}",
            modes_present.join(", ")
        ))),
    }
}

// ---------------------------------------------------------------------------
// Step validation
// ---------------------------------------------------------------------------

/// Validate all steps reachable from the current image configuration.
fn validate_steps(image: &ZImage) -> Result<()> {
    // Single-stage mode: validate top-level steps.
    if image.base.is_some() {
        for (i, step) in image.steps.iter().enumerate() {
            validate_step(step, i, None)?;
        }
    }

    // Multi-stage mode: validate steps in every stage.
    if let Some(ref stages) = image.stages {
        for (stage_name, stage) in stages {
            for (i, step) in stage.steps.iter().enumerate() {
                validate_step(step, i, Some(stage_name))?;
            }
        }
    }

    Ok(())
}

/// Validate a single build step.
///
/// Rules enforced:
/// - Exactly one instruction type must be set (`run`, `copy`, `add`, `env`, `workdir`, `user`).
/// - `copy` and `add` steps must have a `to` field.
/// - `cache` is only valid on `run` steps.
/// - `from` is only valid on `copy`/`add` steps.
/// - `owner`/`chmod` are only valid on `copy`/`add` steps.
fn validate_step(step: &ZStep, index: usize, stage: Option<&str>) -> Result<()> {
    let location = match stage {
        Some(s) => format!("stage '{s}', step {}", index + 1),
        None => format!("step {}", index + 1),
    };

    // Count how many instruction fields are set.
    let instructions: Vec<&str> = [
        step.run.as_ref().map(|_| "run"),
        step.copy.as_ref().map(|_| "copy"),
        step.add.as_ref().map(|_| "add"),
        step.env.as_ref().map(|_| "env"),
        step.workdir.as_ref().map(|_| "workdir"),
        step.user.as_ref().map(|_| "user"),
    ]
    .into_iter()
    .flatten()
    .collect();

    match instructions.len() {
        0 => {
            return Err(BuildError::zimagefile_validation(format!(
                "{location}: step must have exactly one instruction type \
                 (run, copy, add, env, workdir, user), but none were found"
            )));
        }
        1 => {} // good
        _ => {
            return Err(BuildError::zimagefile_validation(format!(
                "{location}: step must have exactly one instruction type, \
                 but multiple were found: {}",
                instructions.join(", ")
            )));
        }
    }

    let instruction = instructions[0];
    let is_copy_or_add = instruction == "copy" || instruction == "add";

    // `to` is required on copy/add steps.
    if is_copy_or_add && step.to.is_none() {
        return Err(BuildError::zimagefile_validation(format!(
            "{location}: '{instruction}' step must have a 'to' field"
        )));
    }

    // `cache` is only valid on `run` steps.
    if !step.cache.is_empty() && instruction != "run" {
        return Err(BuildError::zimagefile_validation(format!(
            "{location}: 'cache' is only valid on 'run' steps, not '{instruction}'"
        )));
    }

    // `from` is only valid on `copy`/`add` steps.
    if step.from.is_some() && !is_copy_or_add {
        return Err(BuildError::zimagefile_validation(format!(
            "{location}: 'from' is only valid on 'copy'/'add' steps, not '{instruction}'"
        )));
    }

    // `owner` is only valid on `copy`/`add` steps.
    if step.owner.is_some() && !is_copy_or_add {
        return Err(BuildError::zimagefile_validation(format!(
            "{location}: 'owner' is only valid on 'copy'/'add' steps, not '{instruction}'"
        )));
    }

    // `chmod` is only valid on `copy`/`add` steps.
    if step.chmod.is_some() && !is_copy_or_add {
        return Err(BuildError::zimagefile_validation(format!(
            "{location}: 'chmod' is only valid on 'copy'/'add' steps, not '{instruction}'"
        )));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Happy-path tests ------------------------------------------------

    #[test]
    fn test_parse_runtime_mode() {
        let yaml = r#"
version: "1"
runtime: node22
cmd: "node server.js"
"#;
        let img = parse_zimagefile(yaml).unwrap();
        assert_eq!(img.runtime.as_deref(), Some("node22"));
    }

    #[test]
    fn test_parse_single_stage() {
        let yaml = r#"
version: "1"
base: "alpine:3.19"
steps:
  - run: "apk add --no-cache curl"
  - copy: "app.sh"
    to: "/usr/local/bin/app.sh"
    chmod: "755"
  - workdir: "/app"
cmd: ["./app.sh"]
"#;
        let img = parse_zimagefile(yaml).unwrap();
        assert_eq!(img.base.as_deref(), Some("alpine:3.19"));
        assert_eq!(img.steps.len(), 3);
    }

    #[test]
    fn test_parse_multi_stage() {
        let yaml = r#"
version: "1"
stages:
  builder:
    base: "node:22-alpine"
    steps:
      - copy: "package.json"
        to: "./"
      - run: "npm ci"
  runtime:
    base: "node:22-alpine"
    steps:
      - copy: "dist"
        from: builder
        to: "/app"
cmd: ["node", "dist/index.js"]
"#;
        let img = parse_zimagefile(yaml).unwrap();
        let stages = img.stages.as_ref().unwrap();
        assert_eq!(stages.len(), 2);
    }

    #[test]
    fn test_parse_wasm_mode() {
        let yaml = r#"
version: "1"
wasm:
  target: preview2
  optimize: true
"#;
        let img = parse_zimagefile(yaml).unwrap();
        assert!(img.wasm.is_some());
    }

    #[test]
    fn test_version_omitted_is_ok() {
        let yaml = r#"
runtime: node22
"#;
        let img = parse_zimagefile(yaml).unwrap();
        assert!(img.version.is_none());
        assert_eq!(img.runtime.as_deref(), Some("node22"));
    }

    // -- Version validation -----------------------------------------------

    #[test]
    fn test_bad_version_rejected() {
        let yaml = r#"
version: "2"
runtime: node22
"#;
        let err = parse_zimagefile(yaml).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unsupported version"), "got: {msg}");
    }

    // -- Mode exclusivity -------------------------------------------------

    #[test]
    fn test_no_mode_rejected() {
        let yaml = r#"
version: "1"
cmd: "echo hi"
"#;
        let err = parse_zimagefile(yaml).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("none were found"), "got: {msg}");
    }

    #[test]
    fn test_multiple_modes_rejected() {
        let yaml = r#"
version: "1"
runtime: node22
base: "alpine:3.19"
"#;
        let err = parse_zimagefile(yaml).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("multiple were found"), "got: {msg}");
    }

    // -- Step validation --------------------------------------------------

    #[test]
    fn test_step_no_instruction_rejected() {
        let yaml = r#"
version: "1"
base: "alpine:3.19"
steps:
  - to: "/app"
"#;
        let err = parse_zimagefile(yaml).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("none were found"), "got: {msg}");
    }

    #[test]
    fn test_step_multiple_instructions_rejected() {
        let yaml = r#"
version: "1"
base: "alpine:3.19"
steps:
  - run: "echo hi"
    workdir: "/app"
"#;
        let err = parse_zimagefile(yaml).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("multiple were found"), "got: {msg}");
    }

    #[test]
    fn test_copy_missing_to_rejected() {
        let yaml = r#"
version: "1"
base: "alpine:3.19"
steps:
  - copy: "file.txt"
"#;
        let err = parse_zimagefile(yaml).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("must have a 'to' field"), "got: {msg}");
    }

    #[test]
    fn test_add_missing_to_rejected() {
        let yaml = r#"
version: "1"
base: "alpine:3.19"
steps:
  - add: "archive.tar.gz"
"#;
        let err = parse_zimagefile(yaml).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("must have a 'to' field"), "got: {msg}");
    }

    #[test]
    fn test_cache_on_non_run_rejected() {
        let yaml = r#"
version: "1"
base: "alpine:3.19"
steps:
  - copy: "file.txt"
    to: "/app/file.txt"
    cache:
      - target: /var/cache
"#;
        let err = parse_zimagefile(yaml).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("'cache' is only valid on 'run'"), "got: {msg}");
    }

    #[test]
    fn test_from_on_non_copy_add_rejected() {
        let yaml = r#"
version: "1"
base: "alpine:3.19"
steps:
  - run: "echo hi"
    from: builder
"#;
        let err = parse_zimagefile(yaml).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("'from' is only valid on 'copy'/'add'"),
            "got: {msg}"
        );
    }

    #[test]
    fn test_owner_on_non_copy_add_rejected() {
        let yaml = r#"
version: "1"
base: "alpine:3.19"
steps:
  - run: "echo hi"
    owner: "root:root"
"#;
        let err = parse_zimagefile(yaml).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("'owner' is only valid on 'copy'/'add'"),
            "got: {msg}"
        );
    }

    #[test]
    fn test_chmod_on_non_copy_add_rejected() {
        let yaml = r#"
version: "1"
base: "alpine:3.19"
steps:
  - run: "echo hi"
    chmod: "755"
"#;
        let err = parse_zimagefile(yaml).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("'chmod' is only valid on 'copy'/'add'"),
            "got: {msg}"
        );
    }

    #[test]
    fn test_from_on_copy_allowed() {
        let yaml = r#"
version: "1"
base: "alpine:3.19"
steps:
  - copy: "dist"
    from: builder
    to: "/app"
"#;
        parse_zimagefile(yaml).unwrap();
    }

    #[test]
    fn test_from_on_add_allowed() {
        let yaml = r#"
version: "1"
base: "alpine:3.19"
steps:
  - add: "https://example.com/file.tar.gz"
    from: builder
    to: "/app"
"#;
        parse_zimagefile(yaml).unwrap();
    }

    #[test]
    fn test_cache_on_run_allowed() {
        let yaml = r#"
version: "1"
base: "alpine:3.19"
steps:
  - run: "apt-get update"
    cache:
      - target: /var/cache/apt
        id: apt-cache
"#;
        parse_zimagefile(yaml).unwrap();
    }

    #[test]
    fn test_owner_chmod_on_copy_allowed() {
        let yaml = r#"
version: "1"
base: "alpine:3.19"
steps:
  - copy: "app.sh"
    to: "/usr/local/bin/app.sh"
    owner: "1000:1000"
    chmod: "755"
"#;
        parse_zimagefile(yaml).unwrap();
    }

    #[test]
    fn test_multi_stage_step_validation() {
        // Ensure validation runs on every stage, not just top-level.
        let yaml = r#"
version: "1"
stages:
  builder:
    base: "node:22"
    steps:
      - copy: "package.json"
"#;
        let err = parse_zimagefile(yaml).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("stage 'builder'"), "got: {msg}");
        assert!(msg.contains("must have a 'to' field"), "got: {msg}");
    }

    #[test]
    fn test_yaml_syntax_error() {
        let yaml = ":::not valid yaml:::";
        let err = parse_zimagefile(yaml).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("parse error"), "got: {msg}");
    }

    #[test]
    fn test_env_step_valid() {
        let yaml = r#"
version: "1"
base: "alpine:3.19"
steps:
  - env:
      NODE_ENV: production
"#;
        parse_zimagefile(yaml).unwrap();
    }

    #[test]
    fn test_user_step_valid() {
        let yaml = r#"
version: "1"
base: "alpine:3.19"
steps:
  - user: "nobody"
"#;
        parse_zimagefile(yaml).unwrap();
    }
}
