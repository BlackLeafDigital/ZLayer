//! ZImage-to-Dockerfile converter
//!
//! Converts a parsed [`ZImage`] into the internal [`Dockerfile`] IR so the
//! existing buildah pipeline can execute ZImagefile builds without any
//! changes to the downstream execution logic.

use std::collections::HashMap;

use crate::dockerfile::{
    AddInstruction, ArgInstruction, CopyInstruction, EnvInstruction, ExposeInstruction,
    HealthcheckInstruction, ImageRef, Instruction, RunInstruction, RunMount, ShellOrExec, Stage,
};
use crate::error::{BuildError, Result};

use super::types::{
    ZCacheMount, ZCommand, ZExpose, ZHealthcheck, ZImage, ZPortSpec, ZStage, ZStep,
};

/// The Dockerfile IR type from the parser module.
use crate::dockerfile::Dockerfile;

/// Cache sharing type re-used from the instruction module.
use crate::dockerfile::CacheSharing;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Convert a parsed [`ZImage`] into the internal [`Dockerfile`] IR.
///
/// Supports two modes:
///
/// 1. **Single-stage** -- when `base` is set at the top level.
/// 2. **Multi-stage** -- when `stages` is set (an `IndexMap` of named stages).
///
/// Runtime-only and WASM modes are not convertible to Dockerfile IR; they
/// are handled separately by the builder.
///
/// # Errors
///
/// Returns [`BuildError::ZImagefileValidation`] if the ZImage cannot be
/// meaningfully converted (e.g. no `base` or `stages` present, or a
/// healthcheck duration string cannot be parsed).
pub fn zimage_to_dockerfile(zimage: &ZImage) -> Result<Dockerfile> {
    // Convert global args.
    let global_args = convert_global_args(&zimage.args);

    // Choose single-stage or multi-stage.
    // Note: `build:` directives must be resolved to `base:` by the caller
    // (ImageBuilder) before calling this function.
    let stages = if let Some(ref base) = zimage.base {
        vec![convert_single_stage(zimage, base)?]
    } else if let Some(ref stage_map) = zimage.stages {
        convert_multi_stage(zimage, stage_map)?
    } else if zimage.build.is_some() {
        return Err(BuildError::zimagefile_validation(
            "ZImage has 'build' set but it was not resolved to a 'base' image. \
             This is an internal error — build directives must be resolved before conversion.",
        ));
    } else {
        return Err(BuildError::zimagefile_validation(
            "ZImage must have 'base', 'build', or 'stages' set to convert to a Dockerfile",
        ));
    };

    Ok(Dockerfile {
        global_args,
        stages,
    })
}

// ---------------------------------------------------------------------------
// Global args
// ---------------------------------------------------------------------------

fn convert_global_args(args: &HashMap<String, String>) -> Vec<ArgInstruction> {
    let mut result: Vec<ArgInstruction> = args
        .iter()
        .map(|(name, default)| {
            if default.is_empty() {
                ArgInstruction::new(name)
            } else {
                ArgInstruction::with_default(name, default)
            }
        })
        .collect();
    // Sort for deterministic output.
    result.sort_by(|a, b| a.name.cmp(&b.name));
    result
}

// ---------------------------------------------------------------------------
// Single-stage conversion
// ---------------------------------------------------------------------------

