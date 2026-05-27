//! `zlayer runtime` — internal runc-compatible runtime CLI. Drives the
//! in-process libcontainer backend ([`zlayer_agent::runtimes::youki`]) so the
//! same patched libcontainer that runs containers on native Linux also runs
//! them inside the `zlayer` WSL2 distro on Windows hosts.
//!
//! Hidden from top-level `--help`: this is not a user-facing surface. End
//! users interact with containers via `zlayer container` / `zlayer image`,
//! which talk to the daemon over UDS; this CLI is invoked per-verb by
//! `Wsl2DelegateRuntime` inside the WSL2 distro.
//!
//! Each verb mirrors the libcontainer usage pattern from
//! `crates/zlayer-agent/src/runtimes/youki.rs`:
//! - `ContainerBuilder::new(id, SyscallType::Linux).with_root_path(state_root).as_init(&bundle).build()` for create
//! - `Container::load(<state_root>/<id>)` followed by `.start()` / `.kill(sig, all)` / `.delete(force)` for the lifecycle verbs
//! - `as_tenant().with_container_args(...).build()` for exec
//! - `container.events(interval, stats)` (already implemented in libcontainer) for stats
//!
//! See `crates/zlayer-agent/src/runtimes/wsl2_delegate.rs` for the caller:
//! `Wsl2DelegateRuntime` shells out to `wsl.exe -d zlayer -- zlayer runtime <verb>`.

#![cfg(all(target_os = "linux", feature = "youki-runtime"))]

use std::convert::TryFrom;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use libcontainer::container::builder::ContainerBuilder;
use libcontainer::container::{Container, ContainerStatus};
use libcontainer::signal::Signal;
use libcontainer::syscall::syscall::SyscallType;

use crate::cli::{RuntimeCommand, RuntimeGlobal};

/// Default poll interval for streaming `events` (seconds). Matches runc.
const EVENTS_INTERVAL_SECS: u32 = 5;

/// Dispatch the `zlayer runtime <verb>` tree.
///
/// # Errors
///
/// Returns an error if the underlying libcontainer operation fails, the
/// container ID is unknown, or the supplied bundle/signal is invalid.
pub async fn handle(global: &RuntimeGlobal, cmd: &RuntimeCommand) -> Result<()> {
    match cmd {
        RuntimeCommand::State { id } => state(global, id).await,
        RuntimeCommand::Create { id, bundle, log } => {
            create(global, id, bundle, log.as_deref()).await
        }
        RuntimeCommand::Start { id } => start(global, id).await,
        RuntimeCommand::Kill { id, signal, all } => kill(global, id, signal, *all).await,
        RuntimeCommand::Delete { id, force } => delete(global, id, *force).await,
        RuntimeCommand::Exec { id, args } => exec(global, id, args).await,
        RuntimeCommand::Events { id, stats } => events(global, id, *stats).await,
    }
}

/// Compute the libcontainer per-container state directory:
/// `<state_root>/<id>`.
fn container_root(global: &RuntimeGlobal, id: &str) -> PathBuf {
    global.state_root.join(id)
}

/// Map libcontainer's [`ContainerStatus`] enum to runc's lowercase status
/// string used in the JSON state output (`creating`, `created`, `running`,
/// `stopped`, `paused`).
fn format_status(s: ContainerStatus) -> &'static str {
    match s {
        ContainerStatus::Creating => "creating",
        ContainerStatus::Created => "created",
        ContainerStatus::Running => "running",
        ContainerStatus::Stopped => "stopped",
        ContainerStatus::Paused => "paused",
    }
}

/// Parse a runc-style signal string (`SIGTERM`, `TERM`, or `"15"`) into the
/// libcontainer [`Signal`] wrapper. Delegates to libcontainer's own
/// `TryFrom<&str>` implementation so we accept exactly the same set of names
/// that the agent runtime does.
fn parse_signal(name: &str) -> Result<Signal> {
    Signal::try_from(name).map_err(|e| anyhow::anyhow!("invalid signal {name:?}: {e:?}"))
}

