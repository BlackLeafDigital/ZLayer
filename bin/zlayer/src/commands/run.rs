//! `zlayer run` — run a container (docker-run semantics) with optional
//! `ZLayer` secret-environment injection, plus `zlayer env run` — locally
//! spawn a command with secrets injected as env vars.
//!
//! [`handle_container_run`] backs the top-level `zlayer run`: it translates
//! the docker-style flags into a single-service [`DeploymentSpec`] (reusing
//! `zlayer_docker::cli::run::build_deployment_spec`), resolves and overlays
//! any selected secret-environment, submits the spec to the daemon, and
//! attaches/streams as appropriate.
//!
//! [`handle_env_run`] backs `zlayer env run`: it merges secrets from the
//! primary env with any extra merge envs (including an implicit `global`
//! unless --no-global), inherits the parent process env, then execs the
//! target command via `tokio::process`. Exit code propagates.

use anyhow::{anyhow, bail, Context, Result};
use std::collections::HashMap;
use tokio::process::Command;
use tracing::{debug, warn};

#[cfg(feature = "docker-compat")]
use zlayer_client::default_socket_path;
use zlayer_client::DaemonClient;

use crate::commands::secret::resolve_env_id;

/// Run a container (docker-run semantics), injecting an optional `ZLayer`
/// secret-environment.
///
/// Builds the deployment spec from the docker-style flags, then — when a
/// secret-environment is selected — overlays the resolved secrets underneath
/// the individual `-e KEY=VAL` vars (which always win). Submits to the daemon
/// and, unless `--detach`, follows the container to completion (raw-mode TTY
/// attach for `-it`, plain log streaming otherwise), propagating its exit
/// code.
///
/// # Errors
///
/// Returns errors for: malformed docker flags, daemon connection failures,
/// unknown/inaccessible secret-envs, spec rejection by the daemon, or a
/// non-zero container exit status.
#[cfg(feature = "docker-compat")]
#[allow(clippy::too_many_lines)]
pub(crate) async fn handle_container_run(args: &crate::cli::RunArgs) -> Result<()> {
    use zlayer_docker::cli::run::{attach_interactive, build_deployment_spec, stream_until_exit};

    // ------------------------------------------------------------------
    // Translate the bin's container-run args into the docker-shim RunArgs
    // so we can reuse its spec builder verbatim. Unsupported docker flags
    // stay at their defaults.
    // ------------------------------------------------------------------
    let docker_args = zlayer_docker::cli::RunArgs {
        lifecycle: zlayer_docker::cli::RunLifecycleArgs {
            detach: args.detach,
            attach: Vec::new(),
            interactive: args.interactive,
            tty: args.tty,
            rm: args.rm,
            sig_proxy: true,
            cidfile: None,
            disable_content_trust: false,
        },
        networking: zlayer_docker::cli::RunNetworkArgs {
            ports: args.ports.clone(),
            publish_all: false,
            network: args.network.clone(),
            network_alias: Vec::new(),
            link: Vec::new(),
            link_local_ip: Vec::new(),
            ip: None,
            ip6: None,
            mac_address: None,
            expose: Vec::new(),
            domainname: None,
            dns: Vec::new(),
            dns_option: Vec::new(),
            dns_search: Vec::new(),
            add_host: Vec::new(),
        },
        volumes_group: zlayer_docker::cli::RunVolumesArgs {
            volumes: args.volumes.clone(),
            volumes_from: Vec::new(),
            volume_driver: None,
            mount: Vec::new(),
            tmpfs: Vec::new(),
            read_only: false,
            storage_opt: Vec::new(),
            shm_size: None,
        },
        runtime: zlayer_docker::cli::RunRuntimeArgs {
            // Individual container env vars (`-e KEY=VAL`). These are the
            // docker-style vars; they override any injected secret of the
            // same name (handled below).
            env: args.env_vars.clone(),
            env_file: Vec::new(),
            name: args.name.clone(),
            restart: None,
            entrypoint: args.entrypoint.clone(),
            workdir: args.workdir.clone(),
            labels: Vec::new(),
            label_file: Vec::new(),
            init: false,
            platform: None,
            pull: "missing".to_string(),
            no_healthcheck: false,
            health_cmd: None,
            health_interval: None,
            health_retries: None,
            health_start_period: None,
            health_start_interval: None,
            health_timeout: None,
        },
        resources: zlayer_docker::cli::RunResourcesArgs {
            memory: args.memory.clone(),
            memory_reservation: None,
            memory_swap: None,
            memory_swappiness: None,
            kernel_memory: None,
            oom_score_adj: None,
            oom_kill_disable: false,
            pids_limit: None,
            cpus: args.cpus.clone(),
            cpu_period: None,
            cpu_quota: None,
            cpu_rt_period: None,
            cpu_rt_runtime: None,
            cpu_shares: None,
            cpuset_cpus: None,
            cpuset_mems: None,
            blkio_weight: None,
            blkio_weight_device: Vec::new(),
            device_read_bps: Vec::new(),
            device_read_iops: Vec::new(),
            device_write_bps: Vec::new(),
            device_write_iops: Vec::new(),
            gpus: None,
            device: Vec::new(),
            device_cgroup_rule: Vec::new(),
        },
        security: zlayer_docker::cli::RunSecurityArgs {
            cap_add: Vec::new(),
            cap_drop: Vec::new(),
            privileged: false,
            security_opt: Vec::new(),
            user: args.user.clone(),
            group_add: Vec::new(),
            userns: None,
            uts: None,
            pid: None,
            ipc: None,
            cgroupns: None,
            cgroup_parent: None,
            isolation: None,
            runtime: None,
        },
        misc: zlayer_docker::cli::RunMiscArgs {
            hostname: None,
            sysctl: Vec::new(),
            ulimit: Vec::new(),
            stop_signal: None,
            stop_timeout: None,
        },
        image: args.image.clone(),
        command: args.command.clone(),
    };

    let want_attach = docker_args.lifecycle.tty && docker_args.lifecycle.interactive;
    let detach = docker_args.lifecycle.detach;
    let rm = docker_args.lifecycle.rm;

    let client = DaemonClient::connect_to(default_socket_path())
        .await
        .context("Failed to connect to zlayer daemon")?;

    // Build the deployment spec from the docker flags (identical to
    // `zlayer docker run`). The `-e` vars already populated `service.env`.
    let mut spec = build_deployment_spec(&docker_args)?;

    // ------------------------------------------------------------------
    // Resolve + overlay the selected secret-environment, if any.
    //
    // Precedence (low → high): global (unless --no-global) < each --merge
    // < primary --env, then the docker `-e KEY=VAL` vars win over all of
    // them. So: start from the secrets map, then `.extend()` with the
    // service's existing (docker `-e`) env.
    // ------------------------------------------------------------------
    if let Some(primary) = args.env.as_deref() {
        let project = args.project.as_deref();
        let mut secrets: HashMap<String, String> = HashMap::new();

        if !args.no_global {
            match resolve_env_id(&client, "global", project).await {
                Ok(global_id) => {
                    let s = client
                        .reveal_all_secrets_in_env(&global_id)
                        .await
                        .context("Failed to fetch secrets from the `global` env")?;
                    debug!(count = s.len(), "merged global env secrets");
                    secrets.extend(s);
                }
                Err(e) => {
                    debug!(error = %e, "`global` env not found or inaccessible — skipping");
                }
            }
        }

        for extra in &args.merge {
            let extra_id = resolve_env_id(&client, extra, project)
                .await
                .with_context(|| format!("Failed to resolve --merge env '{extra}'"))?;
            let s = client
                .reveal_all_secrets_in_env(&extra_id)
                .await
                .with_context(|| format!("Failed to fetch secrets from merge env '{extra}'"))?;
            debug!(env = %extra, count = s.len(), "merged extra env secrets");
            secrets.extend(s);
        }

        let primary_id = resolve_env_id(&client, primary, project)
            .await
            .with_context(|| format!("Failed to resolve --env '{primary}'"))?;
        let primary_secrets = client
            .reveal_all_secrets_in_env(&primary_id)
            .await
            .with_context(|| format!("Failed to fetch secrets from env '{primary}'"))?;
        debug!(env = %primary, count = primary_secrets.len(), "merged primary env secrets");
        secrets.extend(primary_secrets);

        // Overlay: docker `-e` vars (already in service.env) win over secrets.
        if let Some(service) = spec.services.values_mut().next() {
            let mut merged = secrets;
            merged.extend(service.env.clone());
            service.env = merged;
        }
    }

    // `--rm` => auto-remove on exit. `build_deployment_spec` only warns on
    // `--rm` (no native analogue in the docker shim path), so set the
    // lifecycle flag explicitly here.
    if rm {
        if let Some(service) = spec.services.values_mut().next() {
            service.lifecycle.delete_on_exit = true;
        }
    }

    let deployment_name = spec.deployment.clone();
    let service_name = spec
        .services
        .keys()
        .next()
        .cloned()
        .unwrap_or_else(|| deployment_name.clone());

    let spec_yaml =
        serde_yaml::to_string(&spec).context("Failed to serialize DeploymentSpec to YAML")?;

    let resp = client
        .create_deployment(&spec_yaml)
        .await
        .context("Failed to create deployment via daemon")?;

    // Echo the id/name unless we're about to take over the terminal.
    if !want_attach || detach {
        if let Some(id) = resp.get("id").and_then(|v| v.as_str()) {
            println!("{id}");
        } else if let Some(name) = resp.get("name").and_then(|v| v.as_str()) {
            println!("{name}");
        } else {
            println!("{deployment_name}");
        }
    }

    if detach {
        return Ok(());
    }

    if want_attach {
        // `-it`: raw-mode TTY attach. Stdin forwarding into the container is
        // a later pass — see `attach_interactive` in zlayer-docker.
        attach_interactive(&client, &deployment_name, &service_name).await?;
    } else {
        // Foreground non-TTY: stream logs to completion, propagate exit code.
        stream_until_exit(&client, &deployment_name, &service_name).await?;
    }

    Ok(())
}