fn convert_single_stage(zimage: &ZImage, base: &str) -> Result<Stage> {
    let base_image = ImageRef::parse(base);
    let mut instructions = Vec::new();

    // env
    if !zimage.env.is_empty() {
        instructions.push(Instruction::Env(EnvInstruction::from_vars(
            zimage.env.clone(),
        )));
    }

    // workdir
    if let Some(ref wd) = zimage.workdir {
        instructions.push(Instruction::Workdir(wd.clone()));
    }

    // steps
    for step in &zimage.steps {
        instructions.push(convert_step(step)?);
    }

    // labels
    if !zimage.labels.is_empty() {
        instructions.push(Instruction::Label(zimage.labels.clone()));
    }

    // expose
    if let Some(ref expose) = zimage.expose {
        instructions.extend(convert_expose(expose)?);
    }

    // user
    if let Some(ref user) = zimage.user {
        instructions.push(Instruction::User(user.clone()));
    }

    // volumes
    if !zimage.volumes.is_empty() {
        instructions.push(Instruction::Volume(zimage.volumes.clone()));
    }

    // healthcheck
    if let Some(ref hc) = zimage.healthcheck {
        instructions.push(convert_healthcheck(hc)?);
    }

    // stopsignal
    if let Some(ref sig) = zimage.stopsignal {
        instructions.push(Instruction::Stopsignal(sig.clone()));
    }

    // entrypoint
    if let Some(ref ep) = zimage.entrypoint {
        instructions.push(Instruction::Entrypoint(convert_command(ep)));
    }

    // cmd
    if let Some(ref cmd) = zimage.cmd {
        instructions.push(Instruction::Cmd(convert_command(cmd)));
    }

    Ok(Stage {
        index: 0,
        name: None,
        base_image,
        platform: zimage.platform.clone(),
        instructions,
    })
}

// ---------------------------------------------------------------------------
// Multi-stage conversion
// ---------------------------------------------------------------------------

fn convert_multi_stage(
    zimage: &ZImage,
    stage_map: &indexmap::IndexMap<String, ZStage>,
) -> Result<Vec<Stage>> {
    let stage_names: Vec<&String> = stage_map.keys().collect();
    let mut stages = Vec::with_capacity(stage_map.len());

    for (idx, (name, zstage)) in stage_map.iter().enumerate() {
        // `build:` directives must be resolved to `base:` before conversion.
        let base_str = zstage.base.as_deref().ok_or_else(|| {
            if zstage.build.is_some() {
                BuildError::zimagefile_validation(format!(
                    "stage '{name}': 'build' directive was not resolved to a 'base' image. \
                     This is an internal error."
                ))
            } else {
                BuildError::zimagefile_validation(format!(
                    "stage '{name}': must have 'base' or 'build' set"
                ))
            }
        })?;

        let base_image = if stage_names.iter().any(|s| s.as_str() == base_str) {
            ImageRef::Stage(base_str.to_string())
        } else {
            ImageRef::parse(base_str)
        };

        let mut instructions = Vec::new();

        // Stage-level args
        for (arg_name, arg_default) in &zstage.args {
            if arg_default.is_empty() {
                instructions.push(Instruction::Arg(ArgInstruction::new(arg_name)));
            } else {
                instructions.push(Instruction::Arg(ArgInstruction::with_default(
                    arg_name,
                    arg_default,
                )));
            }
        }

        // env
        if !zstage.env.is_empty() {
            instructions.push(Instruction::Env(EnvInstruction::from_vars(
                zstage.env.clone(),
            )));
        }

        // workdir
        if let Some(ref wd) = zstage.workdir {
            instructions.push(Instruction::Workdir(wd.clone()));
        }

        // steps
        for step in &zstage.steps {
            instructions.push(convert_step(step)?);
        }

        // labels
        if !zstage.labels.is_empty() {
            instructions.push(Instruction::Label(zstage.labels.clone()));
        }

        // expose
        if let Some(ref expose) = zstage.expose {
            instructions.extend(convert_expose(expose)?);
        }

        // user
        if let Some(ref user) = zstage.user {
            instructions.push(Instruction::User(user.clone()));
        }

        // volumes
        if !zstage.volumes.is_empty() {
            instructions.push(Instruction::Volume(zstage.volumes.clone()));
        }

        // healthcheck
        if let Some(ref hc) = zstage.healthcheck {
            instructions.push(convert_healthcheck(hc)?);
        }

        // stopsignal
        if let Some(ref sig) = zstage.stopsignal {
            instructions.push(Instruction::Stopsignal(sig.clone()));
        }

        // entrypoint
        if let Some(ref ep) = zstage.entrypoint {
            instructions.push(Instruction::Entrypoint(convert_command(ep)));
        }

        // cmd
        if let Some(ref cmd) = zstage.cmd {
            instructions.push(Instruction::Cmd(convert_command(cmd)));
        }

        stages.push(Stage {
            index: idx,
            name: Some(name.clone()),
            base_image,
            platform: zstage.platform.clone(),
            instructions,
        });
    }

    // Append top-level metadata to the *last* stage (the output image).
    if let Some(last) = stages.last_mut() {
        // Top-level env merges into the final stage.
        if !zimage.env.is_empty() {
            last.instructions
                .push(Instruction::Env(EnvInstruction::from_vars(
                    zimage.env.clone(),
                )));
        }

        if let Some(ref wd) = zimage.workdir {
            last.instructions.push(Instruction::Workdir(wd.clone()));
        }

        if !zimage.labels.is_empty() {
            last.instructions
                .push(Instruction::Label(zimage.labels.clone()));
        }

        if let Some(ref expose) = zimage.expose {
            last.instructions.extend(convert_expose(expose)?);
        }

        if let Some(ref user) = zimage.user {
            last.instructions.push(Instruction::User(user.clone()));
        }

        if !zimage.volumes.is_empty() {
            last.instructions
                .push(Instruction::Volume(zimage.volumes.clone()));
        }

        if let Some(ref hc) = zimage.healthcheck {
            last.instructions.push(convert_healthcheck(hc)?);
        }

        if let Some(ref sig) = zimage.stopsignal {
            last.instructions.push(Instruction::Stopsignal(sig.clone()));
        }

        if let Some(ref ep) = zimage.entrypoint {
            last.instructions
                .push(Instruction::Entrypoint(convert_command(ep)));
        }

        if let Some(ref cmd) = zimage.cmd {
            last.instructions
                .push(Instruction::Cmd(convert_command(cmd)));
        }
    }

    Ok(stages)
}

