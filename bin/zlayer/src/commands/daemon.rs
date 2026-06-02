use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use tracing::info;

use crate::cli::{DaemonAction, DaemonArgs};

/// Resolved admin-account bootstrap material to inject into the daemon's
/// environment at first start. The password is materialised as a file so the
/// daemon can read it via `ZLAYER_BOOTSTRAP_PASSWORD_FILE` without leaking
/// cleartext through process listings or unit-file Environment= lines.
pub struct AdminBootstrap {
    pub email: String,
    /// Path to a file containing the admin password (chmod 0600 on Unix).
    pub password_file: PathBuf,
}

/// Tunnel-server CLI arguments forwarded to the platform-specific install
/// functions. Stored as `Environment=` lines in the systemd unit (Linux) /
/// `EnvironmentVariables` dict (macOS launchd) so the daemon picks them up
/// at next start via `ZLAYER_TUNNEL_*`.
#[derive(Debug, Clone, Copy, Default)]
pub struct TunnelInstallArgs<'a> {
    pub bind: Option<&'a str>,
    pub tls_cert: Option<&'a Path>,
    pub tls_key: Option<&'a Path>,
    pub disabled: bool,
}

/// Write `password` to `<data_dir>/.bootstrap_password` with mode 0600 on Unix
/// and return the path. Used by the `--admin-password*` flags and the
/// interactive prompt.
fn write_bootstrap_password_file(data_dir: &Path, password: &str) -> Result<PathBuf> {
    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("Failed to create {}", data_dir.display()))?;
    let path = data_dir.join(".bootstrap_password");
    std::fs::write(&path, password)
        .with_context(|| format!("Failed to write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("Failed to chmod 0600 {}", path.display()))?;
    }
    Ok(path)
}

/// Read a single trimmed line from stdin. Used for the admin-email prompt
/// because `dialoguer::Input` requires a feature flag that isn't enabled in
/// `bin/zlayer`'s `dialoguer` dep.
fn read_line_from_stdin(prompt: &str) -> Result<String> {
    use std::io::{BufRead, Write};
    print!("{prompt}");
    std::io::stdout().flush().ok();
    let stdin = std::io::stdin();
    let mut line = String::new();
    stdin
        .lock()
        .read_line(&mut line)
        .context("Failed to read from stdin")?;
    Ok(line.trim().to_string())
}

/// Resolve admin bootstrap credentials, prompting interactively when stdin is
/// a TTY and credentials weren't passed via flags.
///
/// Returns `Ok(None)` when the user opted out, `--no-admin-prompt` was set, or
/// stdin is not a TTY and no flags were supplied. In that case the install
/// proceeds without auto-bootstrapping the admin account; the caller should
/// hint at `zlayer auth bootstrap`.
///
/// Side effect: writes the password to `<data_dir>/.bootstrap_password` (mode
/// 0600 on Unix) and returns its path inside `AdminBootstrap.password_file`.
pub fn resolve_admin_bootstrap(
    data_dir: &Path,
    admin_email: Option<&str>,
    admin_password: Option<&str>,
    admin_password_file: Option<&Path>,
    no_admin_prompt: bool,
) -> Result<Option<AdminBootstrap>> {
    use std::io::IsTerminal;

    // Branch 1: explicit password file.
    if let Some(pw_file) = admin_password_file {
        let email = admin_email.map(str::to_string).ok_or_else(|| {
            anyhow::anyhow!(
                "--admin-password-file requires --admin-email (or ZLAYER_BOOTSTRAP_EMAIL) to be set"
            )
        })?;
        let password = std::fs::read_to_string(pw_file)
            .with_context(|| format!("Failed to read admin password file {}", pw_file.display()))?;
        // Strip a single trailing newline so editors that auto-append one
        // don't silently corrupt the password.
        let password = password.strip_suffix('\n').unwrap_or(&password);
        let password = password.strip_suffix('\r').unwrap_or(password);
        let path = write_bootstrap_password_file(data_dir, password)?;
        return Ok(Some(AdminBootstrap {
            email,
            password_file: path,
        }));
    }

    // Branch 2: explicit cleartext password.
    if let Some(password) = admin_password {
        let email = admin_email.map(str::to_string).ok_or_else(|| {
            anyhow::anyhow!(
                "--admin-password requires --admin-email (or ZLAYER_BOOTSTRAP_EMAIL) to be set"
            )
        })?;
        eprintln!(
            "Warning: --admin-password passes the password on the command line. \
             Prefer --admin-password-file for production installs."
        );
        let path = write_bootstrap_password_file(data_dir, password)?;
        return Ok(Some(AdminBootstrap {
            email,
            password_file: path,
        }));
    }

    // Branch 3: non-interactive or opt-out — skip with a hint.
    if no_admin_prompt || !std::io::stdin().is_terminal() {
        println!(
            "Skipping management-UI admin bootstrap. \
             Run `zlayer auth bootstrap --email <email>` after the daemon is running \
             to create the first admin account."
        );
        return Ok(None);
    }

    // Branch 4: interactive prompt.
    let confirm = dialoguer::Confirm::new()
        .with_prompt("Set up the management UI admin account now?")
        .default(true)
        .interact()
        .context("Failed to read confirmation")?;
    if !confirm {
        println!(
            "Skipping management-UI admin bootstrap. \
             Run `zlayer auth bootstrap --email <email>` after the daemon is running \
             to create the first admin account."
        );
        return Ok(None);
    }

    // Email — pre-fill if it came in via env/flag but not flag-as-password-file path.
    let email = if let Some(e) = admin_email {
        e.to_string()
    } else {
        loop {
            let entered = read_line_from_stdin("Admin email: ")?;
            if entered.is_empty() {
                eprintln!("Email cannot be empty.");
                continue;
            }
            if !entered.contains('@') {
                eprintln!("Email must contain '@'.");
                continue;
            }
            break entered;
        }
    };

    // Password — `dialoguer::Password` handles echo suppression + confirmation.
    let password = loop {
        let p = dialoguer::Password::new()
            .with_prompt("Admin password")
            .with_confirmation("Confirm password", "Passwords don't match")
            .interact()
            .context("Failed to read password")?;
        if p.len() < 8 {
            eprintln!("Password must be at least 8 characters.");
            continue;
        }
        break p;
    };

    let path = write_bootstrap_password_file(data_dir, &password)?;
    Ok(Some(AdminBootstrap {
        email,
        password_file: path,
    }))
}

/// Handle daemon lifecycle commands.
pub(crate) async fn handle_daemon(
    daemon_args: &DaemonArgs,
    data_dir: &Path,
    daemon_name_override: Option<&str>,
) -> Result<()> {
    let daemon_name = crate::cli::resolve_daemon_name(daemon_name_override);
    let action = &daemon_args.action;

    // Privileged actions auto-elevate via `sudo -E` (Unix) or the UAC
    // `runas` verb (Windows). `Status` is read-only and `ResumeFromSnapshot`
    // talks to a running daemon over its socket — neither needs root.
    match action {
        DaemonAction::Install(_) => {
            crate::privilege::ensure_root_or_reexec("install the system service")?;
        }
        DaemonAction::Uninstall { .. } => {
            crate::privilege::ensure_root_or_reexec("uninstall the system service")?;
        }
        DaemonAction::Start => {
            crate::privilege::ensure_root_or_reexec("start the system service")?;
        }
        DaemonAction::Stop => {
            crate::privilege::ensure_root_or_reexec("stop the system service")?;
        }
        DaemonAction::Restart => {
            crate::privilege::ensure_root_or_reexec("restart the system service")?;
        }
        DaemonAction::Reset { .. } => {
            crate::privilege::ensure_root_or_reexec("reset daemon state")?;
        }
        DaemonAction::Status | DaemonAction::ResumeFromSnapshot { .. } => {}
        DaemonAction::Migrate { .. } => {
            crate::privilege::ensure_root_or_reexec("migrate data directory layout")?;
        }
    }

    match action {
        DaemonAction::Install(args) => {
            // `install_wsl` / `no_nat` / `stun_servers` / `turn_servers` /
            // `relay_server_bind` are parsed for forward-compatibility; the
            // Unix install path forwards only the daemon-environment flags
            // below.
            install(
                &daemon_name,
                data_dir,
                args.no_start,
                &args.bind,
                args.socket.as_deref(),
                args.jwt_secret.as_deref(),
                args.no_swagger,
                #[cfg(feature = "docker-compat")]
                args.docker_socket,
                args.with_overlay,
                args.admin_email.as_deref(),
                args.admin_password.as_deref(),
                args.admin_password_file.as_deref(),
                args.no_admin_prompt,
                TunnelInstallArgs {
                    bind: args.tunnel_bind.as_deref(),
                    tls_cert: args.tunnel_tls_cert.as_deref(),
                    tls_key: args.tunnel_tls_key.as_deref(),
                    disabled: args.no_tunnel_server,
                },
            )
            .await
        }
        DaemonAction::Uninstall {
            remove_binary,
            remove_completions,
            purge_data,
        } => {
            uninstall(
                &daemon_name,
                *remove_binary,
                *remove_completions,
                *purge_data,
                data_dir,
            )
            .await
        }
        DaemonAction::Start => start(&daemon_name, data_dir).await,
        DaemonAction::Stop => stop(&daemon_name).await,
        DaemonAction::Restart => restart(&daemon_name, data_dir).await,
        DaemonAction::Status => status(&daemon_name, data_dir).await,
        DaemonAction::Reset { force } => reset(data_dir, *force),
        DaemonAction::ResumeFromSnapshot { path } => {
            let outcome = restore_from_snapshot(path).await;
            print_restore_summary(&outcome);
            Ok(())
        }
        DaemonAction::Migrate {
            data_dir: cli_data_dir,
            dry_run,
        } => {
            // Resolve the effective data dir: explicit --data-dir wins, else
            // use the top-level data_dir we were called with.
            let effective: &Path = cli_data_dir.as_deref().unwrap_or(data_dir);
            run_migrate(effective, *dry_run)
        }
    }
}

/// Implementation of `zlayer daemon migrate`. Extracted from `handle_daemon`
/// to keep that dispatcher short enough for clippy's `too_many_lines` budget,
/// since the dry-run branch has several distinct outcomes worth reporting.
fn run_migrate(effective: &Path, dry_run: bool) -> Result<()> {
    if dry_run {
        // Read-only inspection: report what we WOULD change without
        // touching disk.
        let secrets_path = effective.join("secrets");
        match std::fs::symlink_metadata(&secrets_path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                println!(
                    "No migrations needed (no data directory at {}).",
                    secrets_path.display()
                );
            }
            Err(e) => {
                anyhow::bail!("Failed to stat {}: {e}", secrets_path.display());
            }
            Ok(meta) if meta.is_dir() => {
                println!("No migrations needed (already migrated).");
            }
            Ok(meta) if meta.is_file() => {
                println!(
                    "Would migrate legacy secrets file {} -> {}/secrets.sqlite",
                    secrets_path.display(),
                    secrets_path.display()
                );
            }
            Ok(_) => {
                anyhow::bail!(
                    "{} is not a regular file or directory; refusing to plan migration",
                    secrets_path.display()
                );
            }
        }
        return Ok(());
    }
    let report = crate::migrations::migrate_data_dir(effective)
        .context("Failed to migrate on-disk data directory layout")?;
    if report.changed() {
        for step in &report.steps {
            println!("✓ {step}");
        }
    } else {
        println!("No migrations needed.");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Deployment snapshot / restore helpers (shared across platforms)
// ---------------------------------------------------------------------------

/// Outcome of replaying a deployment snapshot.
///
/// `total_services_to_restore` is the number of services in the snapshot whose
/// `was_running` flag was set. `restored` is the count we successfully scaled
/// back up (or that were already at-or-above the snapshot replica count and
/// therefore needed no action). `failed` lists `(deployment, service, error)`
/// for each service we tried to scale but couldn't. `snapshot_retained` is
/// `true` when the snapshot file was kept on disk (partial failure or read
/// error) so the operator can re-run `zlayer daemon resume-from-snapshot`.
pub struct RestoreOutcome {
    pub total_services_to_restore: usize,
    pub restored: usize,
    pub failed: Vec<(String, String, String)>,
    pub snapshot_retained: bool,
}

/// Best-effort snapshot of currently-running deployments and their service
/// replica counts.
///
/// Calls [`zlayer_client::DaemonClient::try_connect`] first; if no daemon is
/// running, returns `None` immediately (nothing to snapshot). Otherwise
/// iterates deployments and services, emits a JSON file at
/// `<data_dir>/.install-snapshot-<unix-ts>.json`, and returns its path.
///
/// Snapshot failures are intentionally non-fatal — a warning is printed and
/// `None` is returned. The install must never abort because a snapshot
/// couldn't be captured.
pub async fn snapshot_running_deployments(data_dir: &Path) -> Option<PathBuf> {
    let client = match zlayer_client::DaemonClient::try_connect().await {
        Ok(Some(c)) => c,
        Ok(None) => return None,
        Err(e) => {
            eprintln!("Warning: snapshot probe failed (continuing without snapshot): {e}");
            return None;
        }
    };

    let deployments = match client.list_deployments().await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Warning: could not list deployments for snapshot: {e}");
            return None;
        }
    };

    let mut deployments_json: Vec<serde_json::Value> = Vec::new();
    for dep in &deployments {
        let dep_name = match dep.get("name").and_then(|v| v.as_str()) {
            Some(n) if !n.is_empty() => n.to_string(),
            _ => continue,
        };

        let services = match client.list_services(&dep_name).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "Warning: could not list services for deployment '{dep_name}' \
                     during snapshot: {e}"
                );
                continue;
            }
        };

        let mut services_json: Vec<serde_json::Value> = Vec::new();
        for svc in &services {
            let svc_name = match svc.get("name").and_then(|v| v.as_str()) {
                Some(n) if !n.is_empty() => n.to_string(),
                _ => continue,
            };
            let replicas = svc
                .get("replicas")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let was_running = service_value_is_running(svc);
            services_json.push(serde_json::json!({
                "name": svc_name,
                "replicas": replicas,
                "was_running": was_running,
            }));
        }

        if !services_json.is_empty() {
            deployments_json.push(serde_json::json!({
                "name": dep_name,
                "services": services_json,
            }));
        }
    }

    let captured_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let snapshot = serde_json::json!({
        "captured_at": captured_at,
        "deployments": deployments_json,
    });

    let path = data_dir.join(format!(".install-snapshot-{captured_at}.json"));
    let bytes = match serde_json::to_vec_pretty(&snapshot) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Warning: could not serialize snapshot: {e}");
            return None;
        }
    };

    if let Err(e) = std::fs::create_dir_all(data_dir) {
        eprintln!(
            "Warning: could not create snapshot directory {}: {e}",
            data_dir.display()
        );
        return None;
    }

    if let Err(e) = std::fs::write(&path, &bytes) {
        eprintln!(
            "Warning: could not write snapshot to {}: {e}",
            path.display()
        );
        return None;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)) {
            eprintln!(
                "Warning: could not set 0600 on snapshot {}: {e}",
                path.display()
            );
        }
    }

    Some(path)
}

