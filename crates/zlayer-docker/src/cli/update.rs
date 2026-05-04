//! Docker `update` command.
//!
//! Implements `docker update [OPTIONS] CONTAINER [CONTAINER...]` by
//! forwarding each container ID/name to the local zlayer daemon via
//! [`DaemonClient::update_container`]. The CLI mirrors Docker's flag set
//! for the resource knobs zlayer supports (cpu, memory, pids, blkio,
//! restart-policy); Windows-only flags (`--cpu-count`, `--cpu-percent`)
//! are not exposed.

use anyhow::Context;
use clap::Parser;
use zlayer_client::{default_socket_path, DaemonClient};
use zlayer_types::api::containers::{ContainerUpdateRequest, ContainerUpdateRestartPolicy};

/// Arguments for `docker update`.
///
/// Docker's `update` command takes a series of named flags and one or
/// more container ID/name positional arguments. Each flag corresponds to
/// a single field on [`ContainerUpdateRequest`]; flags that are not
/// passed are forwarded as `None` and the daemon leaves the
/// corresponding limit untouched.
#[derive(Debug, Parser)]
pub struct UpdateArgs {
    /// CPU shares (relative weight). Maps to Docker's `--cpu-shares`.
    #[clap(long = "cpu-shares")]
    pub cpu_shares: Option<i64>,

    /// Memory limit. Accepts a positive integer of bytes; suffixes like
    /// `m` / `g` are not parsed locally — pass the raw byte count.
    #[clap(long = "memory", short = 'm')]
    pub memory: Option<i64>,

    /// Total memory + swap limit in bytes. `-1` means unlimited swap.
    #[clap(long = "memory-swap")]
    pub memory_swap: Option<i64>,

    /// Kernel memory limit in bytes (deprecated upstream).
    #[clap(long = "kernel-memory")]
    pub kernel_memory: Option<i64>,

    /// CPU CFS period in microseconds (`--cpu-period`).
    #[clap(long = "cpu-period")]
    pub cpu_period: Option<i64>,

    /// CPU CFS quota in microseconds (`--cpu-quota`).
    #[clap(long = "cpu-quota")]
    pub cpu_quota: Option<i64>,

    /// PIDs limit. `0` or `-1` means unlimited.
    #[clap(long = "pids-limit")]
    pub pids_limit: Option<i64>,

    /// Block IO weight (relative weight, 10-1000).
    #[clap(long = "blkio-weight")]
    pub blkio_weight: Option<u16>,

    /// Restart policy. One of `no`, `always`, `unless-stopped`, or
    /// `on-failure[:max-retries]`. Mirrors `docker run --restart`'s
    /// syntax: a colon-separated max-retry count is accepted with
    /// `on-failure`.
    #[clap(long = "restart")]
    pub restart: Option<String>,

    /// Container name(s) or ID(s) to update.
    #[clap(required = true)]
    pub containers: Vec<String>,
}

impl UpdateArgs {
    /// Build the [`ContainerUpdateRequest`] body from this argument set.
    /// Returns `None` if no fields were supplied (so the caller can
    /// reject the invocation with a useful error rather than sending an
    /// empty update to the daemon).
    fn to_request(&self) -> anyhow::Result<ContainerUpdateRequest> {
        let restart_policy = match self.restart.as_deref() {
            None => None,
            Some(spec) => {
                let (name, max_retries) = match spec.split_once(':') {
                    Some((n, count)) => {
                        let parsed = count
                            .parse::<i64>()
                            .with_context(|| format!("invalid retry count in --restart {spec}"))?;
                        (n.to_string(), Some(parsed))
                    }
                    None => (spec.to_string(), None),
                };
                match name.as_str() {
                    "" | "no" | "always" | "unless-stopped" | "on-failure" => {}
                    other => anyhow::bail!(
                        "invalid --restart value '{other}'; expected no, always, unless-stopped, or on-failure"
                    ),
                }
                Some(ContainerUpdateRestartPolicy {
                    name: Some(name),
                    maximum_retry_count: max_retries,
                })
            }
        };

        Ok(ContainerUpdateRequest {
            cpu_shares: self.cpu_shares,
            memory: self.memory,
            cpu_period: self.cpu_period,
            cpu_quota: self.cpu_quota,
            cpu_realtime_period: None,
            cpu_realtime_runtime: None,
            cpuset_cpus: None,
            cpuset_mems: None,
            memory_reservation: None,
            memory_swap: self.memory_swap,
            kernel_memory: self.kernel_memory,
            blkio_weight: self.blkio_weight,
            pids_limit: self.pids_limit,
            restart_policy,
        })
    }

    /// Returns `true` when no resource flags were supplied — the user
    /// passed only container names.
    fn no_fields_set(&self) -> bool {
        self.cpu_shares.is_none()
            && self.memory.is_none()
            && self.memory_swap.is_none()
            && self.kernel_memory.is_none()
            && self.cpu_period.is_none()
            && self.cpu_quota.is_none()
            && self.pids_limit.is_none()
            && self.blkio_weight.is_none()
            && self.restart.is_none()
    }
}