// ---------------------------------------------------------------------------
// Step conversion
// ---------------------------------------------------------------------------

fn convert_step(step: &ZStep) -> Result<Instruction> {
    if let Some(ref cmd) = step.run {
        return Ok(convert_run(cmd, &step.cache, step));
    }

    if let Some(ref sources) = step.copy {
        return Ok(convert_copy(sources, step));
    }

    if let Some(ref sources) = step.add {
        return Ok(convert_add(sources, step));
    }

    if let Some(ref vars) = step.env {
        return Ok(Instruction::Env(EnvInstruction::from_vars(vars.clone())));
    }

    if let Some(ref wd) = step.workdir {
        return Ok(Instruction::Workdir(wd.clone()));
    }

    if let Some(ref user) = step.user {
        return Ok(Instruction::User(user.clone()));
    }

    // Should not be reachable if the parser validated the step, but guard
    // against it anyway.
    Err(BuildError::zimagefile_validation(
        "step has no recognised instruction (run, copy, add, env, workdir, user)",
    ))
}

// ---------------------------------------------------------------------------
// RUN
// ---------------------------------------------------------------------------

fn convert_run(cmd: &ZCommand, caches: &[ZCacheMount], _step: &ZStep) -> Instruction {
    let command = convert_command(cmd);
    let mounts: Vec<RunMount> = caches.iter().map(convert_cache_mount).collect();

    Instruction::Run(RunInstruction {
        command,
        mounts,
        network: None,
        security: None,
    })
}

// ---------------------------------------------------------------------------
// COPY / ADD
// ---------------------------------------------------------------------------

fn convert_copy(sources: &super::types::ZCopySources, step: &ZStep) -> Instruction {
    let destination = step.to.clone().unwrap_or_default();
    Instruction::Copy(CopyInstruction {
        sources: sources.to_vec(),
        destination,
        from: step.from.clone(),
        chown: step.owner.clone(),
        chmod: step.chmod.clone(),
        link: false,
        exclude: Vec::new(),
    })
}