/// Run a local command with secrets injected as env vars (no container).
///
/// Backs `zlayer env run`. Merges secrets from the primary env with any
/// extra merge envs (plus an implicit `global` unless --no-global), inherits
/// the parent process env, then execs the target command. Exit code
/// propagates.
///
/// # Errors
///
/// Returns errors for: daemon connection failures, unknown env ids,
/// non-existent merge envs (other than `global` which is treated as optional),
/// child-process spawn failures, or --unmask without admin.
#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
pub async fn handle_env_run(
    env: &str,
    no_global: bool,
    merge: &[String],
    project: Option<&str>,
    dry_run: bool,
    unmask: bool,
    command: &[String],
) -> Result<()> {
    if command.is_empty() {
        bail!("No command provided (expected after `--`)");
    }

    let client = DaemonClient::connect().await?;

    // 1. Resolve every participating env id in the order we'll merge them.
    //    `global` goes first (lowest priority), then --merge flags,
    //    then the primary --env (highest priority).
    let mut merged: HashMap<String, String> = HashMap::new();

    if !no_global {
        match resolve_env_id(&client, "global", project).await {
            Ok(global_id) => {
                let secrets = client
                    .reveal_all_secrets_in_env(&global_id)
                    .await
                    .context("Failed to fetch secrets from the `global` env")?;
                debug!(count = secrets.len(), "merged global env secrets");
                merged.extend(secrets);
            }
            Err(e) => {
                // Silent if global just doesn't exist; loud if something else
                // went wrong.
                debug!(error = %e, "`global` env not found or inaccessible — skipping");
            }
        }
    }

    for extra in merge {
        let extra_id = resolve_env_id(&client, extra, project)
            .await
            .with_context(|| format!("Failed to resolve --merge env '{extra}'"))?;
        let secrets = client
            .reveal_all_secrets_in_env(&extra_id)
            .await
            .with_context(|| format!("Failed to fetch secrets from merge env '{extra}'"))?;
        debug!(env = %extra, count = secrets.len(), "merged extra env secrets");
        merged.extend(secrets);
    }

    let primary_id = resolve_env_id(&client, env, project)
        .await
        .with_context(|| format!("Failed to resolve --env '{env}'"))?;
    let primary_secrets = client
        .reveal_all_secrets_in_env(&primary_id)
        .await
        .with_context(|| format!("Failed to fetch secrets from env '{env}'"))?;
    debug!(env = %env, count = primary_secrets.len(), "merged primary env secrets");
    merged.extend(primary_secrets);

    if dry_run {
        let mut keys: Vec<&String> = merged.keys().collect();
        keys.sort();
        for k in keys {
            if unmask {
                let v = merged.get(k).map_or("", String::as_str);
                println!("{k}={v}");
            } else {
                println!("{k}=***");
            }
        }
        return Ok(());
    }

    // Spawn: child inherits parent env, then our merged map overlays on top.
    let program = &command[0];
    let args = &command[1..];

    let mut cmd = Command::new(program);
    cmd.args(args);
    for (k, v) in &merged {
        cmd.env(k, v);
    }

    let mut child = cmd
        .spawn()
        .with_context(|| format!("Failed to spawn `{program}`. Is it in PATH?"))?;
    let status = child
        .wait()
        .await
        .map_err(|e| anyhow!("Failed while waiting for child process: {e}"))?;

    let code = status.code().unwrap_or_else(|| {
        warn!("Child process killed by signal; exiting with 1");
        1
    });
    std::process::exit(code);
}