/// Replay a snapshot produced by [`snapshot_running_deployments`].
///
/// Reads the JSON, reconnects to the daemon (this assumes the daemon is up —
/// callers should invoke this *after* `wait_for_daemon_ready`), and for every
/// service whose `was_running` flag was true, ensures the current replica
/// count is at least the snapshotted replica count. If a service is already
/// scaled at-or-above the snapshot, no action is taken — we never scale a
/// service *down* when restoring.
///
/// Single-service failures are collected in [`RestoreOutcome::failed`] but do
/// not abort the rest of the replay. On full success the snapshot file is
/// removed from disk; on partial failure (or snapshot-read failure) it is
/// retained.
#[allow(clippy::too_many_lines)]
pub async fn restore_from_snapshot(snapshot_path: &Path) -> RestoreOutcome {
    let mut outcome = RestoreOutcome {
        total_services_to_restore: 0,
        restored: 0,
        failed: Vec::new(),
        snapshot_retained: true,
    };

    let bytes = match std::fs::read(snapshot_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!(
                "Warning: could not read snapshot {}: {e}",
                snapshot_path.display()
            );
            return outcome;
        }
    };

    let snapshot: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => {
            eprintln!(
                "Warning: snapshot {} is not valid JSON: {e}",
                snapshot_path.display()
            );
            return outcome;
        }
    };

    let client = match zlayer_client::DaemonClient::connect().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Warning: could not reconnect to daemon to replay snapshot: {e}");
            return outcome;
        }
    };

    let Some(deployments) = snapshot.get("deployments").and_then(|v| v.as_array()) else {
        // Empty snapshot → treat as a no-op. Remove the file since there's
        // nothing left to retry.
        let _ = std::fs::remove_file(snapshot_path);
        outcome.snapshot_retained = false;
        return outcome;
    };

    for dep in deployments {
        let Some(dep_name) = dep.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(services) = dep.get("services").and_then(|v| v.as_array()) else {
            continue;
        };

        // Pull current state once per deployment so we don't issue a
        // list_services call per service.
        let current_services = match client.list_services(dep_name).await {
            Ok(s) => s,
            Err(e) => {
                // Every service flagged as was_running in this deployment
                // counts as a failure since we can't even probe state.
                for svc in services {
                    if svc
                        .get("was_running")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false)
                    {
                        outcome.total_services_to_restore += 1;
                        if let Some(svc_name) = svc.get("name").and_then(|v| v.as_str()) {
                            outcome.failed.push((
                                dep_name.to_string(),
                                svc_name.to_string(),
                                format!("list_services failed: {e}"),
                            ));
                        }
                    }
                }
                continue;
            }
        };

        for svc in services {
            let was_running = svc
                .get("was_running")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            if !was_running {
                continue;
            }
            let Some(svc_name) = svc.get("name").and_then(|v| v.as_str()) else {
                continue;
            };
            let snapshot_replicas = svc
                .get("replicas")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);

            outcome.total_services_to_restore += 1;

            // Find the current state for this service, if any.
            let current = current_services.iter().find(|c| {
                c.get("name")
                    .and_then(|v| v.as_str())
                    .is_some_and(|n| n == svc_name)
            });
            let current_replicas = current
                .and_then(|c| c.get("replicas"))
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let currently_running = current.is_some_and(service_value_is_running);

            // Skip the scale RPC when the daemon already brought the service
            // back to (or above) the snapshotted replica count *and* it's
            // currently reporting running. Otherwise re-issue scale.
            if current_replicas >= snapshot_replicas && currently_running && snapshot_replicas > 0 {
                outcome.restored += 1;
                continue;
            }

            if snapshot_replicas == 0 {
                // Nothing meaningful to restore to — count as restored
                // (no-op) so we don't spam failures for stale entries.
                outcome.restored += 1;
                continue;
            }

            let replicas_u32 = u32::try_from(snapshot_replicas).unwrap_or(u32::MAX);
            match client.scale_service(dep_name, svc_name, replicas_u32).await {
                Ok(_) => {
                    outcome.restored += 1;
                }
                Err(e) => {
                    outcome.failed.push((
                        dep_name.to_string(),
                        svc_name.to_string(),
                        e.to_string(),
                    ));
                }
            }
        }
    }

    // Remove the snapshot only on full success (no per-service failures).
    if outcome.failed.is_empty() {
        let _ = std::fs::remove_file(snapshot_path);
        outcome.snapshot_retained = false;
    }

    outcome
}

/// Lenient check for whether a service-state JSON value reports as running.
///
/// Different API surfaces use different field names (`status`, `phase`,
/// `state`) and casing. Match any of them against `running`/`Running`.
fn service_value_is_running(v: &serde_json::Value) -> bool {
    for key in ["status", "phase", "state", "desired_state"] {
        if let Some(s) = v.get(key).and_then(|v| v.as_str()) {
            if s.eq_ignore_ascii_case("running") {
                return true;
            }
        }
    }
    false
}

/// Print a one-line (plus optional failure list) summary of a restore.
fn print_restore_summary(outcome: &RestoreOutcome) {
    println!(
        "Resumed {}/{} services running",
        outcome.restored, outcome.total_services_to_restore
    );
    if !outcome.failed.is_empty() {
        println!("Failed to restore {} service(s):", outcome.failed.len());
        for (dep, svc, err) in &outcome.failed {
            println!("  - {dep}/{svc}: {err}");
        }
        if outcome.snapshot_retained {
            println!(
                "Snapshot retained on disk; replay with `zlayer daemon resume-from-snapshot <path>`"
            );
        }
    }
}

/// Pretty-print the post-install summary. Lines are platform-aware: the
/// `Service:` and `Logs:` rows show the inspection commands appropriate for
/// the current OS (systemctl on Linux, launchctl on macOS, sc/Event Log on
/// Windows). Pass `restore: None` for fresh installs (no prior daemon was
/// running) — that omits the "Resumed:" line entirely.
///
/// Indentation and column alignment match the rest of the install output:
/// two-space indent for value rows, single space between label and value so
/// the labels (`API:`, `Manager:`, `Admin user:`, `Password:`, `Service:`,
/// `Logs:`, `Resumed:`) line up roughly in one column.
pub fn print_install_summary(
    #[cfg_attr(not(target_os = "windows"), allow(unused_variables))] daemon_name: &str,
    bind: &str,
    _data_dir: &Path,
    log_dir: Option<&Path>,
    admin: Option<&AdminBootstrap>,
    restore: Option<&RestoreOutcome>,
) {
    println!("ZLayer daemon installed and running.");
    println!("  API:        http://{bind}");
    println!("  Manager:    http://{bind}/manager");
    if let Some(b) = admin {
        println!("  Admin user: {}", b.email);
        println!("  Password:   {} (chmod 0600)", b.password_file.display());
    }
    #[cfg(target_os = "linux")]
    {
        println!("  Service:    systemctl status zlayer.service");
        println!("  Logs:       journalctl -fu zlayer.service");
        if let Some(dir) = log_dir {
            println!("              {}/daemon.log", dir.display());
        }
        // Group-membership hint. `ensure_zlayer_group` is best-effort and we
        // can't easily distinguish "newly added" from "already a member"
        // here, so print the hint unconditionally on Linux — re-running
        // `newgrp zlayer` in an existing shell that already has the group
        // is a harmless no-op.
        println!("  Group:      You were added to the 'zlayer' group. New shells get this");
        println!("              automatically. For the current shell, run:");
        println!("                newgrp zlayer");
    }
    #[cfg(target_os = "macos")]
    {
        println!("  Service:    launchctl print system/com.zlayer.daemon");
        if let Some(dir) = log_dir {
            println!("  Logs:       {}/daemon.log", dir.display());
        } else {
            println!("  Logs:       <data_dir>/logs/daemon.log");
        }
        // Group-membership hint mirrors the Linux block. macOS supplementary
        // groups behave the same way on stale shells: log out / log back in
        // for the new membership to land, or run `newgrp zlayer` in the
        // current shell for a one-shot subshell that has the group.
        println!("  Group:      You were added to the 'zlayer' group. New shells get this");
        println!("              automatically. For the current shell, run:");
        println!("                newgrp zlayer");
    }
    #[cfg(target_os = "windows")]
    {
        let svc_name = crate::daemon_service::service_name(daemon_name);
        println!("  Service:    sc query {svc_name}");
        println!("  Logs:       Get-EventLog -LogName Application -Source {svc_name}");
        if let Some(dir) = log_dir {
            println!("              {}\\daemon.log", dir.display());
        }
    }
    // Suppress `log_dir`-unused warning on platforms whose blocks above don't
    // reference it (none currently — but keep this defensive in case the
    // layout changes).
    let _ = log_dir;
    if let Some(r) = restore {
        if r.total_services_to_restore == 0 {
            // Snapshot was taken but had nothing meaningful to restore — skip
            // the line; the snapshot itself was a no-op for the operator.
        } else {
            println!(
                "  Resumed:    {}/{} services restored",
                r.restored, r.total_services_to_restore
            );
        }
    }
    println!("  Stop:       zlayer daemon stop");
}

// ---------------------------------------------------------------------------
// macOS (launchd)
// ---------------------------------------------------------------------------

/// Compute the launchd plist `Label` for a given daemon instance name.
///
/// The default `zlayer` instance keeps the legacy label `com.zlayer.daemon`
/// so upgrading does not orphan existing installs. Any other instance name
/// is appended as a suffix (with a leading `zlayer-` prefix stripped if
/// present), allowing multiple instances (`zlayer`, `zlayer-dev`, etc.) to
/// coexist on a single Mac with distinct launchd labels.
#[cfg(target_os = "macos")]
fn plist_label(daemon_name: &str) -> String {
    if daemon_name == "zlayer" {
        "com.zlayer.daemon".to_string()
    } else {
        let suffix = daemon_name.strip_prefix("zlayer-").unwrap_or(daemon_name);
        format!("com.zlayer.daemon-{suffix}")
    }
}

/// Determine plist directory and launchctl target based on privilege level.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn launchd_context() -> Result<(String, String)> {
    let is_root = unsafe { libc::geteuid() } == 0;
    let uid = unsafe { libc::getuid() };

    let plist_dir = if is_root {
        "/Library/LaunchDaemons".to_string()
    } else {
        let home = std::env::var("HOME").context("HOME not set")?;
        let dir = format!("{home}/Library/LaunchAgents");
        std::fs::create_dir_all(&dir).context("Failed to create ~/Library/LaunchAgents")?;
        dir
    };

    let target = if is_root {
        "system".to_string()
    } else {
        format!("gui/{uid}")
    };

    Ok((plist_dir, target))
}

#[cfg(target_os = "macos")]
fn plist_path_for(plist_dir: &str, daemon_name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(plist_dir).join(format!("{}.plist", plist_label(daemon_name)))
}