fn convert_add(sources: &super::types::ZCopySources, step: &ZStep) -> Instruction {
    let destination = step.to.clone().unwrap_or_default();
    Instruction::Add(AddInstruction {
        sources: sources.to_vec(),
        destination,
        chown: step.owner.clone(),
        chmod: step.chmod.clone(),
        link: false,
        checksum: None,
        keep_git_dir: false,
    })
}

// ---------------------------------------------------------------------------
// Command helper
// ---------------------------------------------------------------------------

fn convert_command(cmd: &ZCommand) -> ShellOrExec {
    match cmd {
        ZCommand::Shell(s) => ShellOrExec::Shell(s.clone()),
        ZCommand::Exec(v) => ShellOrExec::Exec(v.clone()),
    }
}

// ---------------------------------------------------------------------------
// Expose
// ---------------------------------------------------------------------------

fn convert_expose(expose: &ZExpose) -> Result<Vec<Instruction>> {
    match expose {
        ZExpose::Single(port) => Ok(vec![Instruction::Expose(ExposeInstruction::tcp(*port))]),
        ZExpose::Multiple(specs) => {
            let mut out = Vec::with_capacity(specs.len());
            for spec in specs {
                out.push(convert_port_spec(spec)?);
            }
            Ok(out)
        }
    }
}

fn convert_port_spec(spec: &ZPortSpec) -> Result<Instruction> {
    match spec {
        ZPortSpec::Number(port) => Ok(Instruction::Expose(ExposeInstruction::tcp(*port))),
        ZPortSpec::WithProtocol(s) => {
            let (port_str, proto_str) = s.split_once('/').ok_or_else(|| {
                BuildError::zimagefile_validation(format!(
                    "invalid port spec '{s}', expected format '<port>/<protocol>'"
                ))
            })?;

            let port: u16 = port_str.parse().map_err(|_| {
                BuildError::zimagefile_validation(format!("invalid port number: '{port_str}'"))
            })?;

            let inst = match proto_str.to_lowercase().as_str() {
                "udp" => ExposeInstruction::udp(port),
                _ => ExposeInstruction::tcp(port),
            };

            Ok(Instruction::Expose(inst))
        }
    }
}

// ---------------------------------------------------------------------------
// Healthcheck
// ---------------------------------------------------------------------------

fn convert_healthcheck(hc: &ZHealthcheck) -> Result<Instruction> {
    let command = convert_command(&hc.cmd);

    let interval = parse_optional_duration(&hc.interval, "healthcheck interval")?;
    let timeout = parse_optional_duration(&hc.timeout, "healthcheck timeout")?;
    let start_period = parse_optional_duration(&hc.start_period, "healthcheck start_period")?;

    Ok(Instruction::Healthcheck(HealthcheckInstruction::Check {
        command,
        interval,
        timeout,
        start_period,
        start_interval: None,
        retries: hc.retries,
    }))
}

fn parse_optional_duration(
    value: &Option<String>,
    label: &str,
) -> Result<Option<std::time::Duration>> {
    match value {
        None => Ok(None),
        Some(s) => {
            let dur = humantime::parse_duration(s).map_err(|e| {
                BuildError::zimagefile_validation(format!("invalid {label} '{s}': {e}"))
            })?;
            Ok(Some(dur))
        }
    }
}

// ---------------------------------------------------------------------------
// Cache mount
// ---------------------------------------------------------------------------