/// `zlayer runtime state <id>` — emits a runc-compatible JSON state record on
/// stdout: `{ ociVersion, id, status, pid, bundle, annotations }`.
async fn state(global: &RuntimeGlobal, id: &str) -> Result<()> {
    let root = container_root(global, id);
    let id_owned = id.to_string();

    if !root.exists() {
        anyhow::bail!(
            "container state not found: {} (state_root={})",
            id_owned,
            global.state_root.display()
        );
    }

    let (status, pid, bundle) = tokio::task::spawn_blocking(move || -> Result<_> {
        let mut container = Container::load(root)
            .with_context(|| format!("Container::load failed for {id_owned}"))?;
        // Refresh status so the reported state matches the live process; mirrors
        // `runtimes/youki.rs:1427`.
        let _ = container.refresh_status();
        let status = container.status();
        // libcontainer ships its own pinned `nix` (0.29) which exposes a
        // distinct `Pid` type from this crate's `nix` dep. Call `.as_raw()`
        // through a closure so the method resolves against libcontainer's
        // version, matching the trick used by `runtimes/youki.rs:1601-1603`.
        // Clippy wants `Pid::as_raw` directly but that picks up the wrong
        // nix version and breaks the build.
        #[allow(clippy::redundant_closure_for_method_calls)]
        let pid = container.pid().map_or(0, |p| p.as_raw());
        let bundle = container.bundle().to_string_lossy().into_owned();
        Ok((status, pid, bundle))
    })
    .await
    .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))??;

    let out = serde_json::json!({
        "ociVersion": "1.0.2",
        "id": id,
        "status": format_status(status),
        "pid": pid,
        "bundle": bundle,
        "annotations": {},
    });
    println!("{}", serde_json::to_string(&out)?);
    Ok(())
}

/// `zlayer runtime create <id> --bundle <path>` — creates the container's
/// init state without starting it. Mirrors `runtimes/youki.rs:1020-1044`.
///
/// `log` is accepted for runc compatibility but is currently ignored: the
/// agent runtime routes container stdout/stderr to its own log dir, and
/// libcontainer's own diagnostics already flow through `tracing`.
async fn create(global: &RuntimeGlobal, id: &str, bundle: &Path, log: Option<&Path>) -> Result<()> {
    if let Some(log_path) = log {
        tracing::debug!(
            container = id,
            log = %log_path.display(),
            "--log accepted for runc compatibility; libcontainer diagnostics go to stderr via tracing"
        );
    }

    let id_owned = id.to_string();
    let bundle_owned = bundle.to_path_buf();
    let state_root = global.state_root.clone();

    tokio::task::spawn_blocking(move || -> Result<()> {
        std::fs::create_dir_all(&state_root)
            .with_context(|| format!("create state_root {}", state_root.display()))?;

        let builder = ContainerBuilder::new(id_owned.clone(), SyscallType::Linux)
            .with_root_path(&state_root)
            .with_context(|| format!("with_root_path({})", state_root.display()))?
            .as_init(&bundle_owned)
            // Match the WSL distro default — systemd cgroups are off inside
            // the `zlayer` distro, the cgroupfs driver handles everything.
            .with_systemd(false)
            .with_detach(true);

        let _container = builder
            .build()
            .with_context(|| format!("ContainerBuilder::build for {id_owned}"))?;
        Ok(())
    })
    .await
    .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))??;

    Ok(())
}

/// `zlayer runtime start <id>` — starts a previously-created container.
/// Mirrors `runtimes/youki.rs:1104-1118`.
async fn start(global: &RuntimeGlobal, id: &str) -> Result<()> {
    let root = container_root(global, id);
    let id_owned = id.to_string();

    tokio::task::spawn_blocking(move || -> Result<()> {
        let mut container = Container::load(root)
            .with_context(|| format!("Container::load failed for {id_owned}"))?;
        container
            .start()
            .with_context(|| format!("Container::start failed for {id_owned}"))?;
        Ok(())
    })
    .await
    .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))??;

    Ok(())
}

/// `zlayer runtime kill <id> <signal> [--all]` — sends a signal to the
/// container init process (or every process when `--all` is set). Mirrors
/// `runtimes/youki.rs:1170-1188`.
async fn kill(global: &RuntimeGlobal, id: &str, signal: &str, all: bool) -> Result<()> {
    let root = container_root(global, id);
    let id_owned = id.to_string();
    let sig = parse_signal(signal)?;

    tokio::task::spawn_blocking(move || -> Result<()> {
        let mut container = Container::load(root)
            .with_context(|| format!("Container::load failed for {id_owned}"))?;
        if !container.status().can_kill() {
            anyhow::bail!(
                "container {id_owned} is in state {:?} which cannot receive signals",
                container.status()
            );
        }
        container
            .kill(sig, all)
            .with_context(|| format!("Container::kill failed for {id_owned}"))?;
        Ok(())
    })
    .await
    .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))??;

    Ok(())
}