#[cfg(target_os = "macos")]
#[allow(
    clippy::too_many_lines,
    clippy::too_many_arguments,
    clippy::fn_params_excessive_bools,
    unsafe_code
)]
async fn install(
    daemon_name: &str,
    data_dir: &Path,
    no_start: bool,
    bind: &str,
    socket: Option<&Path>,
    jwt_secret: Option<&str>,
    no_swagger: bool,
    #[cfg(feature = "docker-compat")] docker_socket: bool,
    with_overlay: bool,
    admin_email: Option<&str>,
    admin_password: Option<&str>,
    admin_password_file: Option<&Path>,
    no_admin_prompt: bool,
    tunnel: TunnelInstallArgs<'_>,
) -> Result<()> {
    use tokio::process::Command;

    // with_overlay is Linux-only; macOS sandbox runtime doesn't use overlay networking.
    let _ = with_overlay;

    // Run on-disk layout migrations first so the rest of the install operates
    // on the current expected layout. Idempotent. Use `println!` rather than
    // `tracing::info!` because this is invoked interactively from
    // `sudo zlayer daemon install` and the user expects to see progress.
    let report = crate::migrations::migrate_data_dir(data_dir)
        .context("Failed to migrate on-disk data directory layout")?;
    for step in &report.steps {
        println!("Migration: {step}");
    }

    // Capture the running deployment topology *before* we tear the daemon
    // down. This is best-effort — if no daemon is running, nothing to do.
    let snapshot_path = snapshot_running_deployments(data_dir).await;

    // Resolve the admin bootstrap material *before* we mutate launchd state,
    // so a failed/declined prompt doesn't leave the system half-installed.
    let bootstrap = resolve_admin_bootstrap(
        data_dir,
        admin_email,
        admin_password,
        admin_password_file,
        no_admin_prompt,
    )?;

    let exe = std::env::current_exe().context("Failed to resolve current executable path")?;
    let exe_str = exe.to_string_lossy();

    let log_dir = crate::cli::default_log_dir(data_dir);
    let run_dir = crate::cli::default_run_dir(data_dir);
    // Per-instance socket: two instances (e.g. `zlayer` and `zlayer-dev`)
    // on the same Mac must not share the Unix socket. Caller may override
    // via the explicit `socket` argument; otherwise we derive a name from
    // the daemon instance.
    let socket_path = socket.map_or_else(
        || run_dir.join(format!("{daemon_name}.sock")),
        std::path::Path::to_path_buf,
    );

    // Build ProgramArguments
    // --data-dir is a top-level Cli arg, so it must come BEFORE the subcommand.
    let mut args = vec![
        format!("        <string>{exe_str}</string>"),
        "        <string>--data-dir</string>".to_string(),
        format!("        <string>{}</string>", data_dir.display()),
        "        <string>serve</string>".to_string(),
        "        <string>--bind</string>".to_string(),
        format!("        <string>{bind}</string>"),
        "        <string>--socket</string>".to_string(),
        format!("        <string>{}</string>", socket_path.display()),
    ];

    if let Some(secret) = jwt_secret {
        args.push("        <string>--jwt-secret</string>".to_string());
        args.push(format!("        <string>{secret}</string>"));
    }
    if no_swagger {
        args.push("        <string>--no-swagger</string>".to_string());
    }
    #[cfg(feature = "docker-compat")]
    if docker_socket {
        args.push("        <string>--docker-socket</string>".to_string());
        let default_path = zlayer_paths::ZLayerDirs::default_docker_socket_path();
        args.push("        <string>--docker-socket-path</string>".to_string());
        args.push(format!("        <string>{default_path}</string>"));
    }

    let args_xml = args.join("\n");

    // Forward HOME for correct data directory resolution, plus
    // ZLAYER_BOOTSTRAP_EMAIL / ZLAYER_BOOTSTRAP_PASSWORD_FILE when the admin
    // bootstrap was configured. The daemon side reads these in
    // `bin/zlayer/src/bootstrap_admin.rs` and creates the first admin user
    // on startup.
    let env_xml = {
        let mut entries: Vec<String> = Vec::new();
        if let Ok(home) = std::env::var("HOME") {
            entries.push(format!(
                "        <key>HOME</key>\n        <string>{home}</string>"
            ));
        }
        if let Some(b) = &bootstrap {
            entries.push(format!(
                "        <key>ZLAYER_BOOTSTRAP_EMAIL</key>\n        <string>{}</string>",
                b.email
            ));
            entries.push(format!(
                "        <key>ZLAYER_BOOTSTRAP_PASSWORD_FILE</key>\n        <string>{}</string>",
                b.password_file.display()
            ));
        }
        if let Some(tb) = tunnel.bind {
            entries.push(format!(
                "        <key>ZLAYER_TUNNEL_BIND</key>\n        <string>{tb}</string>"
            ));
        }
        if let Some(cert) = tunnel.tls_cert {
            entries.push(format!(
                "        <key>ZLAYER_TUNNEL_TLS_CERT</key>\n        <string>{}</string>",
                cert.display()
            ));
        }
        if let Some(key) = tunnel.tls_key {
            entries.push(format!(
                "        <key>ZLAYER_TUNNEL_TLS_KEY</key>\n        <string>{}</string>",
                key.display()
            ));
        }
        if tunnel.disabled {
            entries.push(
                "        <key>ZLAYER_DISABLE_TUNNEL_SERVER</key>\n        <string>1</string>"
                    .to_string(),
            );
        }
        if entries.is_empty() {
            String::new()
        } else {
            format!(
                "    <key>EnvironmentVariables</key>\n    <dict>\n{}\n    </dict>",
                entries.join("\n")
            )
        }
    };

    let log_path = log_dir.join("daemon.log");
    let log_path_str = log_path.to_string_lossy();

    let label = plist_label(daemon_name);

    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>GroupName</key>
    <!-- Why: unix group is shared across instances (access-control), not the daemon identity. Same convention as the Linux block. -->
    <string>zlayer</string>
    <key>ProgramArguments</key>
    <array>
{args_xml}
    </array>
{env_xml}
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{log_path_str}</string>
    <key>StandardErrorPath</key>
    <string>{log_path_str}</string>
    <key>WorkingDirectory</key>
    <string>/</string>
</dict>
</plist>"#
    );

    // Create directories
    std::fs::create_dir_all(&log_dir)
        .with_context(|| format!("Failed to create {}", log_dir.display()))?;
    std::fs::create_dir_all(&run_dir)
        .with_context(|| format!("Failed to create {}", run_dir.display()))?;

    // Provision the `zlayer` group BEFORE writing the new plist — the plist
    // sets `<key>GroupName</key><string>zlayer</string>`, so launchd will fail
    // to spawn the daemon if the group does not yet exist.
    ensure_zlayer_group_macos(data_dir).await?;

    let (plist_dir, target) = launchd_context()?;
    let path = plist_path_for(&plist_dir, daemon_name);
    let path_str = path.to_string_lossy().to_string();

    // Unload existing service first (ignore errors)
    let _ = Command::new("launchctl")
        .args(["bootout", &format!("{target}/{label}")])
        .output()
        .await;
    let _ = Command::new("launchctl")
        .args(["unload", &path_str])
        .output()
        .await;

    // Write plist
    std::fs::write(&path, &plist)
        .with_context(|| format!("Failed to write plist to {}", path.display()))?;
    println!("Installed launchd plist: {}", path.display());

    if !no_start {
        // Write spawner PID so the new daemon's cleanup_stale_daemon() won't
        // kill this CLI process while we wait for readiness.
        let spawner_pid_path = data_dir.join("spawner.pid");
        std::fs::write(&spawner_pid_path, std::process::id().to_string()).ok();

        // Clear stale logs so failure diagnostics only show this attempt.
        truncate_daemon_logs(&log_dir);

        // Use modern launchctl bootstrap, fall back to legacy load
        let out = Command::new("launchctl")
            .args(["bootstrap", &target, &path_str])
            .output()
            .await
            .context("Failed to run launchctl bootstrap")?;

        if !out.status.success() {
            let out = Command::new("launchctl")
                .args(["load", "-w", &path_str])
                .output()
                .await
                .context("Failed to run launchctl load")?;
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                let _ = std::fs::remove_file(&spawner_pid_path);
                bail!("launchctl failed to load the service: {stderr}");
            }
        }

        // Kickstart the (possibly already-loaded) service so launchd
        // re-reads the freshly-written plist and the daemon ends up
        // running against the current ProgramArguments / env. `-k`
        // terminates an existing instance first, then respawns. This is
        // the macOS analog of `systemctl daemon-reload` + restart.
        // Tolerate non-zero — on a fresh install the service was just
        // bootstrapped and may not be running yet, and we don't want to
        // fail the whole install over a cosmetic warning.
        let kickstart_out = Command::new("launchctl")
            .args(["kickstart", "-k", &format!("{target}/{label}")])
            .output()
            .await;
        match kickstart_out {
            Ok(o) if !o.status.success() => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                eprintln!(
                    "Warning: launchctl kickstart -k {target}/{label} exited non-zero \
                     (the freshly-installed plist is loaded; if the daemon was already \
                     running it may still be on the old binary until next restart): {stderr}",
                );
            }
            Err(e) => {
                eprintln!("Warning: failed to invoke launchctl kickstart: {e}");
            }
            _ => {}
        }

        print!("Daemon starting...");
        match wait_for_daemon_ready(daemon_name, 45).await {
            Ok(()) => {
                let _ = std::fs::remove_file(&spawner_pid_path);
                println!(" started");
                // Replay any snapshot first so the "Resumed:" line in the
                // summary reflects the actual restore outcome.
                let restore_outcome = if let Some(ref sp) = snapshot_path {
                    Some(restore_from_snapshot(sp).await)
                } else {
                    None
                };
                print_install_summary(
                    daemon_name,
                    bind,
                    data_dir,
                    Some(&log_dir),
                    bootstrap.as_ref(),
                    restore_outcome.as_ref(),
                );
                // Surface the snapshot-retained / per-service-failure hint
                // separately — the summary only carries the success counter.
                if let Some(ref outcome) = restore_outcome {
                    if !outcome.failed.is_empty() {
                        println!("Failed to restore {} service(s):", outcome.failed.len());
                        for (dep, svc, err) in &outcome.failed {
                            println!("  - {dep}/{svc}: {err}");
                        }
                        if outcome.snapshot_retained {
                            println!(
                                "Snapshot retained on disk; replay with \
                                 `zlayer daemon resume-from-snapshot <path>`"
                            );
                        }
                    }
                }
            }
            Err(e) => {
                let _ = std::fs::remove_file(&spawner_pid_path);
                println!(" failed");
                if let Some(ref sp) = snapshot_path {
                    eprintln!(
                        "Pre-install snapshot retained at {}; replay after fixing the install with \
                         `zlayer daemon resume-from-snapshot {}`",
                        sp.display(),
                        sp.display()
                    );
                }
                return Err(e);
            }
        }
    }

    #[cfg(feature = "docker-compat")]
    if docker_socket {
        let shim_dir = zlayer_paths::ZLayerDirs::default_binary_dir();
        if !shim_dir.exists() {
            let _ = std::fs::create_dir_all(&shim_dir);
        }
        for (name, target) in [
            ("docker", "zlayer docker"),
            ("docker-compose", "zlayer docker compose"),
        ] {
            match zlayer_docker::shim::install_shim(&shim_dir, name, target) {
                Ok(zlayer_docker::shim::ShimInstalled::Fresh(p)) => {
                    println!("Installed shim: {} -> {target}", p.display());
                }
                Ok(zlayer_docker::shim::ShimInstalled::ReplacedExisting { shim, backup }) => {
                    println!(
                        "Installed shim: {} -> {target} (backed up existing file to {})",
                        shim.display(),
                        backup.display()
                    );
                }
                Ok(zlayer_docker::shim::ShimInstalled::AlreadyOurs(p)) => {
                    println!("Shim already installed: {}", p.display());
                }
                Err(e) => {
                    eprintln!(
                        "Warning: could not install {name} shim in {}: {e}",
                        shim_dir.display()
                    );
                }
            }
        }
    }

    Ok(())
}

#[cfg(target_os = "macos")]
#[allow(clippy::too_many_lines)]
async fn uninstall(
    daemon_name: &str,
    remove_binary: bool,
    remove_completions: bool,
    purge_data: bool,
    data_dir: &Path,
) -> Result<()> {
    use tokio::process::Command;
    use tokio::time::sleep;

    let (plist_dir, target) = launchd_context()?;
    let path = plist_path_for(&plist_dir, daemon_name);
    let label = plist_label(daemon_name);

    if path.exists() {
        let _ = Command::new("launchctl")
            .args(["bootout", &format!("{target}/{label}")])
            .output()
            .await;
        let _ = Command::new("launchctl")
            .args(["unload", path.to_string_lossy().as_ref()])
            .output()
            .await;

        std::fs::remove_file(&path)
            .with_context(|| format!("Failed to remove {}", path.display()))?;
        println!("Uninstalled launchd plist: {}", path.display());
    } else {
        println!("No launchd plist found (checked {})", path.display());
    }

    #[cfg(feature = "docker-compat")]
    {
        let shim_dir = zlayer_paths::ZLayerDirs::default_binary_dir();
        for (name, target) in [
            ("docker", "zlayer docker"),
            ("docker-compose", "zlayer docker compose"),
        ] {
            match zlayer_docker::shim::uninstall_shim(&shim_dir, name, target, true) {
                Ok(zlayer_docker::shim::ShimUninstalled::Removed(p)) => {
                    println!("Removed shim: {}", p.display());
                }
                Ok(zlayer_docker::shim::ShimUninstalled::RemovedAndRestored {
                    shim,
                    restored_from,
                }) => {
                    println!(
                        "Removed shim {} (restored backup from {})",
                        shim.display(),
                        restored_from.display()
                    );
                }
                Ok(zlayer_docker::shim::ShimUninstalled::NotPresent) => {}
                Err(e) => {
                    eprintln!(
                        "Warning: could not remove {name} shim from {}: {e}",
                        shim_dir.display()
                    );
                }
            }
        }
    }

    if remove_binary {
        let bin_path = zlayer_paths::ZLayerDirs::default_binary_dir().join(daemon_name);
        match std::fs::remove_file(&bin_path) {
            Ok(()) => println!("Removed binary: {}", bin_path.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                println!("Binary already gone: {}", bin_path.display());
            }
            Err(e) => eprintln!("Warning: failed to remove {}: {e}", bin_path.display()),
        }
        // Best-effort: also try Homebrew prefix if `brew` is in PATH.
        if let Ok(out) = Command::new("brew").arg("--prefix").output().await {
            if out.status.success() {
                let prefix = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !prefix.is_empty() {
                    let brew_bin = PathBuf::from(prefix).join("bin").join(daemon_name);
                    if brew_bin.exists() {
                        match std::fs::remove_file(&brew_bin) {
                            Ok(()) => println!("Removed brew binary: {}", brew_bin.display()),
                            Err(e) => {
                                eprintln!("Warning: failed to remove {}: {e}", brew_bin.display());
                            }
                        }
                    }
                }
            }
        }
    }

    if remove_completions {
        let bash_paths = [PathBuf::from(format!(
            "/usr/local/etc/bash_completion.d/{daemon_name}"
        ))];
        for p in bash_paths {
            if p.exists() {
                let _ = std::fs::remove_file(&p);
                println!("Removed bash completion: {}", p.display());
            }
        }
        if let Ok(home) = std::env::var("HOME") {
            let zsh = PathBuf::from(home.clone())
                .join(".zsh/completions")
                .join(format!("_{daemon_name}"));
            if zsh.exists() {
                let _ = std::fs::remove_file(&zsh);
                println!("Removed zsh completion: {}", zsh.display());
            }
            let fish = PathBuf::from(home)
                .join(".config/fish/completions")
                .join(format!("{daemon_name}.fish"));
            if fish.exists() {
                let _ = std::fs::remove_file(&fish);
                println!("Removed fish completion: {}", fish.display());
            }
        }
        // Homebrew-managed completions
        if let Ok(out) = Command::new("brew").arg("--prefix").output().await {
            if out.status.success() {
                let prefix = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !prefix.is_empty() {
                    let brew_zsh = PathBuf::from(&prefix)
                        .join("share/zsh/site-functions")
                        .join(format!("_{daemon_name}"));
                    if brew_zsh.exists() {
                        let _ = std::fs::remove_file(&brew_zsh);
                        println!("Removed brew zsh completion: {}", brew_zsh.display());
                    }
                    let brew_bash = PathBuf::from(&prefix)
                        .join("etc/bash_completion.d")
                        .join(daemon_name);
                    if brew_bash.exists() {
                        let _ = std::fs::remove_file(&brew_bash);
                        println!("Removed brew bash completion: {}", brew_bash.display());
                    }
                }
            }
        }
    }

    if purge_data {
        println!("WARNING: purging data directory {}", data_dir.display());
        println!("  This deletes containers, secrets, raft state, everything.");
        println!("  Press Ctrl-C within 3 seconds to abort.");
        sleep(std::time::Duration::from_secs(3)).await;
        match tokio::fs::remove_dir_all(data_dir).await {
            Ok(()) => println!("Removed data dir: {}", data_dir.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                println!("Data dir already gone: {}", data_dir.display());
            }
            Err(e) => eprintln!("Warning: failed to remove {}: {e}", data_dir.display()),
        }
    }

    Ok(())
}