pub fn convert_cache_mount(cm: &ZCacheMount) -> RunMount {
    let sharing = match cm.sharing.as_deref() {
        Some("shared") => CacheSharing::Shared,
        Some("private") => CacheSharing::Private,
        Some("locked") | None => CacheSharing::Locked,
        // Unknown value falls back to the default.
        Some(_) => CacheSharing::Locked,
    };

    RunMount::Cache {
        target: cm.target.clone(),
        id: cm.id.clone(),
        sharing,
        readonly: cm.readonly,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zimage::parse_zimagefile;

    // -- Helpers ----------------------------------------------------------

    fn parse_and_convert(yaml: &str) -> Dockerfile {
        let zimage = parse_zimagefile(yaml).expect("YAML parse failed");
        zimage_to_dockerfile(&zimage).expect("conversion failed")
    }

    // -- Single-stage tests -----------------------------------------------

    #[test]
    fn test_single_stage_basic() {
        let df = parse_and_convert(
            r#"
base: "alpine:3.19"
steps:
  - run: "apk add --no-cache curl"
  - copy: "app.sh"
    to: "/usr/local/bin/app.sh"
    chmod: "755"
  - workdir: "/app"
cmd: ["./app.sh"]
"#,
        );

        assert_eq!(df.stages.len(), 1);
        let stage = &df.stages[0];
        assert_eq!(stage.index, 0);
        assert!(stage.name.is_none());

        // base image
        assert!(matches!(
            &stage.base_image,
            ImageRef::Registry { image, tag: Some(t), .. } if image == "alpine" && t == "3.19"
        ));

        // Should contain RUN, COPY, WORKDIR, CMD
        let names: Vec<&str> = stage.instructions.iter().map(|i| i.name()).collect();
        assert!(names.contains(&"RUN"));
        assert!(names.contains(&"COPY"));
        assert!(names.contains(&"WORKDIR"));
        assert!(names.contains(&"CMD"));
    }

    #[test]
    fn test_single_stage_env_and_expose() {
        let df = parse_and_convert(
            r#"
base: "node:22-alpine"
env:
  NODE_ENV: production
expose: 3000
cmd: "node server.js"
"#,
        );

        let stage = &df.stages[0];
        let has_env = stage.instructions.iter().any(|i| matches!(i, Instruction::Env(e) if e.vars.get("NODE_ENV") == Some(&"production".to_string())));
        assert!(has_env);

        let has_expose = stage
            .instructions
            .iter()
            .any(|i| matches!(i, Instruction::Expose(e) if e.port == 3000));
        assert!(has_expose);
    }

    #[test]
    fn test_single_stage_healthcheck() {
        let df = parse_and_convert(
            r#"
base: "alpine:3.19"
healthcheck:
  cmd: "curl -f http://localhost/ || exit 1"
  interval: "30s"
  timeout: "10s"
  start_period: "5s"
  retries: 3
"#,
        );

        let stage = &df.stages[0];
        let hc = stage
            .instructions
            .iter()
            .find(|i| matches!(i, Instruction::Healthcheck(_)));
        assert!(hc.is_some());

        if let Some(Instruction::Healthcheck(HealthcheckInstruction::Check {
            interval,
            timeout,
            start_period,
            retries,
            ..
        })) = hc
        {
            assert_eq!(*interval, Some(std::time::Duration::from_secs(30)));
            assert_eq!(*timeout, Some(std::time::Duration::from_secs(10)));
            assert_eq!(*start_period, Some(std::time::Duration::from_secs(5)));
            assert_eq!(*retries, Some(3));
        } else {
            panic!("Expected Healthcheck::Check");
        }
    }

    #[test]
    fn test_global_args() {
        let df = parse_and_convert(
            r#"
base: "alpine:3.19"
args:
  VERSION: "1.0"
  BUILD_TYPE: ""
"#,
        );

        assert_eq!(df.global_args.len(), 2);

        let version = df.global_args.iter().find(|a| a.name == "VERSION");
        assert!(version.is_some());
        assert_eq!(version.unwrap().default, Some("1.0".to_string()));

        let build_type = df.global_args.iter().find(|a| a.name == "BUILD_TYPE");
        assert!(build_type.is_some());
        assert!(build_type.unwrap().default.is_none());
    }

    // -- Multi-stage tests ------------------------------------------------

    #[test]
    fn test_multi_stage_basic() {
        let df = parse_and_convert(
            r#"
stages:
  builder:
    base: "node:22-alpine"
    workdir: "/src"
    steps:
      - copy: ["package.json", "package-lock.json"]
        to: "./"
      - run: "npm ci"
      - copy: "."
        to: "."
      - run: "npm run build"
  runtime:
    base: "node:22-alpine"
    workdir: "/app"
    steps:
      - copy: "dist"
        from: builder
        to: "/app"
cmd: ["node", "dist/index.js"]
expose: 3000
"#,
        );

        assert_eq!(df.stages.len(), 2);

        let builder = &df.stages[0];
        assert_eq!(builder.name, Some("builder".to_string()));
        assert_eq!(builder.index, 0);

        let runtime = &df.stages[1];
        assert_eq!(runtime.name, Some("runtime".to_string()));
        assert_eq!(runtime.index, 1);

        // The COPY --from=builder should be present.
        let copy_from = runtime
            .instructions
            .iter()
            .find(|i| matches!(i, Instruction::Copy(c) if c.from == Some("builder".to_string())));
        assert!(copy_from.is_some());

        // Top-level CMD and EXPOSE should be on the last stage.
        let has_cmd = runtime
            .instructions
            .iter()
            .any(|i| matches!(i, Instruction::Cmd(_)));
        assert!(has_cmd);

        let has_expose = runtime
            .instructions
            .iter()
            .any(|i| matches!(i, Instruction::Expose(e) if e.port == 3000));
        assert!(has_expose);
    }

    #[test]
    fn test_multi_stage_cross_stage_base() {
        let df = parse_and_convert(
            r#"
stages:
  base:
    base: "alpine:3.19"
    steps:
      - run: "apk add --no-cache curl"
  derived:
    base: "base"
    steps:
      - run: "echo derived"
"#,
        );

        // The 'derived' stage should reference 'base' as ImageRef::Stage.
        let derived = &df.stages[1];
        assert!(matches!(&derived.base_image, ImageRef::Stage(name) if name == "base"));
    }

    // -- Step conversion tests --------------------------------------------

    #[test]
    fn test_step_run_with_cache() {
        let df = parse_and_convert(
            r#"
base: "ubuntu:22.04"
steps:
  - run: "apt-get update && apt-get install -y curl"
    cache:
      - target: /var/cache/apt
        id: apt-cache
        sharing: shared
      - target: /var/lib/apt
        readonly: true
"#,
        );

        let stage = &df.stages[0];
        let run = stage
            .instructions
            .iter()
            .find(|i| matches!(i, Instruction::Run(_)));
        assert!(run.is_some());

        if let Some(Instruction::Run(r)) = run {
            assert_eq!(r.mounts.len(), 2);
            assert!(matches!(
                &r.mounts[0],
                RunMount::Cache { target, id: Some(id), sharing: CacheSharing::Shared, readonly: false }
                    if target == "/var/cache/apt" && id == "apt-cache"
            ));
            assert!(matches!(
                &r.mounts[1],
                RunMount::Cache { target, sharing: CacheSharing::Locked, readonly: true, .. }
                    if target == "/var/lib/apt"
            ));
        }
    }

    #[test]
    fn test_step_copy_with_options() {
        let df = parse_and_convert(
            r#"
base: "alpine:3.19"
steps:
  - copy: "app.sh"
    to: "/usr/local/bin/app.sh"
    owner: "1000:1000"
    chmod: "755"
"#,
        );

        let stage = &df.stages[0];
        if let Some(Instruction::Copy(c)) = stage.instructions.first() {
            assert_eq!(c.sources, vec!["app.sh"]);
            assert_eq!(c.destination, "/usr/local/bin/app.sh");
            assert_eq!(c.chown, Some("1000:1000".to_string()));
            assert_eq!(c.chmod, Some("755".to_string()));
        } else {
            panic!("Expected COPY instruction");
        }
    }

    #[test]
    fn test_step_add() {
        let df = parse_and_convert(
            r#"
base: "alpine:3.19"
steps:
  - add: "https://example.com/file.tar.gz"
    to: "/app/"
"#,
        );

        let stage = &df.stages[0];
        if let Some(Instruction::Add(a)) = stage.instructions.first() {
            assert_eq!(a.sources, vec!["https://example.com/file.tar.gz"]);
            assert_eq!(a.destination, "/app/");
        } else {
            panic!("Expected ADD instruction");
        }
    }

    #[test]
    fn test_step_env() {
        let df = parse_and_convert(
            r#"
base: "alpine:3.19"
steps:
  - env:
      FOO: bar
      BAZ: qux
"#,
        );

        let stage = &df.stages[0];
        if let Some(Instruction::Env(e)) = stage.instructions.first() {
            assert_eq!(e.vars.get("FOO"), Some(&"bar".to_string()));
            assert_eq!(e.vars.get("BAZ"), Some(&"qux".to_string()));
        } else {
            panic!("Expected ENV instruction");
        }
    }

    #[test]
    fn test_expose_multiple_with_protocol() {
        let df = parse_and_convert(
            r#"
base: "alpine:3.19"
expose:
  - 8080
  - "9090/udp"
"#,
        );

        let stage = &df.stages[0];
        let exposes: Vec<&ExposeInstruction> = stage
            .instructions
            .iter()
            .filter_map(|i| match i {
                Instruction::Expose(e) => Some(e),
                _ => None,
            })
            .collect();

        assert_eq!(exposes.len(), 2);
        assert_eq!(exposes[0].port, 8080);
        assert!(matches!(
            exposes[0].protocol,
            crate::dockerfile::ExposeProtocol::Tcp
        ));
        assert_eq!(exposes[1].port, 9090);
        assert!(matches!(
            exposes[1].protocol,
            crate::dockerfile::ExposeProtocol::Udp
        ));
    }

    #[test]
    fn test_volumes_and_stopsignal() {
        let df = parse_and_convert(
            r#"
base: "alpine:3.19"
volumes:
  - /data
  - /logs
stopsignal: SIGTERM
"#,
        );

        let stage = &df.stages[0];
        let has_volume = stage.instructions.iter().any(|i| {
            matches!(i, Instruction::Volume(v) if v.len() == 2 && v.contains(&"/data".to_string()))
        });
        assert!(has_volume);

        let has_signal = stage
            .instructions
            .iter()
            .any(|i| matches!(i, Instruction::Stopsignal(s) if s == "SIGTERM"));
        assert!(has_signal);
    }

    #[test]
    fn test_entrypoint_and_cmd() {
        let df = parse_and_convert(
            r#"
base: "alpine:3.19"
entrypoint: ["/docker-entrypoint.sh"]
cmd: ["node", "server.js"]
"#,
        );

        let stage = &df.stages[0];
        let has_ep = stage.instructions.iter().any(|i| {
            matches!(i, Instruction::Entrypoint(ShellOrExec::Exec(v)) if v == &["/docker-entrypoint.sh"])
        });
        assert!(has_ep);

        let has_cmd = stage.instructions.iter().any(
            |i| matches!(i, Instruction::Cmd(ShellOrExec::Exec(v)) if v == &["node", "server.js"]),
        );
        assert!(has_cmd);
    }

    #[test]
    fn test_user_instruction() {
        let df = parse_and_convert(
            r#"
base: "alpine:3.19"
user: "nobody"
"#,
        );

        let stage = &df.stages[0];
        let has_user = stage
            .instructions
            .iter()
            .any(|i| matches!(i, Instruction::User(u) if u == "nobody"));
        assert!(has_user);
    }

    #[test]
    fn test_scratch_base() {
        let df = parse_and_convert(
            r#"
base: scratch
cmd: ["/app"]
"#,
        );

        assert!(matches!(&df.stages[0].base_image, ImageRef::Scratch));
    }

    #[test]
    fn test_runtime_mode_not_convertible() {
        let yaml = r#"
runtime: node22
cmd: "node server.js"
"#;
        let zimage = parse_zimagefile(yaml).unwrap();
        let result = zimage_to_dockerfile(&zimage);
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_healthcheck_duration() {
        let yaml = r#"
base: "alpine:3.19"
healthcheck:
  cmd: "true"
  interval: "not_a_duration"
"#;
        let zimage = parse_zimagefile(yaml).unwrap();
        let result = zimage_to_dockerfile(&zimage);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("interval"), "got: {msg}");
    }
}