/// Handle the `docker update` command.
///
/// # Errors
///
/// Returns an error if:
///
/// * No resource flags were supplied (Docker treats this as a CLI
///   usage error).
/// * The daemon connection fails.
/// * The daemon rejects the update for any container.
pub async fn handle_update(args: UpdateArgs) -> anyhow::Result<()> {
    tracing::info!("docker update: containers={:?}", args.containers);

    if args.no_fields_set() {
        anyhow::bail!(
            "you must specify at least one of --cpu-shares, --memory, --memory-swap, \
             --kernel-memory, --cpu-period, --cpu-quota, --pids-limit, --blkio-weight, \
             or --restart"
        );
    }

    let body = args.to_request()?;

    let client = DaemonClient::connect_to(default_socket_path())
        .await
        .context("Failed to connect to zlayer daemon")?;

    let mut first_err: Option<anyhow::Error> = None;
    for id in &args.containers {
        match client.update_container(id, &body).await {
            Ok(resp) => {
                println!("{id}");
                for w in &resp.warnings {
                    eprintln!("WARNING: {w}");
                }
            }
            Err(e) => {
                eprintln!("Error response from daemon: {e}");
                if first_err.is_none() {
                    first_err = Some(e.context(format!("Failed to update container '{id}'")));
                }
            }
        }
    }
    if let Some(e) = first_err {
        return Err(e);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cpu_and_memory_flags() {
        let args = UpdateArgs::try_parse_from([
            "update",
            "--cpu-shares",
            "512",
            "--memory",
            "536870912",
            "my-container",
        ])
        .expect("parse");
        assert_eq!(args.cpu_shares, Some(512));
        assert_eq!(args.memory, Some(536_870_912));
        assert_eq!(args.containers, vec!["my-container"]);
    }

    #[test]
    fn parses_pids_blkio_and_cpu_quota() {
        let args = UpdateArgs::try_parse_from([
            "update",
            "--cpu-period",
            "100000",
            "--cpu-quota",
            "50000",
            "--pids-limit",
            "2048",
            "--blkio-weight",
            "500",
            "c1",
        ])
        .expect("parse");
        assert_eq!(args.cpu_period, Some(100_000));
        assert_eq!(args.cpu_quota, Some(50_000));
        assert_eq!(args.pids_limit, Some(2048));
        assert_eq!(args.blkio_weight, Some(500));
    }

    #[test]
    fn parses_restart_with_retry_count() {
        let args = UpdateArgs::try_parse_from(["update", "--restart", "on-failure:5", "c1"])
            .expect("parse");
        let req = args.to_request().expect("build request");
        let rp = req.restart_policy.expect("restart_policy");
        assert_eq!(rp.name.as_deref(), Some("on-failure"));
        assert_eq!(rp.maximum_retry_count, Some(5));
    }

    #[test]
    fn parses_restart_without_retry_count() {
        let args = UpdateArgs::try_parse_from(["update", "--restart", "unless-stopped", "c1"])
            .expect("parse");
        let req = args.to_request().expect("build request");
        let rp = req.restart_policy.expect("restart_policy");
        assert_eq!(rp.name.as_deref(), Some("unless-stopped"));
        assert_eq!(rp.maximum_retry_count, None);
    }

    #[test]
    fn rejects_unknown_restart_name() {
        let args =
            UpdateArgs::try_parse_from(["update", "--restart", "garbage", "c1"]).expect("parse");
        let err = args
            .to_request()
            .expect_err("invalid restart name must error");
        assert!(format!("{err}").contains("invalid --restart value"));
    }

    #[test]
    fn parses_short_memory_flag() {
        // Docker's `-m` short flag for `--memory`.
        let args = UpdateArgs::try_parse_from(["update", "-m", "1073741824", "c1"]).expect("parse");
        assert_eq!(args.memory, Some(1_073_741_824));
    }

    #[test]
    fn no_fields_set_returns_true_when_only_container_supplied() {
        let args = UpdateArgs::try_parse_from(["update", "c1"]).expect("parse");
        assert!(args.no_fields_set());
    }

    #[test]
    fn rejects_missing_container_argument() {
        // Containers field is `required = true` so clap rejects an
        // invocation without any positional arguments.
        let result = UpdateArgs::try_parse_from(["update", "--cpu-shares", "100"]);
        assert!(result.is_err());
    }

    #[test]
    fn to_request_threads_cpu_shares_through() {
        let args =
            UpdateArgs::try_parse_from(["update", "--cpu-shares", "256", "c1"]).expect("parse");
        let req = args.to_request().expect("build request");
        assert_eq!(req.cpu_shares, Some(256));
        assert_eq!(req.memory, None);
    }
}