#[cfg(target_os = "macos")]
async fn start(daemon_name: &str, data_dir: &Path) -> Result<()> {
    use tokio::process::Command;

    let (plist_dir, target) = launchd_context()?;
    let path = plist_path_for(&plist_dir, daemon_name);
    let path_str = path.to_string_lossy().to_string();

    if !path.exists() {
        bail!("Daemon not installed. Run `zlayer daemon install` first.");
    }

    // Write spawner PID so the new daemon's cleanup_stale_daemon() won't
    // kill this CLI process while we wait for readiness.
    let spawner_pid_path = data_dir.join("spawner.pid");
    std::fs::write(&spawner_pid_path, std::process::id().to_string()).ok();

    // Clear stale logs so failure diagnostics only show this attempt.
    truncate_daemon_logs(&crate::cli::default_log_dir(data_dir));

    let out = Command::new("launchctl")
        .args(["bootstrap", &target, &path_str])
        .output()
        .await
        .context("Failed to run launchctl bootstrap")?;

    if !out.status.success() {
        let out = Command::new("launchctl")
            .args(["load", "-w", &path_str])
            .output()
            .await
            .context("Failed to run launchctl load")?;
        if !out.status.success() {
            let _ = std::fs::remove_file(&spawner_pid_path);
            bail!("Failed to start daemon");
        }
    }

    print!("Daemon starting...");
    match wait_for_daemon_ready(daemon_name, 45).await {
        Ok(()) => {
            let _ = std::fs::remove_file(&spawner_pid_path);
            println!(" started");
        }
        Err(e) => {
            let _ = std::fs::remove_file(&spawner_pid_path);
            println!(" failed");
            return Err(e);
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
async fn stop(daemon_name: &str) -> Result<()> {
    use tokio::process::Command;

    let (_plist_dir, target) = launchd_context()?;
    let label = plist_label(daemon_name);

    let out = Command::new("launchctl")
        .args(["bootout", &format!("{target}/{label}")])
        .output()
        .await
        .context("Failed to run launchctl bootout")?;

    if !out.status.success() {
        let (plist_dir, _) = launchd_context()?;
        let path = plist_path_for(&plist_dir, daemon_name);
        let _ = Command::new("launchctl")
            .args(["unload", path.to_string_lossy().as_ref()])
            .output()
            .await;
    }

    println!("Daemon stopped");
    Ok(())
}

#[cfg(target_os = "macos")]
async fn restart(daemon_name: &str, data_dir: &Path) -> Result<()> {
    use tokio::process::Command;

    let (_plist_dir, target) = launchd_context()?;
    let label = plist_label(daemon_name);

    let out = Command::new("launchctl")
        .args(["kickstart", "-k", &format!("{target}/{label}")])
        .output()
        .await
        .context("Failed to run launchctl kickstart")?;

    if !out.status.success() {
        stop(daemon_name).await.ok();
        return start(daemon_name, data_dir).await;
    }

    println!("Daemon restarted");
    Ok(())
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code, clippy::cast_possible_truncation)]
async fn status(daemon_name: &str, data_dir: &Path) -> Result<()> {
    use tokio::process::Command;

    let label = plist_label(daemon_name);
    let out = Command::new("launchctl")
        .args(["list", &label])
        .output()
        .await
        .context("Failed to run launchctl list")?;

    if out.status.success() {
        println!("Service: registered with launchd");
    } else {
        println!("Service: not registered");
    }

    // Check daemon.json for process info
    let metadata_path = data_dir.join("daemon.json");
    if metadata_path.exists() {
        if let Ok(contents) = std::fs::read_to_string(&metadata_path) {
            println!("Daemon metadata: {}", metadata_path.display());
            if let Ok(meta) = serde_json::from_str::<serde_json::Value>(&contents) {
                if let Some(pid) = meta.get("pid").and_then(serde_json::Value::as_u64) {
                    let alive = unsafe { libc::kill(pid as i32, 0) } == 0;
                    println!(
                        "  PID: {pid} ({})",
                        if alive { "running" } else { "not running" }
                    );
                }
                if let Some(bind) = meta.get("api_bind").and_then(serde_json::Value::as_str) {
                    println!("  API: {bind}");
                }
                if let Some(sock) = meta.get("socket_path").and_then(serde_json::Value::as_str) {
                    println!("  Socket: {sock}");
                }
            }
        }
    } else {
        println!("Daemon: not running (no daemon.json)");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Linux (systemd)
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
fn unit_name(daemon_name: &str) -> String {
    format!("{daemon_name}.service")
}

#[cfg(target_os = "linux")]
fn unit_path(daemon_name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from("/etc/systemd/system").join(unit_name(daemon_name))
}

#[cfg(target_os = "linux")]
fn systemctl_args(base_args: &[&str]) -> Vec<String> {
    base_args.iter().copied().map(ToString::to_string).collect()
}

/// Pick a writable system location for the daemon binary, named after the
/// daemon instance so side-by-side daemons land at distinct paths
/// (`/usr/local/bin/<daemon_name>`).
///
/// Delegates to [`zlayer_paths::ZLayerDirs::default_binary_dir`] which
/// write-probes `/usr/local/bin` first, then falls back to the `ZLayer`
/// data dir (`/var/lib/zlayer/bin`) which is always writable.
#[cfg(target_os = "linux")]
fn pick_system_binary_path(daemon_name: &str) -> std::path::PathBuf {
    zlayer_paths::ZLayerDirs::default_binary_dir().join(daemon_name)
}

/// Create the `zlayer` group, add the invoking user to it, and make the
/// shared build-facing data directories group-writable.
///
/// Lets unprivileged users run `zlayer build` against the system-wide data
/// directory without sudo, which otherwise fails silently when the builder's
/// local-registry import hits EACCES on `/var/lib/zlayer/registry/`.
///
/// Scope is deliberately narrow: only `registry`, `cache`, and `bundles` are
/// group-writable. Secrets, raft state, `admin_password`, and live container
/// runtime state (containers/rootfs/volumes) stay root-only.
///
/// Membership in the `zlayer` group is effectively root-equivalent on the
/// host (a group member can publish a manifest that the daemon later runs as
/// root), same trust model as the `docker` group.
///
/// Failures here are non-fatal — the systemd service install must still
/// succeed even when `groupadd` / `usermod` / `chgrp` aren't available, so
/// this only logs warnings and returns `Ok`.
#[cfg(target_os = "linux")]
#[allow(clippy::too_many_lines, unsafe_code)]
async fn ensure_zlayer_group(data_dir: &Path) -> Result<()> {
    use tokio::process::Command;

    // Only provisioning system-wide paths makes sense as root.
    let is_root = unsafe { libc::geteuid() } == 0;
    if !is_root {
        return Ok(());
    }

    // 1. Create the group if it doesn't exist.
    let getent_ok = Command::new("getent")
        .args(["group", "zlayer"])
        .output()
        .await
        .is_ok_and(|out| out.status.success());
    if !getent_ok {
        match Command::new("groupadd")
            .args(["--system", "zlayer"])
            .output()
            .await
        {
            Ok(out) if out.status.success() => {
                println!("Created system group 'zlayer'.");
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                eprintln!("Warning: groupadd zlayer failed: {}", stderr.trim());
                eprintln!("         Unprivileged users will need sudo to run `zlayer build`.");
                return Ok(());
            }
            Err(e) => {
                eprintln!("Warning: could not run groupadd: {e}");
                return Ok(());
            }
        }
    }

    // 2. Add the invoking user to the group (if sudo'd).
    let sudo_user = std::env::var("SUDO_USER")
        .ok()
        .filter(|u| !u.is_empty() && u != "root");

    if let Some(ref user) = sudo_user {
        // Skip if already a member — avoids redundant usermod noise.
        let already_member = Command::new("id")
            .args(["-nG", user])
            .output()
            .await
            .is_ok_and(|out| {
                out.status.success()
                    && String::from_utf8_lossy(&out.stdout)
                        .split_whitespace()
                        .any(|g| g == "zlayer")
            });

        if !already_member {
            match Command::new("usermod")
                .args(["-aG", "zlayer", user])
                .output()
                .await
            {
                Ok(out) if out.status.success() => {
                    println!("Added user '{user}' to group 'zlayer'.");
                }
                Ok(out) => {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    eprintln!(
                        "Warning: usermod -aG zlayer {user} failed: {}",
                        stderr.trim()
                    );
                }
                Err(e) => {
                    eprintln!("Warning: could not run usermod: {e}");
                }
            }
        }
    }

    // 3. Give the installing user write access to the build-facing subdirs.
    //    Only registry/cache/bundles — NOT secrets, raft, containers, rootfs, volumes.
    //
    //    Preferred: chown -R $SUDO_USER:zlayer. UID-based access works in the
    //    user's existing shells immediately, sidestepping the "log out and back
    //    in for supplementary-group membership" trap that the group-only
    //    approach hit. Group membership is still provisioned above so
    //    additional users added later can share these dirs (after their own
    //    re-login). Without SUDO_USER we fall back to chgrp-only.
    let shared_dirs = ["registry", "cache", "bundles"];
    let mut provisioned_any = false;
    for sub in shared_dirs {
        let path = data_dir.join(sub);
        if let Err(e) = std::fs::create_dir_all(&path) {
            eprintln!("Warning: could not create {}: {e}", path.display());
            continue;
        }

        let ownership_ok = if let Some(ref user) = sudo_user {
            Command::new("chown")
                .args(["-R", &format!("{user}:zlayer")])
                .arg(&path)
                .output()
                .await
                .is_ok_and(|out| out.status.success())
        } else {
            Command::new("chgrp")
                .args(["-R", "zlayer"])
                .arg(&path)
                .output()
                .await
                .is_ok_and(|out| out.status.success())
        };
        if !ownership_ok {
            eprintln!(
                "Warning: could not set ownership on {}; unprivileged builds may fail",
                path.display()
            );
            continue;
        }

        // u+rwX,g+rwX → owner and group get read/write; capital X only sets +x
        // on dirs or already-exec files so regular files don't flip executable.
        let _ = Command::new("chmod")
            .args(["-R", "u+rwX,g+rwX"])
            .arg(&path)
            .output()
            .await;

        // setgid on every directory so newly-created files inherit the zlayer
        // group. Applied via `find -type d` so we don't flip g+s on regular
        // files (where SGID has a very different, security-sensitive meaning).
        let _ = Command::new("find")
            .arg(&path)
            .args(["-type", "d", "-exec", "chmod", "g+s", "{}", "+"])
            .output()
            .await;

        provisioned_any = true;
    }

    if provisioned_any {
        if let Some(ref user) = sudo_user {
            println!(
                "Configured build data directories for '{user}': {}",
                shared_dirs.join(", ")
            );
        } else {
            println!(
                "Configured group-writable data directories: {}",
                shared_dirs.join(", ")
            );
        }
    }

    Ok(())
}

/// macOS counterpart of [`ensure_zlayer_group`].  Same trust model and same
/// scope (only the `registry`, `cache`, `bundles` data subdirs are made
/// group-writable), implemented with macOS's Directory Services tools.
///
/// `dseditgroup` reads/writes the local DS node by default, which is what we
/// want — local groups defined under `/Local/Default` are honored by
/// launchd's `<key>GroupName</key>` plist field.
///
/// Failures here are non-fatal: install still completes if `dseditgroup`
/// isn't on `$PATH` or the group cannot be created (the launchd plist will
/// then fail to spawn the daemon with a clearer error than a silent socket
/// unreachable, which is the actual diagnostic improvement here).
#[cfg(target_os = "macos")]
#[allow(clippy::too_many_lines, unsafe_code)]
async fn ensure_zlayer_group_macos(data_dir: &Path) -> Result<()> {
    use tokio::process::Command;

    // dseditgroup creating/modifying a system group requires root.
    let is_root = unsafe { libc::geteuid() } == 0;
    if !is_root {
        return Ok(());
    }

    // 1. Create the group if it doesn't exist.
    let group_exists = Command::new("dseditgroup")
        .args(["-o", "read", "zlayer"])
        .output()
        .await
        .is_ok_and(|out| out.status.success());

    if !group_exists {
        match Command::new("dseditgroup")
            .args([
                "-o",
                "create",
                "-r",
                "ZLayer Daemon Group",
                "-t",
                "group",
                "zlayer",
            ])
            .output()
            .await
        {
            Ok(out) if out.status.success() => {
                println!("Created group 'zlayer'.");
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                eprintln!(
                    "Warning: dseditgroup create zlayer failed: {}",
                    stderr.trim()
                );
                eprintln!("         The launchd unit will fail to start until the group exists.");
                return Ok(());
            }
            Err(e) => {
                eprintln!("Warning: could not run dseditgroup: {e}");
                return Ok(());
            }
        }
    }

    // 2. Add the invoking user to the group (if sudo'd).
    let sudo_user = std::env::var("SUDO_USER")
        .ok()
        .filter(|u| !u.is_empty() && u != "root");

    if let Some(ref user) = sudo_user {
        // dseditgroup checkmember exits 0 if the user is already a member.
        let already_member = Command::new("dseditgroup")
            .args(["-o", "checkmember", "-m", user, "zlayer"])
            .output()
            .await
            .is_ok_and(|out| out.status.success());

        if !already_member {
            match Command::new("dseditgroup")
                .args(["-o", "edit", "-a", user, "-t", "user", "zlayer"])
                .output()
                .await
            {
                Ok(out) if out.status.success() => {
                    println!("Added user '{user}' to group 'zlayer'.");
                }
                Ok(out) => {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    eprintln!(
                        "Warning: dseditgroup edit -a {user} zlayer failed: {}",
                        stderr.trim()
                    );
                }
                Err(e) => {
                    eprintln!("Warning: could not run dseditgroup edit: {e}");
                }
            }
        }
    }

    // 3. Give the installing user write access to the build-facing subdirs.
    //    Same scope as the Linux helper: registry/cache/bundles only.
    let shared_dirs = ["registry", "cache", "bundles"];
    let mut provisioned_any = false;
    for sub in shared_dirs {
        let path = data_dir.join(sub);
        if let Err(e) = std::fs::create_dir_all(&path) {
            eprintln!("Warning: could not create {}: {e}", path.display());
            continue;
        }

        let ownership_ok = if let Some(ref user) = sudo_user {
            Command::new("chown")
                .args(["-R", &format!("{user}:zlayer")])
                .arg(&path)
                .output()
                .await
                .is_ok_and(|out| out.status.success())
        } else {
            Command::new("chgrp")
                .args(["-R", "zlayer"])
                .arg(&path)
                .output()
                .await
                .is_ok_and(|out| out.status.success())
        };
        if !ownership_ok {
            eprintln!(
                "Warning: could not set ownership on {}; unprivileged builds may fail",
                path.display()
            );
            continue;
        }

        let _ = Command::new("chmod")
            .args(["-R", "u+rwX,g+rwX"])
            .arg(&path)
            .output()
            .await;

        // setgid on directories so new files inherit the zlayer group.
        let _ = Command::new("find")
            .arg(&path)
            .args(["-type", "d", "-exec", "chmod", "g+s", "{}", "+"])
            .output()
            .await;

        provisioned_any = true;
    }

    if provisioned_any {
        if let Some(ref user) = sudo_user {
            println!(
                "Configured build data directories for '{user}': {}",
                shared_dirs.join(", ")
            );
        } else {
            println!(
                "Configured group-writable data directories: {}",
                shared_dirs.join(", ")
            );
        }
    }

    Ok(())
}

#[cfg(target_os = "linux")]
#[allow(
    clippy::too_many_lines,
    clippy::too_many_arguments,
    clippy::fn_params_excessive_bools,
    unsafe_code
)]
async fn install(
    daemon_name: &str,
    data_dir: &Path,
    no_start: bool,
    bind: &str,
    socket: Option<&Path>,
    jwt_secret: Option<&str>,
    no_swagger: bool,
    #[cfg(feature = "docker-compat")] docker_socket: bool,
    with_overlay: bool,
    admin_email: Option<&str>,
    admin_password: Option<&str>,
    admin_password_file: Option<&Path>,
    no_admin_prompt: bool,
    tunnel: TunnelInstallArgs<'_>,
) -> Result<()> {
    use std::fmt::Write as _;
    use tokio::process::Command;
    let unit_file_name = unit_name(daemon_name);

    // Run on-disk layout migrations first so the rest of the install operates
    // on the current expected layout. Idempotent. Use `println!` rather than
    // `tracing::info!` because this is invoked interactively from
    // `sudo zlayer daemon install` and the user expects to see progress.
    let report = crate::migrations::migrate_data_dir(data_dir)
        .context("Failed to migrate on-disk data directory layout")?;
    for step in &report.steps {
        println!("Migration: {step}");
    }

    // Capture the running deployment topology *before* we tear the daemon
    // down. If no daemon is running (`try_connect` returns Ok(None)), this
    // is a no-op. Snapshot failures never block the install.
    let snapshot_path = snapshot_running_deployments(data_dir).await;

    // Resolve the admin bootstrap material *before* writing the systemd unit,
    // so the user is prompted before we mutate system state.
    let bootstrap = resolve_admin_bootstrap(
        data_dir,
        admin_email,
        admin_password,
        admin_password_file,
        no_admin_prompt,
    )?;

    let exe = std::env::current_exe().context("Cannot determine zlayer binary path")?;

    // When installing a system-wide service (root), the binary must be in a
    // location accessible to root at runtime.  User home directories are
    // typically mode 0700, so /var/home/user/.local/bin/zlayer is unreachable
    // by the systemd service.  Detect this and copy the binary to
    // /usr/local/bin so the ExecStart path works.
    let exe = {
        let is_root = unsafe { libc::geteuid() } == 0;
        if is_root {
            let in_home = std::env::var("SUDO_USER")
                .ok()
                .and_then(|u| {
                    // Check if exe lives under any home-like prefix
                    let home_prefixes = [format!("/home/{u}"), format!("/var/home/{u}")];
                    home_prefixes.iter().find(|p| exe.starts_with(p)).cloned()
                })
                .is_some()
                || exe.to_string_lossy().contains("/.local/bin/");

            if in_home {
                let system_path = pick_system_binary_path(daemon_name);

                // Stop any running service before overwriting (avoids ETXTBSY)
                let stop_args = systemctl_args(&["stop", unit_file_name.as_str()]);
                let _ = Command::new("systemctl").args(&stop_args).output().await;

                // Unlink destination first — succeeds even while the old process
                // runs (inode stays alive until exit), and the new copy gets a
                // fresh inode that won't conflict with the running text segment.
                let _ = std::fs::remove_file(&system_path);

                std::fs::copy(&exe, &system_path).with_context(|| {
                    format!(
                        "Failed to copy {} to {} (binary is in a user home directory, \
                         inaccessible to the systemd service running as root)",
                        exe.display(),
                        system_path.display()
                    )
                })?;
                // Ensure it's executable
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&system_path, std::fs::Permissions::from_mode(0o755))
                        .ok();
                }
                println!(
                    "Copied binary to {} (original in user home is inaccessible to root service)",
                    system_path.display()
                );
                system_path
            } else {
                exe
            }
        } else {
            exe
        }
    };

    // --data-dir is a top-level Cli arg, so it must come BEFORE the subcommand.
    let mut exec_start = format!(
        "{} --data-dir {} serve --bind {bind}",
        exe.display(),
        data_dir.display()
    );
    if let Some(sock_path) = socket {
        // Custom UDS socket path so side-by-side daemon instances don't
        // collide on the default `/var/run/<daemon-name>.sock`.
        write!(exec_start, " --socket {}", sock_path.display()).unwrap();
    }
    if no_swagger {
        exec_start.push_str(" --no-swagger");
    }
    #[cfg(feature = "docker-compat")]
    if docker_socket {
        exec_start.push_str(" --docker-socket");
    }

    let mut env_line = String::new();
    if let Some(secret) = jwt_secret {
        writeln!(env_line, "Environment=ZLAYER_JWT_SECRET={secret}").unwrap();
    }
    if let Some(b) = &bootstrap {
        writeln!(env_line, "Environment=ZLAYER_BOOTSTRAP_EMAIL={}", b.email).unwrap();
        writeln!(
            env_line,
            "Environment=ZLAYER_BOOTSTRAP_PASSWORD_FILE={}",
            b.password_file.display()
        )
        .unwrap();
    }
    if let Some(tb) = tunnel.bind {
        writeln!(env_line, "Environment=ZLAYER_TUNNEL_BIND={tb}").unwrap();
    }
    if let Some(cert) = tunnel.tls_cert {
        writeln!(
            env_line,
            "Environment=ZLAYER_TUNNEL_TLS_CERT={}",
            cert.display()
        )
        .unwrap();
    }
    if let Some(key) = tunnel.tls_key {
        writeln!(
            env_line,
            "Environment=ZLAYER_TUNNEL_TLS_KEY={}",
            key.display()
        )
        .unwrap();
    }
    if tunnel.disabled {
        env_line.push_str("Environment=ZLAYER_DISABLE_TUNNEL_SERVER=1\n");
    }

    // Pre-create log directory so tracing-appender can write on first start.
    let log_dir = crate::cli::default_log_dir(data_dir);

    // Overlay networking capabilities. The `=-` prefix on `ExecStartPre`
    // tells systemd to ignore failures (e.g. `tun` built into the kernel
    // or `/sbin/modprobe` missing), so the daemon still starts on hosts
    // where `modprobe tun` is unnecessary or unavailable.
    let overlay_block = if with_overlay {
        "ExecStartPre=-/sbin/modprobe tun\n\
         AmbientCapabilities=CAP_NET_ADMIN CAP_SYS_ADMIN\n\
         CapabilityBoundingSet=CAP_NET_ADMIN CAP_SYS_ADMIN\n"
    } else {
        ""
    };

    let unit = format!(
        r"[Unit]
Description=ZLayer Container Orchestration Daemon
Documentation=https://zlayer.dev
After=network-online.target
Wants=network-online.target

[Service]
Type=notify
Group=zlayer
ExecStart={exec_start}
ExecReload=/bin/kill -HUP $MAINPID
TimeoutStartSec=60
Restart=always
RestartSec=5
# Exit code 75 (EX_TEMPFAIL) is used by `zlayer serve --restart-on-exit`
# and the self-update / cluster-upgrade flow to request a supervisor
# respawn. SuccessExitStatus prevents `Restart=on-failure` from treating
# 75 as a hard failure if an operator switches the policy, while
# RestartForceExitStatus guarantees a restart regardless of the chosen
# Restart= policy. Belt + suspenders against future policy edits.
SuccessExitStatus=75
RestartForceExitStatus=75
StandardOutput=journal
StandardError=journal
SyslogIdentifier={daemon_name}
LimitNOFILE=1048576
LimitNPROC=infinity
LimitCORE=infinity
Delegate=yes
KillMode=process
{overlay_block}{env_line}
[Install]
WantedBy=multi-user.target
",
    );

    let path = unit_path(daemon_name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }

    tokio::fs::write(&path, unit)
        .await
        .with_context(|| format!("Failed to write {}", path.display()))?;
    println!("Installed systemd unit: {}", path.display());

    // Provision the `zlayer` group and make build-facing data directories
    // group-writable. Done before systemctl calls so it still takes effect on
    // systemd-less environments (e.g. WSL distros with systemd disabled),
    // where the subsequent daemon-reload/enable calls may fail.
    ensure_zlayer_group(data_dir).await?;

    let reload_args = systemctl_args(&["daemon-reload"]);
    let reload_out = Command::new("systemctl")
        .args(&reload_args)
        .output()
        .await
        .context("Failed to run systemctl daemon-reload")?;
    if !reload_out.status.success() {
        let stderr = String::from_utf8_lossy(&reload_out.stderr);
        bail!("systemctl daemon-reload failed: {stderr}");
    }

    let enable_args = systemctl_args(&["enable", unit_file_name.as_str()]);
    let enable_out = Command::new("systemctl")
        .args(&enable_args)
        .output()
        .await
        .context("Failed to run systemctl enable")?;
    if !enable_out.status.success() {
        let stderr = String::from_utf8_lossy(&enable_out.stderr);
        bail!("systemctl enable failed: {stderr}");
    }

    // Pre-create log directory so tracing-appender can write on first
    // start before init_daemon() runs.
    if let Err(e) = std::fs::create_dir_all(&log_dir) {
        eprintln!("Warning: could not create {}: {e}", log_dir.display());
    }

    // Install Docker + Docker Compose CLI shims when docker-compat is enabled
    #[cfg(feature = "docker-compat")]
    if docker_socket {
        let shim_dir = pick_system_binary_path(daemon_name)
            .parent()
            .unwrap_or(std::path::Path::new("/usr/local/bin"))
            .to_path_buf();
        for (name, target) in [
            ("docker", "zlayer docker"),
            ("docker-compose", "zlayer docker compose"),
        ] {
            match zlayer_docker::shim::install_shim(&shim_dir, name, target) {
                Ok(zlayer_docker::shim::ShimInstalled::Fresh(p)) => {
                    println!("Installed shim: {} -> {target}", p.display());
                }
                Ok(zlayer_docker::shim::ShimInstalled::ReplacedExisting { shim, backup }) => {
                    println!(
                        "Installed shim: {} -> {target} (backed up existing file to {})",
                        shim.display(),
                        backup.display()
                    );
                }
                Ok(zlayer_docker::shim::ShimInstalled::AlreadyOurs(p)) => {
                    println!("Shim already installed: {}", p.display());
                }
                Err(e) => {
                    eprintln!(
                        "Warning: could not install {name} shim in {}: {e}",
                        shim_dir.display()
                    );
                }
            }
        }
    }

    if !no_start {
        // Write spawner PID so the new daemon's cleanup_stale_daemon() won't
        // kill this CLI process while we wait for readiness.
        let spawner_pid_path = data_dir.join("spawner.pid");
        std::fs::write(&spawner_pid_path, std::process::id().to_string()).ok();

        // Clear stale logs so failure diagnostics only show this attempt.
        truncate_daemon_logs(&log_dir);

        // Branch on whether the unit is currently active. If it is, a
        // bare `systemctl start` is a no-op — meaning unit-file changes
        // applied earlier in this install (e.g. updated `Environment=`
        // lines) won't take effect. Use `restart` in that case so the
        // daemon picks up the new ExecStart/Environment.
        let is_active_args = systemctl_args(&["is-active", unit_file_name.as_str()]);
        let is_active_status = Command::new("systemctl")
            .args(&is_active_args)
            .output()
            .await
            .context("Failed to query service active state")?;
        let was_active = is_active_status.status.success();

        let action = if was_active { "restart" } else { "start" };
        let action_args = systemctl_args(&[action, unit_file_name.as_str()]);
        let out = Command::new("systemctl")
            .args(&action_args)
            .output()
            .await
            .with_context(|| format!("Failed to {action} service"))?;
        if !out.status.success() {
            let _ = std::fs::remove_file(&spawner_pid_path);
            let context = get_daemon_failure_context(daemon_name);
            if was_active {
                bail!("Daemon failed to restart.\n{context}");
            }
            bail!("Daemon failed to start.\n{context}");
        }

        print!("Daemon starting...");
        match wait_for_daemon_ready(daemon_name, 45).await {
            Ok(()) => {
                let _ = std::fs::remove_file(&spawner_pid_path);
                if was_active {
                    println!(" restarted via systemd");
                } else {
                    println!(" started via systemd");
                }
                let restore_outcome = if let Some(ref sp) = snapshot_path {
                    Some(restore_from_snapshot(sp).await)
                } else {
                    None
                };
                print_install_summary(
                    daemon_name,
                    bind,
                    data_dir,
                    Some(&log_dir),
                    bootstrap.as_ref(),
                    restore_outcome.as_ref(),
                );
                if let Some(ref outcome) = restore_outcome {
                    if !outcome.failed.is_empty() {
                        println!("Failed to restore {} service(s):", outcome.failed.len());
                        for (dep, svc, err) in &outcome.failed {
                            println!("  - {dep}/{svc}: {err}");
                        }
                        if outcome.snapshot_retained {
                            println!(
                                "Snapshot retained on disk; replay with \
                                 `zlayer daemon resume-from-snapshot <path>`"
                            );
                        }
                    }
                }
            }
            Err(e) => {
                let _ = std::fs::remove_file(&spawner_pid_path);
                println!(" failed");
                if let Some(ref sp) = snapshot_path {
                    eprintln!(
                        "Pre-install snapshot retained at {}; replay after fixing the install with \
                         `zlayer daemon resume-from-snapshot {}`",
                        sp.display(),
                        sp.display()
                    );
                }
                return Err(e);
            }
        }
    }

    Ok(())
}