/// `zlayer runtime delete <id> [--force]` — removes the container's state.
/// With `--force`, also SIGKILLs a running container before deleting. Mirrors
/// `runtimes/youki.rs:1281-1291`.
async fn delete(global: &RuntimeGlobal, id: &str, force: bool) -> Result<()> {
    let root = container_root(global, id);
    let id_owned = id.to_string();

    tokio::task::spawn_blocking(move || -> Result<()> {
        let mut container = Container::load(root)
            .with_context(|| format!("Container::load failed for {id_owned}"))?;
        container
            .delete(force)
            .with_context(|| format!("Container::delete failed for {id_owned}"))?;
        Ok(())
    })
    .await
    .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))??;

    Ok(())
}

/// `zlayer runtime exec <id> -- <args>...` — execs a process inside an
/// existing container by attaching to its namespaces via libcontainer's tenant
/// builder. Mirrors `runtimes/youki.rs:1573-1599`.
///
/// The verb blocks until the exec'd process exits, then forwards its exit
/// status to the parent's process exit code (via the standard tokio main
/// error propagation: a nonzero exit code is reported as an anyhow error
/// whose `Display` carries the signal/code information).
async fn exec(global: &RuntimeGlobal, id: &str, args: &[String]) -> Result<()> {
    if args.is_empty() {
        anyhow::bail!("zlayer runtime exec: no command supplied (use `--` to separate)");
    }

    let id_owned = id.to_string();
    let state_root = global.state_root.clone();
    let cmd = args.to_vec();

    // Build & launch the tenant — returns the exec'd PID. Same shape as
    // `runtimes/youki.rs:1571-1609`.
    let exec_pid = tokio::task::spawn_blocking(move || -> Result<i32> {
        let builder = ContainerBuilder::new(id_owned.clone(), SyscallType::Linux)
            .with_root_path(&state_root)
            .with_context(|| format!("with_root_path({})", state_root.display()))?
            .as_tenant()
            .with_container_args(cmd)
            // Wait for completion — same as the agent runtime's exec path.
            .with_detach(false);

        let pid = builder
            .build()
            .with_context(|| format!("TenantContainerBuilder::build for {id_owned}"))?;
        Ok(pid.as_raw())
    })
    .await
    .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))??;

    // Wait for the exec'd process and forward its exit status. Mirrors the
    // waitpid pattern at `runtimes/youki.rs:1612-1622`.
    let exit_code = tokio::task::spawn_blocking(move || -> Result<i32> {
        use nix::sys::wait::{waitpid, WaitStatus};
        use nix::unistd::Pid;
        let pid = Pid::from_raw(exec_pid);
        match waitpid(pid, None) {
            Ok(WaitStatus::Exited(_, code)) => Ok(code),
            Ok(WaitStatus::Signaled(_, signal, _)) => Ok(128 + (signal as i32)),
            Ok(other) => anyhow::bail!("unexpected wait status: {other:?}"),
            Err(e) => Err(anyhow::anyhow!("waitpid({exec_pid}) failed: {e}")),
        }
    })
    .await
    .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))??;

    if exit_code == 0 {
        Ok(())
    } else {
        // Match runc/youki behaviour: non-zero exit propagates to our caller
        // via a process exit status, not a panic. Since the binary's
        // top-level `main` converts anyhow errors to exit code 1, embed the
        // child code in the message — the daemon caller (Wsl2DelegateRuntime)
        // parses both stdout/stderr and the exit status when it cares.
        anyhow::bail!("exec process exited with status {exit_code}")
    }
}

/// `zlayer runtime events <id> [--stats]` — with `--stats`, emit a one-shot
/// cgroup stats snapshot and exit. Without `--stats`, stream snapshots on
/// the runc-default 5-second interval. Delegates to libcontainer's
/// `Container::events()` which already implements both modes and the JSON
/// serialization (see `libcontainer/src/container/container_events.rs`).
async fn events(global: &RuntimeGlobal, id: &str, stats: bool) -> Result<()> {
    let root = container_root(global, id);
    let id_owned = id.to_string();

    // `Container::events` is blocking (sleeps in a loop when streaming);
    // run it on the blocking pool so the async runtime stays responsive.
    tokio::task::spawn_blocking(move || -> Result<()> {
        let mut container = Container::load(root)
            .with_context(|| format!("Container::load failed for {id_owned}"))?;
        container
            .events(EVENTS_INTERVAL_SECS, stats)
            .with_context(|| format!("Container::events failed for {id_owned}"))?;
        Ok(())
    })
    .await
    .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))??;

    Ok(())
}