#[cfg(target_os = "linux")]
#[allow(clippy::too_many_lines)]
async fn uninstall(
    daemon_name: &str,
    remove_binary: bool,
    remove_completions: bool,
    purge_data: bool,
    data_dir: &Path,
) -> Result<()> {
    use tokio::process::Command;
    use tokio::time::sleep;

    let unit_file_name = unit_name(daemon_name);
    let stop_args = systemctl_args(&["stop", unit_file_name.as_str()]);
    let _ = Command::new("systemctl").args(&stop_args).output().await;
    let disable_args = systemctl_args(&["disable", unit_file_name.as_str()]);
    let _ = Command::new("systemctl").args(&disable_args).output().await;

    let path = unit_path(daemon_name);
    if path.exists() {
        tokio::fs::remove_file(&path)
            .await
            .with_context(|| format!("Failed to remove {}", path.display()))?;
        let reload_args = systemctl_args(&["daemon-reload"]);
        let _ = Command::new("systemctl").args(&reload_args).output().await;
        println!("Uninstalled systemd unit: {}", path.display());
    } else {
        println!("No systemd unit found at {}", path.display());
    }

    // Remove Docker + Docker Compose CLI shims that may have been
    // installed by `daemon install --docker-socket`.
    #[cfg(feature = "docker-compat")]
    {
        let shim_dir = pick_system_binary_path(daemon_name)
            .parent()
            .unwrap_or(std::path::Path::new("/usr/local/bin"))
            .to_path_buf();
        for (name, target) in [
            ("docker", "zlayer docker"),
            ("docker-compose", "zlayer docker compose"),
        ] {
            match zlayer_docker::shim::uninstall_shim(&shim_dir, name, target, true) {
                Ok(zlayer_docker::shim::ShimUninstalled::Removed(p)) => {
                    println!("Removed shim: {}", p.display());
                }
                Ok(zlayer_docker::shim::ShimUninstalled::RemovedAndRestored {
                    shim,
                    restored_from,
                }) => {
                    println!(
                        "Removed shim {} (restored backup from {})",
                        shim.display(),
                        restored_from.display()
                    );
                }
                Ok(zlayer_docker::shim::ShimUninstalled::NotPresent) => {
                    // nothing to do
                }
                Err(e) => {
                    eprintln!(
                        "Warning: could not remove {name} shim from {}: {e}",
                        shim_dir.display()
                    );
                }
            }
        }
    }

    if remove_binary {
        let bin_path = pick_system_binary_path(daemon_name);
        match std::fs::remove_file(&bin_path) {
            Ok(()) => println!("Removed binary: {}", bin_path.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                println!("Binary already gone: {}", bin_path.display());
            }
            Err(e) => eprintln!("Warning: failed to remove {}: {e}", bin_path.display()),
        }
    }

    if remove_completions {
        // Bash: /etc/bash_completion.d/<name> + Homebrew prefix
        let bash_paths = [
            PathBuf::from(format!("/etc/bash_completion.d/{daemon_name}")),
            // Homebrew prefix not relevant on Linux; ignore
        ];
        for p in bash_paths {
            if p.exists() {
                let _ = std::fs::remove_file(&p);
                println!("Removed bash completion: {}", p.display());
            }
        }
        // Zsh: $HOME/.zsh/completions/_<name>
        if let Ok(home) = std::env::var("HOME") {
            let zsh = PathBuf::from(home.clone())
                .join(".zsh/completions")
                .join(format!("_{daemon_name}"));
            if zsh.exists() {
                let _ = std::fs::remove_file(&zsh);
                println!("Removed zsh completion: {}", zsh.display());
            }
            let fish = PathBuf::from(home)
                .join(".config/fish/completions")
                .join(format!("{daemon_name}.fish"));
            if fish.exists() {
                let _ = std::fs::remove_file(&fish);
                println!("Removed fish completion: {}", fish.display());
            }
        }
    }

    if purge_data {
        println!("WARNING: purging data directory {}", data_dir.display());
        println!("  This deletes containers, secrets, raft state, everything.");
        println!("  Press Ctrl-C within 3 seconds to abort.");
        sleep(std::time::Duration::from_secs(3)).await;
        match tokio::fs::remove_dir_all(data_dir).await {
            Ok(()) => println!("Removed data dir: {}", data_dir.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                println!("Data dir already gone: {}", data_dir.display());
            }
            Err(e) => eprintln!("Warning: failed to remove {}: {e}", data_dir.display()),
        }
    }

    Ok(())
}

#[cfg(target_os = "linux")]
async fn start(daemon_name: &str, data_dir: &Path) -> Result<()> {
    use tokio::process::Command;

    let unit_file_name = unit_name(daemon_name);
    let path = unit_path(daemon_name);
    if !path.exists() {
        bail!("Daemon not installed. Run `zlayer daemon install` first.");
    }

    // Write spawner PID so the new daemon's cleanup_stale_daemon() won't
    // kill this CLI process while we wait for readiness.
    let spawner_pid_path = data_dir.join("spawner.pid");
    std::fs::write(&spawner_pid_path, std::process::id().to_string()).ok();

    // Clear stale logs so failure diagnostics only show this attempt.
    truncate_daemon_logs(&crate::cli::default_log_dir(data_dir));

    let args = systemctl_args(&["start", unit_file_name.as_str()]);
    let out = Command::new("systemctl")
        .args(&args)
        .output()
        .await
        .context("Failed to start service")?;
    if !out.status.success() {
        let _ = std::fs::remove_file(&spawner_pid_path);
        let context = get_daemon_failure_context(daemon_name);
        bail!("Daemon failed to start.\n{context}");
    }

    print!("Daemon starting...");
    match wait_for_daemon_ready(daemon_name, 45).await {
        Ok(()) => {
            let _ = std::fs::remove_file(&spawner_pid_path);
            println!(" started");
            println!("  Stop: zlayer daemon stop");
        }
        Err(e) => {
            let _ = std::fs::remove_file(&spawner_pid_path);
            println!(" failed");
            return Err(e);
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
async fn stop(daemon_name: &str) -> Result<()> {
    use tokio::process::Command;

    let unit_file_name = unit_name(daemon_name);
    let args = systemctl_args(&["stop", unit_file_name.as_str()]);
    let out = Command::new("systemctl")
        .args(&args)
        .output()
        .await
        .context("Failed to stop service")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("systemctl stop failed: {stderr}");
    }
    println!("Daemon stopped");
    Ok(())
}

#[cfg(target_os = "linux")]
async fn restart(daemon_name: &str, _data_dir: &Path) -> Result<()> {
    use tokio::process::Command;

    let unit_file_name = unit_name(daemon_name);
    let args = systemctl_args(&["restart", unit_file_name.as_str()]);
    let out = Command::new("systemctl")
        .args(&args)
        .output()
        .await
        .context("Failed to restart service")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("systemctl restart failed: {stderr}");
    }
    println!("Daemon restarted");
    Ok(())
}

#[cfg(target_os = "linux")]
async fn status(daemon_name: &str, data_dir: &Path) -> Result<()> {
    use tokio::process::Command;

    let unit_file_name = unit_name(daemon_name);
    let args = systemctl_args(&["status", unit_file_name.as_str()]);
    let out = Command::new("systemctl")
        .args(&args)
        .output()
        .await
        .context("Failed to query service status")?;

    let stdout = String::from_utf8_lossy(&out.stdout);
    if stdout.is_empty() {
        println!("Service: not installed");
    } else {
        println!("{stdout}");
    }

    // Check daemon.json for extra info
    let metadata_path = data_dir.join("daemon.json");
    if metadata_path.exists() {
        if let Ok(contents) = std::fs::read_to_string(&metadata_path) {
            if let Ok(meta) = serde_json::from_str::<serde_json::Value>(&contents) {
                if let Some(bind) = meta.get("api_bind").and_then(|v| v.as_str()) {
                    println!("  API: {bind}");
                }
                if let Some(sock) = meta.get("socket_path").and_then(|v| v.as_str()) {
                    println!("  Socket: {sock}");
                }
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Windows (SCM-managed service)
// ---------------------------------------------------------------------------
//
// After I-1 (`zlayer serve --service` registers with the Service Control
// Manager), I-2..I-5 drive the daemon entirely through SCM:
//
//   - `install`   — `ServiceManager::create_service` registers the daemon
//                   under the service name `ZLayerDaemon`, then starts it
//                   via `Service::start` unless `--no-start` is passed.
//   - `uninstall` — best-effort SCM stop, then `Service::delete` to
//                   deregister.  Foreground-running daemons are unaffected.
//   - `start`     — kept around as a foreground-spawn fallback for users who
//                   haven't run `install` yet; operators using the SCM
//                   service should prefer `sc start ZLayerDaemon` or
//                   (preferred) `zlayer daemon install` then start.
//   - `stop`      — `Service::stop` sends SERVICE_CONTROL_STOP and polls
//                   `query_status` until `Stopped` or 30s timeout.  Falls
//                   back to a message for foreground-running daemons.
//   - `restart`   — `stop` then `start`.
//   - `status`    — `Service::query_status` first, falls back to a TCP
//                   probe via `DaemonClient::try_connect` so a foreground
//                   daemon still reports as "running (foreground)".

/// Windows Win32 error code returned when an SCM operation targets a service
/// that isn't registered.  Named to avoid a `0x424` magic number at each use
/// site; the canonical definition lives in `winerror.h`
/// (`ERROR_SERVICE_DOES_NOT_EXIST`).
#[cfg(target_os = "windows")]
const ERROR_SERVICE_DOES_NOT_EXIST: i32 = 1060;

/// Returns `true` when the given [`windows_service::Error`] wraps the Win32
/// error indicating the target service has not been registered with SCM.
///
/// Callers use this to downgrade "service not found" from a hard failure to
/// a recoverable state (fall back to TCP probe for `status`, print a helpful
/// message for `stop`, no-op for `uninstall`).
#[cfg(target_os = "windows")]
fn is_service_not_found(err: &windows_service::Error) -> bool {
    matches!(
        err,
        windows_service::Error::Winapi(io_err)
            if io_err.raw_os_error() == Some(ERROR_SERVICE_DOES_NOT_EXIST)
    )
}

/// Build the SCM launch-argument vector for `zlayer serve --service ...`.
///
/// Split out so it can be unit-tested without touching SCM: the exact flag
/// set matters for I-2 (if `serve` doesn't recognize one of these, SCM will
/// start the process and it'll exit immediately).
///
/// The JWT secret, if any, is propagated via `--jwt-secret <value>` on the
/// SCM command line. This mirrors the Linux install path, which writes the
/// secret into the systemd unit's `Environment=` line — both are readable
/// by any local admin via `sc qc` / `systemctl cat`, so there's no extra
/// exposure beyond what is already accepted.
#[cfg(target_os = "windows")]
#[allow(clippy::too_many_arguments)]
fn build_service_launch_arguments(
    daemon_name: &str,
    data_dir: &Path,
    bind: &str,
    socket: Option<&Path>,
    jwt_secret: Option<&str>,
    no_swagger: bool,
    #[cfg(feature = "docker-compat")] docker_socket: bool,
    tunnel: TunnelInstallArgs<'_>,
) -> Vec<std::ffi::OsString> {
    use std::ffi::OsString;

    // `--data-dir` and `--daemon-name` are top-level Cli args, so they must
    // come BEFORE the `serve` subcommand — mirrors the Linux/macOS install
    // paths. `--daemon-name` is plumbed onto the top-level `Cli` struct in a
    // later wave; emitting it here unconditionally is forward-compatible
    // (older builds without the flag would reject it, but only newer builds
    // produce these argument vectors).
    let mut args: Vec<OsString> = vec![
        OsString::from("--data-dir"),
        data_dir.as_os_str().to_os_string(),
        OsString::from("--daemon-name"),
        OsString::from(daemon_name),
        OsString::from("serve"),
        OsString::from("--service"),
        OsString::from("--bind"),
        OsString::from(bind),
    ];
    if let Some(sock) = socket {
        args.push(OsString::from("--socket"));
        args.push(sock.as_os_str().to_os_string());
    }
    if no_swagger {
        args.push(OsString::from("--no-swagger"));
    }
    if let Some(secret) = jwt_secret {
        args.push(OsString::from("--jwt-secret"));
        args.push(OsString::from(secret));
    }
    #[cfg(feature = "docker-compat")]
    if docker_socket {
        args.push(OsString::from("--docker-socket"));
    }
    if let Some(tb) = tunnel.bind {
        args.push(OsString::from("--tunnel-bind"));
        args.push(OsString::from(tb));
    }
    if let Some(cert) = tunnel.tls_cert {
        args.push(OsString::from("--tunnel-tls-cert"));
        args.push(OsString::from(cert));
    }
    if let Some(key) = tunnel.tls_key {
        args.push(OsString::from("--tunnel-tls-key"));
        args.push(OsString::from(key));
    }
    if tunnel.disabled {
        args.push(OsString::from("--no-tunnel-server"));
    }
    args
}

#[cfg(target_os = "windows")]
#[allow(
    clippy::too_many_lines,
    clippy::too_many_arguments,
    clippy::fn_params_excessive_bools,
    clippy::items_after_statements
)]
async fn install(
    daemon_name: &str,
    data_dir: &Path,
    no_start: bool,
    bind: &str,
    socket: Option<&Path>,
    jwt_secret: Option<&str>,
    no_swagger: bool,
    #[cfg(feature = "docker-compat")] docker_socket: bool,
    with_overlay: bool,
    admin_email: Option<&str>,
    admin_password: Option<&str>,
    admin_password_file: Option<&Path>,
    no_admin_prompt: bool,
    tunnel: TunnelInstallArgs<'_>,
) -> Result<()> {
    use std::ffi::{OsStr, OsString};
    use windows_service::service::{
        ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType, ServiceType,
    };
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    // with_overlay is Linux-only; Windows SCM doesn't use Linux ambient capabilities.
    let _ = with_overlay;

    // Run on-disk layout migrations first so the rest of the install operates
    // on the current expected layout. Idempotent. Use `println!` rather than
    // `tracing::info!` because this is invoked interactively from an elevated
    // PowerShell prompt and the operator expects to see progress.
    let report = crate::migrations::migrate_data_dir(data_dir)
        .context("Failed to migrate on-disk data directory layout")?;
    for step in &report.steps {
        println!("Migration: {step}");
    }

    // Capture the running deployment topology *before* we tear the daemon
    // down. If no daemon is running, this is a no-op.
    let snapshot_path = snapshot_running_deployments(data_dir).await;

    // Resolve admin bootstrap material before SCM mutation. On Windows, SCM
    // does not inherit env from the installing CLI, and the daemon `serve`
    // subcommand has no `--env` style flag to inject vars into the SCM
    // process environment. So we materialise the password file here and tell
    // the user to wire ZLAYER_BOOTSTRAP_EMAIL / ZLAYER_BOOTSTRAP_PASSWORD_FILE
    // separately (or run `zlayer auth bootstrap` after start). The email is
    // also written to `<data_dir>\.bootstrap_email` for parity with the
    // password file so an operator can script the env wiring.
    let bootstrap = resolve_admin_bootstrap(
        data_dir,
        admin_email,
        admin_password,
        admin_password_file,
        no_admin_prompt,
    )?;
    if let Some(b) = &bootstrap {
        let email_path = data_dir.join(".bootstrap_email");
        if let Err(e) = std::fs::write(&email_path, &b.email) {
            eprintln!(
                "Warning: failed to write bootstrap email to {}: {e}",
                email_path.display()
            );
        }
    }

    // Pre-create the log directory so tracing-appender has a destination on
    // first SCM start — parity with the systemd install flow.
    //
    // NOTE: Windows SCM does not natively redirect a service's stdout/stderr
    // to a file (unlike launchd's `StandardErrorPath` or systemd's
    // `StandardError=journal`). Pre-init crashes (panics before the tracing
    // subscriber attaches) are therefore lost — there is no equivalent of
    // `journalctl -u zlayer` for those frames. The daemon catches this in
    // practice by initialising tracing-appender to `log_dir` very early in
    // `serve`, so anything after subscriber init lands on disk. If we ever
    // need pre-init capture on Windows, the path is wrapping `serve` in a
    // shim that redirects its handles, or attaching ETW.
    let log_dir = crate::cli::default_log_dir(data_dir);
    if let Err(e) = std::fs::create_dir_all(&log_dir) {
        eprintln!("Warning: could not create {}: {e}", log_dir.display());
    }

    let exe = std::env::current_exe().context("Failed to resolve current executable path")?;

    let launch_arguments = build_service_launch_arguments(
        daemon_name,
        data_dir,
        bind,
        socket,
        jwt_secret,
        no_swagger,
        #[cfg(feature = "docker-compat")]
        docker_socket,
        tunnel,
    );

    let svc_name = crate::daemon_service::service_name(daemon_name);
    let svc_display = crate::daemon_service::display_name(daemon_name);

    let service_info = ServiceInfo {
        name: OsString::from(svc_name.clone()),
        display_name: OsString::from(svc_display.clone()),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe.clone(),
        launch_arguments,
        dependencies: vec![],
        // None = LocalSystem. Matches the task spec and the Linux default
        // (root). Enterprise installs that need a dedicated service account
        // can `sc config` after install.
        account_name: None,
        account_password: None,
    };

    let manager = ServiceManager::local_computer(None::<&OsStr>, ServiceManagerAccess::ALL_ACCESS)
        .context(
            "Failed to open Service Control Manager. \
             Run this command from an elevated (Administrator) prompt.",
        )?;

    // `create_service` fails with ERROR_SERVICE_EXISTS (1073) if the unit
    // is already registered. Treat that as a reinstall: stop + delete the
    // existing service, then re-create with the (possibly updated)
    // `service_info`. Anything else propagates with the original context.
    const ERROR_SERVICE_EXISTS: i32 = 1073;
    let service = match manager.create_service(&service_info, ServiceAccess::ALL_ACCESS) {
        Ok(s) => s,
        Err(e) => {
            let already_exists = matches!(
                &e,
                windows_service::Error::Winapi(io_err)
                    if io_err.raw_os_error() == Some(ERROR_SERVICE_EXISTS)
            );
            if !already_exists {
                return Err(e).with_context(|| {
                    format!(
                        "Failed to create Windows Service '{svc_name}'. \
                         If the service is already registered, run `zlayer daemon uninstall` first."
                    )
                });
            }

            // Open the existing service and tear it down before recreating.
            // STOP + QUERY_STATUS for the graceful-stop poll, DELETE for the
            // teardown that follows.
            let existing = manager
                .open_service(
                    svc_name.as_str(),
                    ServiceAccess::STOP | ServiceAccess::QUERY_STATUS | ServiceAccess::DELETE,
                )
                .with_context(|| {
                    format!(
                        "Service '{svc_name}' already exists but could not be opened for replacement"
                    )
                })?;

            // Best-effort stop. ERROR_SERVICE_NOT_ACTIVE (1062) means the
            // service was already stopped — fine. Other errors are surfaced
            // because they likely mean we won't be able to delete either.
            const ERROR_SERVICE_NOT_ACTIVE: i32 = 1062;
            match existing.stop() {
                Ok(_) => {
                    // Wait briefly for the service to actually reach Stopped
                    // before deleting; SCM rejects delete on a STOP_PENDING
                    // service in some configurations.
                    use std::time::{Duration, Instant};
                    use windows_service::service::ServiceState;
                    let deadline = Instant::now() + Duration::from_secs(10);
                    loop {
                        match existing.query_status() {
                            Ok(status) if status.current_state == ServiceState::Stopped => break,
                            Ok(_) => {}
                            Err(_) => break,
                        }
                        if Instant::now() >= deadline {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                }
                Err(stop_err) => {
                    let already_stopped = matches!(
                        &stop_err,
                        windows_service::Error::Winapi(io_err)
                            if io_err.raw_os_error() == Some(ERROR_SERVICE_NOT_ACTIVE)
                    );
                    if !already_stopped {
                        eprintln!(
                            "Warning: failed to stop existing service before replace: {stop_err}"
                        );
                    }
                }
            }

            existing.delete().with_context(|| {
                format!(
                    "Failed to delete existing Windows Service '{svc_name}' before replacing it"
                )
            })?;

            // Drop the handle so SCM finishes tearing down the registration
            // before we recreate. Without this, a second `create_service`
            // can race with pending deletion and return ERROR_SERVICE_MARKED_FOR_DELETE.
            drop(existing);

            println!("Replaced existing service '{svc_name}'");

            manager
                .create_service(&service_info, ServiceAccess::ALL_ACCESS)
                .with_context(|| {
                    format!(
                        "Failed to recreate Windows Service '{svc_name}' after replacing existing instance"
                    )
                })?
        }
    };

    // NB: `jwt_secret` is baked into the SCM command line via
    // `build_service_launch_arguments` above — SCM does not inherit env
    // from the installing CLI, so env-passing wouldn't work here.

    println!("Registered Windows Service '{svc_name}' (display: '{svc_display}').");
    println!("  Binary:   {}", exe.display());
    println!("  Data dir: {}", data_dir.display());
    println!("  Bind:     {bind}");
    println!("  Account:  LocalSystem");
    println!("  Startup:  AutoStart");
    if no_swagger {
        println!("  Swagger:  disabled");
    }

    // Configure SCM failure-restart actions so the daemon respawns on any
    // non-zero exit (including 75 / EX_TEMPFAIL from
    // `zlayer serve --restart-on-exit` and the self-update / cluster-upgrade
    // flow). The `windows-service` crate as of this writing doesn't expose
    // `ChangeServiceConfig2` for SERVICE_CONFIG_FAILURE_ACTIONS, so we shell
    // out to `sc.exe failure`. Best-effort: log a warning on failure rather
    // than rolling the install back — the service is still registered and
    // operators can re-run `sc failure` manually if needed.
    //
    // Reset counter: 86400s (24h) before SCM forgets a previous failure.
    // Actions: restart at 5s, 5s, 5s. After three consecutive failures
    // within the reset window SCM stops trying, matching systemd's
    // `StartLimitBurst=3 / StartLimitIntervalSec=86400` defaults.
    {
        let sc_out = tokio::process::Command::new("sc.exe")
            .args([
                "failure",
                svc_name.as_str(),
                "reset=",
                "86400",
                "actions=",
                "restart/5000/restart/5000/restart/5000",
            ])
            .output()
            .await;
        match sc_out {
            Ok(o) if !o.status.success() => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                let stdout = String::from_utf8_lossy(&o.stdout);
                eprintln!(
                    "Warning: `sc failure {svc_name}` exited non-zero. Service will NOT auto-restart \
                     on exit code 75 until this is configured. stderr={stderr} stdout={stdout}"
                );
            }
            Err(e) => {
                eprintln!(
                    "Warning: failed to invoke `sc.exe failure` to configure auto-restart \
                     for service '{svc_name}': {e}"
                );
            }
            _ => {
                println!("  Restart:  on failure (3x at 5s intervals, 86400s reset)");
            }
        }
    }

    if !no_start {
        service.start::<&OsStr>(&[]).with_context(|| {
            format!(
                "Failed to start Windows Service '{svc_name}'. \
                 Check the Windows Event Log (Application) for startup errors."
            )
        })?;

        print!("Daemon starting...");
        // Poll the API endpoint until it's reachable — same readiness
        // signal as the systemd/launchd paths.
        match wait_for_daemon_ready(daemon_name, 45).await {
            Ok(()) => {
                println!(" started");
                let restore_outcome = if let Some(ref sp) = snapshot_path {
                    Some(restore_from_snapshot(sp).await)
                } else {
                    None
                };
                print_install_summary(
                    daemon_name,
                    bind,
                    data_dir,
                    Some(&log_dir),
                    bootstrap.as_ref(),
                    restore_outcome.as_ref(),
                );
                if let Some(b) = &bootstrap {
                    // Windows-specific addendum: SCM doesn't inherit env from
                    // the installing CLI, so the bootstrap files have to be
                    // wired into the service environment by hand (or via a
                    // post-start `zlayer auth bootstrap` call).
                    println!(
                        "  Note:       Windows SCM does not inherit env. Set \
                         ZLAYER_BOOTSTRAP_EMAIL/ZLAYER_BOOTSTRAP_PASSWORD_FILE \
                         via `sc config` (files in {}), or run \
                         `zlayer auth bootstrap --email {} --password-file {}` \
                         after the daemon is reachable.",
                        data_dir.display(),
                        b.email,
                        b.password_file.display()
                    );
                }
                if let Some(ref outcome) = restore_outcome {
                    if !outcome.failed.is_empty() {
                        println!("Failed to restore {} service(s):", outcome.failed.len());
                        for (dep, svc, err) in &outcome.failed {
                            println!("  - {dep}/{svc}: {err}");
                        }
                        if outcome.snapshot_retained {
                            println!(
                                "Snapshot retained on disk; replay with \
                                 `zlayer daemon resume-from-snapshot <path>`"
                            );
                        }
                    }
                }
            }
            Err(e) => {
                println!(" failed");
                if let Some(ref sp) = snapshot_path {
                    eprintln!(
                        "Pre-install snapshot retained at {}; replay after fixing the install with \
                         `zlayer daemon resume-from-snapshot {}`",
                        sp.display(),
                        sp.display()
                    );
                }
                return Err(e);
            }
        }
    }

    #[cfg(feature = "docker-compat")]
    if docker_socket {
        let shim_dir = zlayer_paths::ZLayerDirs::default_binary_dir();
        if !shim_dir.exists() {
            let _ = std::fs::create_dir_all(&shim_dir);
        }
        for (name, target) in [
            ("docker", "zlayer docker"),
            ("docker-compose", "zlayer docker compose"),
        ] {
            match zlayer_docker::shim::install_shim(&shim_dir, name, target) {
                Ok(zlayer_docker::shim::ShimInstalled::Fresh(p)) => {
                    println!("Installed shim: {} -> {target}", p.display());
                }
                Ok(zlayer_docker::shim::ShimInstalled::ReplacedExisting { shim, backup }) => {
                    println!(
                        "Installed shim: {} -> {target} (backed up existing file to {})",
                        shim.display(),
                        backup.display()
                    );
                }
                Ok(zlayer_docker::shim::ShimInstalled::AlreadyOurs(p)) => {
                    println!("Shim already installed: {}", p.display());
                }
                Err(e) => {
                    eprintln!(
                        "Warning: could not install {name} shim in {}: {e}",
                        shim_dir.display()
                    );
                }
            }
        }
    }

    Ok(())
}

#[cfg(target_os = "windows")]
#[allow(clippy::too_many_lines)]
async fn uninstall(
    daemon_name: &str,
    remove_binary: bool,
    remove_completions: bool,
    purge_data: bool,
    data_dir: &Path,
) -> Result<()> {
    use std::ffi::OsStr;
    use tokio::time::sleep;
    use windows_service::service::ServiceAccess;
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    // Track whether the SCM service was found so the post-stop cleanup flags
    // (--remove-binary / --remove-completions / --purge-data) still run even
    // if the service was never registered.
    let mut service_unregistered = false;

    // Best-effort stop first so we don't leave an orphaned process after
    // delete. Ignore errors — if it's already stopped (or not registered),
    // `delete` below will pick up the slack.
    let _ = stop(daemon_name).await;

    let manager =
        match ServiceManager::local_computer(None::<&OsStr>, ServiceManagerAccess::CONNECT) {
            Ok(m) => m,
            Err(e) => {
                return Err(e).context(
                    "Failed to open Service Control Manager. \
                 Run this command from an elevated (Administrator) prompt.",
                );
            }
        };

    let svc_name = crate::daemon_service::service_name(daemon_name);

    match manager.open_service(
        svc_name.as_str(),
        ServiceAccess::DELETE | ServiceAccess::STOP | ServiceAccess::QUERY_STATUS,
    ) {
        Ok(service) => {
            service
                .delete()
                .with_context(|| format!("Failed to delete Windows Service '{svc_name}'"))?;
            println!("Unregistered Windows Service '{svc_name}'.");
            service_unregistered = true;
        }
        Err(e) if is_service_not_found(&e) => {
            println!("Windows Service '{svc_name}' is not registered; nothing to uninstall.");
        }
        Err(e) => {
            return Err(e).with_context(|| format!("Failed to open Windows Service '{svc_name}'"));
        }
    }
    let _ = service_unregistered; // silence unused-var warning when not consumed downstream

    #[cfg(feature = "docker-compat")]
    {
        let shim_dir = zlayer_paths::ZLayerDirs::default_binary_dir();
        for (name, target) in [
            ("docker", "zlayer docker"),
            ("docker-compose", "zlayer docker compose"),
        ] {
            match zlayer_docker::shim::uninstall_shim(&shim_dir, name, target, true) {
                Ok(zlayer_docker::shim::ShimUninstalled::Removed(p)) => {
                    println!("Removed shim: {}", p.display());
                }
                Ok(zlayer_docker::shim::ShimUninstalled::RemovedAndRestored {
                    shim,
                    restored_from,
                }) => {
                    println!(
                        "Removed shim {} (restored backup from {})",
                        shim.display(),
                        restored_from.display()
                    );
                }
                Ok(zlayer_docker::shim::ShimUninstalled::NotPresent) => {}
                Err(e) => {
                    eprintln!(
                        "Warning: could not remove {name} shim from {}: {e}",
                        shim_dir.display()
                    );
                }
            }
        }
    }

    if remove_binary {
        let bin_path =
            zlayer_paths::ZLayerDirs::default_binary_dir().join(format!("{daemon_name}.exe"));
        match std::fs::remove_file(&bin_path) {
            Ok(()) => println!("Removed binary: {}", bin_path.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                println!("Binary already gone: {}", bin_path.display());
            }
            Err(e) => eprintln!("Warning: failed to remove {}: {e}", bin_path.display()),
        }
    }

    if remove_completions {
        // PowerShell completions, if installed, would live under the user's
        // PowerShell module path. The installer does not currently create
        // these, so this branch is a no-op on Windows.
        let _ = daemon_name;
    }

    if purge_data {
        println!("WARNING: purging data directory {}", data_dir.display());
        println!("  This deletes containers, secrets, raft state, everything.");
        println!("  Press Ctrl-C within 3 seconds to abort.");
        sleep(std::time::Duration::from_secs(3)).await;

        // Tear down the host-level HCN overlay network(s) ZLayer created. They
        // persist across daemon restarts/reinstalls and are deleted ONLY here,
        // on a full uninstall. Must run before the data dir (which holds the
        // network marker) is removed. HCN calls are blocking → spawn_blocking.
        {
            let dd = data_dir.to_path_buf();
            let dn = daemon_name.to_string();
            let _ = tokio::task::spawn_blocking(move || {
                zlayer_agent::runtimes::hcs::purge_managed_networks(&dd, &dn);
            })
            .await;
        }

        match tokio::fs::remove_dir_all(data_dir).await {
            Ok(()) => println!("Removed data dir: {}", data_dir.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                println!("Data dir already gone: {}", data_dir.display());
            }
            Err(e) => eprintln!("Warning: failed to remove {}: {e}", data_dir.display()),
        }
    }

    Ok(())
}

#[cfg(target_os = "windows")]
async fn start(daemon_name: &str, data_dir: &Path) -> Result<()> {
    // Kept as a foreground-spawn fallback for users who haven't registered
    // the SCM service (e.g. during local development). For an installed
    // service, callers should use `sc start ZLayerDaemon` or re-run
    // `zlayer daemon install` — we don't promote `start` to an SCM call
    // here because `install` already starts the service on registration.
    let bind = "127.0.0.1:3669";
    spawn_daemon_windows(daemon_name, data_dir, bind, None, false).await
}

#[cfg(target_os = "windows")]
async fn stop(daemon_name: &str) -> Result<()> {
    use std::ffi::OsStr;
    use std::time::{Duration, Instant};
    use windows_service::service::{ServiceAccess, ServiceState};
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    let manager = ServiceManager::local_computer(None::<&OsStr>, ServiceManagerAccess::CONNECT)
        .context(
            "Failed to open Service Control Manager. \
             Run this command from an elevated (Administrator) prompt.",
        )?;

    let svc_name = crate::daemon_service::service_name(daemon_name);

    let service = match manager.open_service(
        svc_name.as_str(),
        ServiceAccess::STOP | ServiceAccess::QUERY_STATUS,
    ) {
        Ok(s) => s,
        Err(e) if is_service_not_found(&e) => {
            println!(
                "Windows Service '{svc_name}' is not registered. \
                 If a foreground `zlayer serve` is running, press Ctrl+C in its window."
            );
            return Ok(());
        }
        Err(e) => {
            return Err(e).with_context(|| format!("Failed to open Windows Service '{svc_name}'"));
        }
    };

    // Send SERVICE_CONTROL_STOP. If the service is already stopped, SCM
    // returns an error we can ignore — the subsequent poll will confirm.
    match service.stop() {
        Ok(_) => {}
        Err(e) => {
            // Already stopped (ERROR_SERVICE_NOT_ACTIVE = 1062) is a no-op;
            // any other SCM error is worth surfacing.
            const ERROR_SERVICE_NOT_ACTIVE: i32 = 1062;
            let already_stopped = matches!(
                &e,
                windows_service::Error::Winapi(io_err)
                    if io_err.raw_os_error() == Some(ERROR_SERVICE_NOT_ACTIVE)
            );
            if !already_stopped {
                return Err(e).context("Failed to send Stop control to Windows Service");
            }
        }
    }

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let status = service
            .query_status()
            .context("Failed to query Windows Service status during stop")?;
        if status.current_state == ServiceState::Stopped {
            println!("Daemon stopped.");
            return Ok(());
        }
        if Instant::now() >= deadline {
            println!(
                "Daemon did not reach Stopped state within 30s (current: {:?}). \
                 The service may still be shutting down; check `sc query {svc_name}`.",
                status.current_state
            );
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

#[cfg(target_os = "windows")]
async fn restart(daemon_name: &str, data_dir: &Path) -> Result<()> {
    stop(daemon_name).await.ok();
    start(daemon_name, data_dir).await
}

#[cfg(target_os = "windows")]
async fn status(daemon_name: &str, _data_dir: &Path) -> Result<()> {
    use std::ffi::OsStr;
    use windows_service::service::{ServiceAccess, ServiceState};
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    let svc_name = crate::daemon_service::service_name(daemon_name);

    // Try SCM first. If the service is registered, its state is the
    // authoritative answer — a foreground daemon on a different bind could
    // give a false "running" via TCP probe otherwise.
    let manager_result =
        ServiceManager::local_computer(None::<&OsStr>, ServiceManagerAccess::CONNECT);

    if let Ok(manager) = manager_result {
        match manager.open_service(svc_name.as_str(), ServiceAccess::QUERY_STATUS) {
            Ok(service) => {
                let status = service
                    .query_status()
                    .context("Failed to query Windows Service status")?;
                let label = match status.current_state {
                    ServiceState::Running => "Running",
                    ServiceState::Stopped => "Stopped",
                    ServiceState::StartPending => "StartPending",
                    ServiceState::StopPending => "StopPending",
                    ServiceState::Paused => "Paused",
                    ServiceState::PausePending => "PausePending",
                    ServiceState::ContinuePending => "ContinuePending",
                };
                println!("Daemon: {label} (Windows Service '{svc_name}')");
                if let Some(pid) = status.process_id {
                    println!("  PID: {pid}");
                }
                return Ok(());
            }
            Err(e) if is_service_not_found(&e) => {
                // Fall through to the TCP-probe fallback — the user may be
                // running a foreground `zlayer serve` rather than an
                // installed service.
            }
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("Failed to open Windows Service '{svc_name}'"));
            }
        }
    }

    // SCM service not registered (or SCM handle unavailable to non-admin
    // callers). Fall back to the old TCP probe, which correctly detects a
    // foreground-running daemon.
    match zlayer_client::DaemonClient::try_connect().await {
        Ok(Some(_)) => {
            println!("Daemon: running (foreground)");
        }
        _ => {
            println!("Daemon: stopped");
        }
    }
    Ok(())
}

#[cfg(target_os = "windows")]
async fn spawn_daemon_windows(
    daemon_name: &str,
    data_dir: &Path,
    bind: &str,
    jwt_secret: Option<&str>,
    no_swagger: bool,
) -> Result<()> {
    use tokio::process::Command;

    let exe = std::env::current_exe().context("Failed to resolve current executable path")?;

    let log_dir = crate::cli::default_log_dir(data_dir);
    truncate_daemon_logs(&log_dir);

    // Write spawner PID so the new daemon's cleanup_stale_daemon() won't
    // kill this CLI process while we wait for readiness. Mirrors the Unix
    // behavior in `install`/`start`.
    let spawner_pid_path = data_dir.join("spawner.pid");
    std::fs::write(&spawner_pid_path, std::process::id().to_string()).ok();

    // --data-dir and --daemon-name are top-level Cli args, so they must come
    // BEFORE the subcommand — matches the Linux systemd unit. `--daemon-name`
    // is added to the top-level Cli struct in a later wave; the
    // foreground-spawned daemon needs to receive its instance identity the
    // same way the SCM-launched service does.
    let mut cmd = Command::new(&exe);
    cmd.arg("--data-dir").arg(data_dir);
    cmd.arg("--daemon-name").arg(daemon_name);
    cmd.arg("serve").arg("--daemon").arg("--bind").arg(bind);
    if no_swagger {
        cmd.arg("--no-swagger");
    }
    if let Some(secret) = jwt_secret {
        cmd.env("ZLAYER_JWT_SECRET", secret);
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    // Spawn detached — `zlayer serve` on Windows stays in the foreground
    // of its own process (no fork/daemonize), so we must not `.status()`
    // here or we'd block forever waiting for the daemon to exit.
    let child = cmd
        .spawn()
        .with_context(|| format!("Failed to spawn daemon process: {}", exe.display()))?;
    // Dropping the Child handle without awaiting it leaves the daemon
    // running independently of this CLI process.
    drop(child);

    print!("Daemon starting...");
    match wait_for_daemon_ready(daemon_name, 45).await {
        Ok(()) => {
            let _ = std::fs::remove_file(&spawner_pid_path);
            println!(" started");
            println!("  Stop: see `zlayer daemon stop`");
        }
        Err(e) => {
            let _ = std::fs::remove_file(&spawner_pid_path);
            println!(" failed");
            return Err(e);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Log helpers (shared across platforms)
// ---------------------------------------------------------------------------

/// Clear daemon log files so failure diagnostics only show the current attempt.
fn truncate_daemon_logs(log_dir: &std::path::Path) {
    if let Ok(entries) = std::fs::read_dir(log_dir) {
        for entry in entries.flatten() {
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with("daemon.log")
            {
                let _ = std::fs::File::create(entry.path());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Readiness check (shared across platforms)
// ---------------------------------------------------------------------------

/// Poll for daemon readiness by connecting to the API socket.
///
/// The API socket is the unified readiness signal: the daemon binds it only
/// after all infrastructure phases (overlay, raft, storage, API routes)
/// complete.  A successful health-checked connection means the daemon is
/// fully ready.  This works identically on Linux, macOS, and WSL.
///
/// On timeout, [`get_daemon_failure_context`] auto-surfaces the error from
/// the OS service manager (systemctl/journalctl on Linux, the stderr log on
/// macOS) so the user sees why it failed without running a separate command.
async fn wait_for_daemon_ready(daemon_name: &str, timeout_secs: u64) -> Result<()> {
    let poll_interval = std::time::Duration::from_millis(500);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);

    #[cfg(unix)]
    {
        let socket_path = zlayer_client::default_socket_path();
        loop {
            let reachability = zlayer_client::DaemonClient::probe(&socket_path).await;
            match reachability {
                zlayer_client::DaemonReachability::Reachable(_) => return Ok(()),
                zlayer_client::DaemonReachability::PermissionDenied => {
                    // Daemon IS up — we just can't talk to it from this user's shell.
                    // Don't burn the whole timeout; surface the right hint and return Ok.
                    eprintln!();
                    eprintln!(
                        "Daemon is running, but its socket at {socket_path} is not readable from your user."
                    );
                    eprintln!(
                        "  The 'zlayer' group was added during install. Pick up the new group"
                    );
                    eprintln!("  membership in the current shell with:");
                    eprintln!("    newgrp zlayer");
                    eprintln!("  or open a new login shell.");
                    eprintln!();
                    return Ok(());
                }
                _ if tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(poll_interval).await;
                }
                _ => break,
            }
        }
    }

    #[cfg(windows)]
    {
        loop {
            match zlayer_client::DaemonClient::try_connect().await {
                Ok(Some(_)) => return Ok(()),
                _ if tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(poll_interval).await;
                }
                _ => break,
            }
        }
    }

    let error_context = get_daemon_failure_context(daemon_name);
    bail!("Daemon failed to start within {timeout_secs}s.\n{error_context}");
}

/// Auto-surface error context from the OS service manager when the daemon
/// fails to start.  On Linux, pulls from `systemctl status` and journalctl.
/// On macOS, reads the stderr log from the launchd plist's `StandardErrorPath`.
#[allow(unused_variables)]
fn get_daemon_failure_context(daemon_name: &str) -> String {
    let mut context = String::new();

    #[cfg(target_os = "linux")]
    {
        // systemctl status includes exit code and the last few journal lines
        let unit_file_name = unit_name(daemon_name);
        let args = systemctl_args(&["status", unit_file_name.as_str()]);
        if let Ok(out) = std::process::Command::new("systemctl").args(&args).output() {
            let status = String::from_utf8_lossy(&out.stdout);
            if !status.trim().is_empty() {
                context.push_str(&status);
                context.push('\n');
            }
        }
        // If status output is sparse, pull more lines from the journal
        if context.len() < 100 {
            let jctl_args = vec![
                "--no-pager",
                "-n",
                "30",
                "--since=-2min",
                "-u",
                daemon_name,
                "--output",
                "cat",
            ];
            if let Ok(out) = std::process::Command::new("journalctl")
                .args(&jctl_args)
                .output()
            {
                let journal = String::from_utf8_lossy(&out.stdout);
                if !journal.trim().is_empty() {
                    context.push_str(&journal);
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        // Read the stderr log from the plist's StandardErrorPath
        let log_dir =
            crate::cli::default_log_dir(std::path::Path::new(&crate::cli::default_data_dir()));
        let log_path = log_dir.join("daemon.log");
        if let Ok(content) = std::fs::read_to_string(&log_path) {
            let lines: Vec<&str> = content.lines().collect();
            let start = lines.len().saturating_sub(20);
            context = lines[start..].join("\n");
        }
        if context.trim().is_empty() {
            context = format!(
                "No log output found. Check: {}/daemon.log",
                log_dir.display()
            );
        }
    }

    if context.trim().is_empty() {
        context = "No log output found.".to_string();
    }

    context
}

// ---------------------------------------------------------------------------
// Reset (shared across platforms)
// ---------------------------------------------------------------------------

fn reset(data_dir: &Path, force: bool) -> Result<()> {
    if !force {
        eprintln!(
            "This will wipe Raft storage and node identity in {}.",
            data_dir.display()
        );
        eprintln!("The daemon will reinitialise on next start.");
        eprintln!("Run with --force to skip this prompt.");
        bail!("Aborted. Pass --force to confirm.");
    }

    let dirs_to_remove = ["raft", "raft-log", "raft-sm"];
    for name in &dirs_to_remove {
        let p = data_dir.join(name);
        if p.exists() {
            std::fs::remove_dir_all(&p)
                .with_context(|| format!("Failed to remove {}", p.display()))?;
            info!("Removed {}", p.display());
        }
    }

    let files_to_remove = ["raft.db", "node_config.json"];
    for name in &files_to_remove {
        let p = data_dir.join(name);
        if p.exists() {
            std::fs::remove_file(&p)
                .with_context(|| format!("Failed to remove {}", p.display()))?;
            info!("Removed {}", p.display());
        }
    }

    println!("Daemon state reset. Start the daemon to reinitialize.");
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(all(test, target_os = "linux"))]
mod linux_uninstall_tests {
    use super::*;

    #[test]
    fn uninstall_remove_binary_path_default_name() {
        let p = pick_system_binary_path("zlayer");
        assert!(p.ends_with("zlayer"));
    }

    #[test]
    fn uninstall_remove_binary_path_dev_name() {
        let p = pick_system_binary_path("zlayer-dev");
        assert!(p.ends_with("zlayer-dev"));
    }
}

#[cfg(all(test, target_os = "windows"))]
mod windows_tests {
    use super::*;
    use std::ffi::OsStr;
    use std::path::PathBuf;

    /// `build_service_launch_arguments` must emit `--data-dir <path>` BEFORE
    /// the `serve` subcommand — `--data-dir` is a top-level Cli arg and the
    /// service would fail to start if the order slipped.
    #[test]
    fn launch_arguments_puts_data_dir_before_serve() {
        let data_dir = PathBuf::from(r"C:\ProgramData\zlayer");
        let args = build_service_launch_arguments(
            "zlayer",
            &data_dir,
            "0.0.0.0:3669",
            None,
            None,
            false,
            #[cfg(feature = "docker-compat")]
            false,
            TunnelInstallArgs::default(),
        );
        let data_dir_pos = args
            .iter()
            .position(|a| a == OsStr::new("--data-dir"))
            .expect("--data-dir flag present");
        let serve_pos = args
            .iter()
            .position(|a| a == OsStr::new("serve"))
            .expect("serve subcommand present");
        assert!(
            data_dir_pos < serve_pos,
            "--data-dir must come before `serve` (got args: {args:?})"
        );
        // `--data-dir` must be immediately followed by its value.
        assert_eq!(
            args[data_dir_pos + 1].as_os_str(),
            data_dir.as_os_str(),
            "--data-dir value mismatch"
        );
    }

    /// `--service` must be present so SCM spawn enters the Windows Service
    /// dispatcher (I-1) rather than the foreground `serve` path.
    #[test]
    fn launch_arguments_includes_service_flag() {
        let args = build_service_launch_arguments(
            "zlayer",
            Path::new(r"C:\data"),
            "127.0.0.1:3669",
            None,
            None,
            false,
            #[cfg(feature = "docker-compat")]
            false,
            TunnelInstallArgs::default(),
        );
        assert!(
            args.iter().any(|a| a == OsStr::new("--service")),
            "--service flag must be present (got args: {args:?})"
        );
    }

    /// `--bind <addr>` must be forwarded verbatim.
    #[test]
    fn launch_arguments_forwards_bind() {
        let args = build_service_launch_arguments(
            "zlayer",
            Path::new(r"C:\data"),
            "10.0.0.5:4242",
            None,
            None,
            false,
            #[cfg(feature = "docker-compat")]
            false,
            TunnelInstallArgs::default(),
        );
        let bind_pos = args
            .iter()
            .position(|a| a == OsStr::new("--bind"))
            .expect("--bind flag present");
        assert_eq!(args[bind_pos + 1].as_os_str(), OsStr::new("10.0.0.5:4242"));
    }

    /// `--no-swagger` is only passed through when requested.
    #[test]
    fn launch_arguments_no_swagger_toggle() {
        let without = build_service_launch_arguments(
            "zlayer",
            Path::new(r"C:\data"),
            "127.0.0.1:3669",
            None,
            None,
            false,
            #[cfg(feature = "docker-compat")]
            false,
            TunnelInstallArgs::default(),
        );
        assert!(!without.iter().any(|a| a == OsStr::new("--no-swagger")));

        let with = build_service_launch_arguments(
            "zlayer",
            Path::new(r"C:\data"),
            "127.0.0.1:3669",
            None,
            None,
            true,
            #[cfg(feature = "docker-compat")]
            false,
            TunnelInstallArgs::default(),
        );
        assert!(with.iter().any(|a| a == OsStr::new("--no-swagger")));
    }

    /// `--jwt-secret <value>` is forwarded verbatim when provided, and
    /// absent otherwise. SCM does not inherit env from the CLI, so the
    /// secret has to ride the command line.
    #[test]
    fn launch_arguments_jwt_secret_round_trip() {
        let without = build_service_launch_arguments(
            "zlayer",
            Path::new(r"C:\data"),
            "127.0.0.1:3669",
            None,
            None,
            false,
            #[cfg(feature = "docker-compat")]
            false,
            TunnelInstallArgs::default(),
        );
        assert!(!without.iter().any(|a| a == OsStr::new("--jwt-secret")));

        let with = build_service_launch_arguments(
            "zlayer",
            Path::new(r"C:\data"),
            "127.0.0.1:3669",
            None,
            Some("super-secret-token"),
            false,
            #[cfg(feature = "docker-compat")]
            false,
            TunnelInstallArgs::default(),
        );
        let pos = with
            .iter()
            .position(|a| a == OsStr::new("--jwt-secret"))
            .expect("--jwt-secret flag present");
        assert_eq!(with[pos + 1].as_os_str(), OsStr::new("super-secret-token"));
    }

    /// `is_service_not_found` must identify `ERROR_SERVICE_DOES_NOT_EXIST`
    /// so the uninstall/stop/status fallbacks fire instead of erroring out.
    #[test]
    fn is_service_not_found_matches_1060() {
        let io_err = std::io::Error::from_raw_os_error(ERROR_SERVICE_DOES_NOT_EXIST);
        let err = windows_service::Error::Winapi(io_err);
        assert!(is_service_not_found(&err));
    }

    /// Any other Win32 error must NOT be misread as "service not found".
    #[test]
    fn is_service_not_found_rejects_other_errors() {
        // ERROR_ACCESS_DENIED (5) is the other common SCM error and must
        // propagate rather than be swallowed as a no-op.
        let io_err = std::io::Error::from_raw_os_error(5);
        let err = windows_service::Error::Winapi(io_err);
        assert!(!is_service_not_found(&err));
    }
}

#[cfg(all(test, target_os = "macos"))]
mod macos_label_tests {
    use super::*;

    #[test]
    fn plist_label_default_is_com_zlayer_daemon() {
        assert_eq!(plist_label("zlayer"), "com.zlayer.daemon");
    }

    #[test]
    fn plist_label_dev_becomes_daemon_dev() {
        assert_eq!(plist_label("zlayer-dev"), "com.zlayer.daemon-dev");
    }

    #[test]
    fn plist_label_arbitrary_name() {
        assert_eq!(plist_label("foo"), "com.zlayer.daemon-foo");
    }
}
