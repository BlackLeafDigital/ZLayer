use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use tracing::info;

use crate::cli::ManagerCommands;

/// Handle zlayer-manager commands
pub(crate) async fn handle_manager(cmd: &ManagerCommands) -> Result<()> {
    match cmd {
        ManagerCommands::Init {
            output,
            port,
            deploy,
            version,
            email,
            password,
            password_file,
            random,
            no_prompt,
            env_file,
            no_bootstrap,
            force,
        } => {
            handle_manager_init(
                output.clone(),
                *port,
                *deploy,
                version.clone(),
                email.clone(),
                password.clone(),
                password_file.clone(),
                *random,
                *no_prompt,
                env_file.clone(),
                *no_bootstrap,
                *force,
            )
            .await
        }
        ManagerCommands::Status => handle_manager_status(),
        ManagerCommands::Stop { force } => handle_manager_stop(*force),
    }
}

/// Initialize zlayer-manager deployment spec.
///
/// Depending on the flag combination, this either:
///
/// 1. Emits a legacy spec with commented-out bootstrap env examples
///    (`--no-bootstrap`, or `--no-prompt` with no password source),
/// 2. Emits a spec pointing at a user-supplied env file
///    (`--env-file <path>`), or
/// 3. Resolves an admin email + password, stores the password in the
///    daemon's `bootstrap` environment via the secrets API, and emits a
///    spec that references `$secret://bootstrap/ZLAYER_BOOTSTRAP_PASSWORD`.
#[allow(
    clippy::too_many_arguments,
    clippy::fn_params_excessive_bools,
    clippy::needless_pass_by_value
)]
pub(crate) async fn handle_manager_init(
    output: PathBuf,
    port: u16,
    deploy: bool,
    version: Option<String>,
    email: Option<String>,
    password: Option<String>,
    password_file: Option<PathBuf>,
    random: bool,
    no_prompt: bool,
    env_file: Option<PathBuf>,
    no_bootstrap: bool,
    force: bool,
) -> Result<()> {
    // --env-file and --no-bootstrap are mutually exclusive: the former wires
    // an active reference into the spec, the latter leaves the spec silent.
    if env_file.is_some() && no_bootstrap {
        bail!("--env-file and --no-bootstrap are mutually exclusive");
    }

    let version = version.unwrap_or_else(|| "latest".to_string());
    info!(port = port, version = %version, "Initializing zlayer-manager deployment");

    // Branch A: --no-bootstrap, or --no-prompt with nothing to do — emit the
    // legacy commented spec for back-compat.
    if no_bootstrap
        || (no_prompt
            && password.is_none()
            && password_file.is_none()
            && !random
            && env_file.is_none())
    {
        return emit_legacy_spec(&output, port, &version, deploy);
    }

    // Branch B: --env-file — spec references the user's file, no daemon calls.
    if let Some(path) = env_file {
        return emit_env_file_spec(&output, port, &version, deploy, &path);
    }

    // Branch C: full integration — resolve email + password, write to the
    // `bootstrap` env via the secrets API, emit a spec with a `$secret://` ref.
    // Delegated to a platform-gated helper: the Unix version talks to the
    // daemon, the Windows version bails with a WSL2 hint.
    integrated_bootstrap_init(
        &output,
        port,
        &version,
        deploy,
        email,
        password,
        password_file,
        random,
        no_prompt,
        force,
    )
    .await
}

// ---------------------------------------------------------------------------
// Branch C: integrated bootstrap init (Unix-only; Windows bails)
// ---------------------------------------------------------------------------

/// Full integration path for `zlayer manager init`: resolve an admin email +
/// password, stash the password in the daemon's `bootstrap` environment via
/// the secrets API, and emit a spec that references
/// `$secret://bootstrap/ZLAYER_BOOTSTRAP_PASSWORD`.
///
/// This requires the daemon, which only runs on Unix. The Windows build
/// exposes a matching signature that bails with a WSL2 hint so the rest of
/// the `handle_manager` dispatcher remains cross-platform.
#[cfg(unix)]
#[allow(
    clippy::too_many_arguments,
    clippy::fn_params_excessive_bools,
    clippy::needless_pass_by_value
)]
async fn integrated_bootstrap_init(
    output: &Path,
    port: u16,
    version: &str,
    deploy: bool,
    email: Option<String>,
    password: Option<String>,
    password_file: Option<PathBuf>,
    random: bool,
    no_prompt: bool,
    force: bool,
) -> Result<()> {
    let email = if let Some(e) = email {
        validate_email(&e)?;
        e
    } else {
        if no_prompt {
            bail!("--email is required when --no-prompt is set");
        }
        prompt_email_interactive()?
    };

    // Connect to the daemon and make sure the `bootstrap` env exists.
    let client = zlayer_client::DaemonClient::connect().await.context(
        "Failed to connect to the daemon — is it running? Start with `zlayer serve --daemon`",
    )?;

    let env_id = ensure_bootstrap_env(&client).await?;

    // Reuse-or-overwrite policy: if the password is already stored AND the
    // user did not pass an explicit password source AND --force is absent,
    // keep the existing password and just rewrite the spec.
    let explicit_pw_source = password.is_some() || password_file.is_some() || random;
    let already_stored = bootstrap_password_exists(&client, &env_id).await?;

    if already_stored && !explicit_pw_source && !force {
        println!("Bootstrap password already set in the `bootstrap` environment; reusing.");
        println!("  Key: ZLAYER_BOOTSTRAP_PASSWORD");
        println!(
            "  (pass --force to rotate, or --password / --password-file / --random to overwrite.)"
        );

        emit_integrated_spec(output, port, version, deploy, &email)?;
        let spec_path = output.join("manager.zlayer.yml");
        println!();
        println!("Admin email: {email}");
        if deploy {
            println!();
            println!("Deploying zlayer-manager...");
            println!();
            println!("To deploy manually, run:");
            println!("  zlayer deploy {}", spec_path.display());
        } else {
            println!();
            println!("To deploy: zlayer deploy {}", spec_path.display());
        }
        return Ok(());
    }

    let (password_plaintext, was_random) =
        resolve_password(password, password_file, random, no_prompt)?;

    client
        .set_secret_in_env(&env_id, "ZLAYER_BOOTSTRAP_PASSWORD", &password_plaintext)
        .await
        .context("Failed to store bootstrap password in the secrets store")?;

    emit_integrated_spec(output, port, version, deploy, &email)?;

    let spec_path = output.join("manager.zlayer.yml");

    println!();
    if already_stored {
        println!("Rotated admin password in the `bootstrap` environment.");
    } else {
        println!("Stored admin password in the `bootstrap` environment.");
    }
    println!("  Key: ZLAYER_BOOTSTRAP_PASSWORD");
    println!();
    if was_random {
        println!("  Generated password (shown ONCE — save it now):");
        println!("    {password_plaintext}");
        println!();
        println!("  Store this somewhere safe. It will NOT be shown again.");
        println!();
    }
    println!("Admin email: {email}");

    if deploy {
        println!();
        println!("Deploying zlayer-manager...");
        println!();
        println!("To deploy manually, run:");
        println!("  zlayer deploy {}", spec_path.display());
    } else {
        println!();
        println!("To deploy: zlayer deploy {}", spec_path.display());
    }

    Ok(())
}

/// Return true iff `ZLAYER_BOOTSTRAP_PASSWORD` already exists in the given
/// environment. Uses the metadata-only `list_secrets_in_env` endpoint —
/// reading the value would require admin auth and is unnecessary here.
#[cfg(unix)]
async fn bootstrap_password_exists(
    client: &zlayer_client::DaemonClient,
    env_id: &str,
) -> Result<bool> {
    let secrets = client
        .list_secrets_in_env(env_id)
        .await
        .context("Failed to list secrets in bootstrap environment")?;
    Ok(secrets
        .iter()
        .any(|s| s.name == "ZLAYER_BOOTSTRAP_PASSWORD"))
}

/// Windows stub for [`integrated_bootstrap_init`]: the daemon is Unix-only,
/// so we can't resolve `$secret://` refs or talk to the secrets API from the
/// thin Windows CLI. Point the user at the two cross-platform branches or at
/// WSL2.
#[cfg(not(unix))]
#[allow(
    clippy::too_many_arguments,
    clippy::fn_params_excessive_bools,
    clippy::needless_pass_by_value,
    clippy::unused_async
)]
async fn integrated_bootstrap_init(
    _output: &Path,
    _port: u16,
    _version: &str,
    _deploy: bool,
    _email: Option<String>,
    _password: Option<String>,
    _password_file: Option<PathBuf>,
    _random: bool,
    _no_prompt: bool,
    _force: bool,
) -> Result<()> {
    bail!(
        "`zlayer manager init` with bootstrap password integration requires \
         the daemon, which only runs on Unix.\n\
         On Windows, use `--no-bootstrap` or `--env-file`, or run `zlayer serve` inside WSL2 \
         and use the daemon API."
    )
}

// ---------------------------------------------------------------------------
// Helpers: email + password resolution (Unix-only; consumed by
// `integrated_bootstrap_init`'s Unix arm)
// ---------------------------------------------------------------------------

#[cfg(unix)]
fn validate_email(email: &str) -> Result<()> {
    let trimmed = email.trim();
    if trimmed.is_empty() || !trimmed.contains('@') {
        bail!("Invalid email: {email}");
    }
    Ok(())
}

#[cfg(unix)]
fn prompt_email_interactive() -> Result<String> {
    use std::io::{stdin, stdout, Write};
    print!("Admin email: ");
    stdout().flush().ok();
    let mut buf = String::new();
    stdin()
        .read_line(&mut buf)
        .context("Failed to read admin email from stdin")?;
    let e = buf.trim().to_string();
    validate_email(&e)?;
    Ok(e)
}

/// Resolve an admin password from the CLI flags.
///
/// Returns `(plaintext, was_random)`. Exactly one of `--password`,
/// `--password-file`, or `--random` may be set; otherwise the password is
/// read interactively (or we error out when `--no-prompt` is set).
#[cfg(unix)]
fn resolve_password(
    inline: Option<String>,
    file: Option<PathBuf>,
    random: bool,
    no_prompt: bool,
) -> Result<(String, bool)> {
    match (inline, file, random) {
        (Some(v), None, false) => {
            tracing::warn!("--password exposes the password on argv");
            if v.is_empty() {
                bail!("--password cannot be empty");
            }
            Ok((v, false))
        }
        (None, Some(p), false) => {
            let raw = std::fs::read_to_string(&p)
                .with_context(|| format!("Failed to read {}", p.display()))?;
            // Match the stdin convention used elsewhere in the CLI: strip a
            // single trailing newline (with optional CR), but keep any inner
            // whitespace intact.
            let trimmed = raw
                .strip_suffix('\n')
                .map_or_else(|| raw.clone(), std::string::ToString::to_string);
            let trimmed = trimmed
                .strip_suffix('\r')
                .map_or_else(|| trimmed.clone(), std::string::ToString::to_string);
            if trimmed.is_empty() {
                bail!("Password file is empty: {}", p.display());
            }
            Ok((trimmed, false))
        }
        (None, None, true) => {
            use rand::distr::Alphanumeric;
            use rand::Rng;
            let v: String = rand::rng()
                .sample_iter(Alphanumeric)
                .take(32)
                .map(char::from)
                .collect();
            Ok((v, true))
        }
        (None, None, false) => {
            if no_prompt {
                bail!(
                    "No password source given and --no-prompt is set. \
                     Pass --password, --password-file, or --random."
                );
            }
            prompt_password_interactive().map(|v| (v, false))
        }
        _ => bail!("--password, --password-file, --random are mutually exclusive"),
    }
}

#[cfg(unix)]
fn prompt_password_interactive() -> Result<String> {
    // `dialoguer::Password` suppresses echo and handles confirmation for us.
    // The `password` feature is already enabled in bin/zlayer/Cargo.toml.
    use dialoguer::Password;
    let pw = Password::new()
        .with_prompt("Admin password")
        .with_confirmation("Confirm password", "Passwords do not match")
        .interact()
        .context("Failed to read admin password")?;
    if pw.is_empty() {
        bail!("Password cannot be empty");
    }
    Ok(pw)
}

// ---------------------------------------------------------------------------
// Helpers: daemon interaction (Unix-only; `DaemonClient` is gated to Unix)
// ---------------------------------------------------------------------------

/// Look up (or create) the global `bootstrap` environment and return its id.
#[cfg(unix)]
async fn ensure_bootstrap_env(client: &zlayer_client::DaemonClient) -> Result<String> {
    const NAME: &str = "bootstrap";

    let envs = client
        .list_environments(None)
        .await
        .context("Failed to list environments")?;
    if let Some(existing) = envs.iter().find(|e| e.name == NAME) {
        return Ok(existing.id.clone());
    }

    let created = client
        .create_environment(NAME, None, Some("Admin bootstrap credentials"))
        .await
        .context("Failed to create bootstrap environment")?;
    Ok(created.id)
}

// ---------------------------------------------------------------------------
// Helpers: spec emission
// ---------------------------------------------------------------------------

fn write_spec(output: &Path, spec: &str) -> Result<PathBuf> {
    if !output.exists() {
        std::fs::create_dir_all(output)
            .with_context(|| format!("Failed to create output directory: {}", output.display()))?;
    }
    let spec_path = output.join("manager.zlayer.yml");
    std::fs::write(&spec_path, spec)
        .with_context(|| format!("Failed to write spec file: {}", spec_path.display()))?;
    Ok(spec_path)
}

fn print_trailer(spec_path: &Path, version: &str, port: u16, deploy: bool) {
    println!("Created deployment spec: {}", spec_path.display());
    println!();
    println!("Manager configuration:");
    println!("  - Image: zachhandley/zlayer-manager:{version}");
    println!("  - Port: {port}");
    println!();

    if deploy {
        println!("Deploying zlayer-manager...");
        println!();
        println!("To deploy manually, run:");
        println!("  zlayer deploy");
    } else {
        println!("To deploy, run:");
        println!("  zlayer deploy");
        println!();
        println!("Or use --deploy flag to deploy immediately:");
        println!("  zlayer manager init --deploy");
    }
}

/// Legacy spec — a commented-out env block explaining the bootstrap knobs.
/// Preserves pre-rewrite behavior for back-compat with `--no-bootstrap` and
/// `--no-prompt` (with no password source).
fn emit_legacy_spec(output: &Path, port: u16, version: &str, deploy: bool) -> Result<()> {
    let spec = format!(
        r#"version: v1
deployment: zlayer-manager

services:
  manager:
    rtype: service
    image:
      name: zachhandley/zlayer-manager:{version}
      pull_policy: always
    resources:
      cpu: 0.5
      memory: 256Mi
    endpoints:
      - name: http
        protocol: http
        port: {port}
        target_port: 6677
        expose: public
    scale:
      mode: fixed
      replicas: 1
    health:
      check:
        type: http
        url: http://localhost:6677/health
    # Optional: non-interactive admin bootstrap.
    # Read ONLY when the users table is empty on first startup;
    # subsequent restarts ignore these vars. The file-based form
    # (`ZLAYER_BOOTSTRAP_PASSWORD_FILE`) is preferred in production
    # so the password doesn't appear in /proc/<pid>/environ.
    # env:
    #   ZLAYER_BOOTSTRAP_EMAIL: "admin@example.com"
    #   ZLAYER_BOOTSTRAP_PASSWORD: "<long-random-string>"
    #   ZLAYER_BOOTSTRAP_PASSWORD_FILE: "/run/secrets/zlayer_admin_pw"
    #   ZLAYER_BOOTSTRAP_DISPLAY_NAME: "Admin"
"#
    );

    let spec_path = write_spec(output, &spec)?;
    print_trailer(&spec_path, version, port, deploy);
    Ok(())
}

/// Spec that points `ZLAYER_BOOTSTRAP_PASSWORD_FILE` at a user-supplied file.
fn emit_env_file_spec(
    output: &Path,
    port: u16,
    version: &str,
    deploy: bool,
    env_file_path: &Path,
) -> Result<()> {
    let file_str = env_file_path.display().to_string();
    let spec = format!(
        r#"version: v1
deployment: zlayer-manager

services:
  manager:
    rtype: service
    image:
      name: zachhandley/zlayer-manager:{version}
      pull_policy: always
    resources:
      cpu: 0.5
      memory: 256Mi
    endpoints:
      - name: http
        protocol: http
        port: {port}
        target_port: 6677
        expose: public
    scale:
      mode: fixed
      replicas: 1
    health:
      check:
        type: http
        url: http://localhost:6677/health
    # Admin bootstrap: the manager reads the password from the file at
    # `ZLAYER_BOOTSTRAP_PASSWORD_FILE` on first start (users table empty).
    # The path must exist inside the container — mount it as a file secret
    # or bake it into the image, not into the env string.
    env:
      ZLAYER_BOOTSTRAP_PASSWORD_FILE: "{file_str}"
"#
    );

    let spec_path = write_spec(output, &spec)?;
    print_trailer(&spec_path, version, port, deploy);
    println!();
    println!("Bootstrap password file: {file_str}");
    Ok(())
}

/// Spec that references the secret stored in the `bootstrap` environment.
/// The `$secret://` reference is resolved by zlayer-agent at container start.
#[cfg(unix)]
fn emit_integrated_spec(
    output: &Path,
    port: u16,
    version: &str,
    deploy: bool,
    email: &str,
) -> Result<()> {
    let spec = format!(
        r#"version: v1
deployment: zlayer-manager

services:
  manager:
    rtype: service
    image:
      name: zachhandley/zlayer-manager:{version}
      pull_policy: always
    resources:
      cpu: 0.5
      memory: 256Mi
    endpoints:
      - name: http
        protocol: http
        port: {port}
        target_port: 6677
        expose: public
    scale:
      mode: fixed
      replicas: 1
    health:
      check:
        type: http
        url: http://localhost:6677/health
    # Admin bootstrap — read ONLY when the users table is empty on first
    # startup; subsequent restarts ignore these vars.
    #
    # `$secret://<env>/<KEY>` is resolved by zlayer-agent at container start
    # by reading the named secret from the specified environment in the
    # daemon's secrets store.
    env:
      ZLAYER_BOOTSTRAP_EMAIL: "{email}"
      ZLAYER_BOOTSTRAP_PASSWORD: "$secret://bootstrap/ZLAYER_BOOTSTRAP_PASSWORD"
"#
    );

    let spec_path = write_spec(output, &spec)?;
    print_trailer(&spec_path, version, port, deploy);
    Ok(())
}

// ---------------------------------------------------------------------------
// Other manager subcommands (unchanged)
// ---------------------------------------------------------------------------

/// Show manager status
#[allow(clippy::unnecessary_wraps)]
pub(crate) fn handle_manager_status() -> Result<()> {
    info!("Checking zlayer-manager status");

    // TODO: Query the actual manager service status
    // This would check if the manager container is running, its health, etc.
    println!("zlayer-manager status:");
    println!("  Status: unknown (status check not yet implemented)");
    println!();
    println!("To check container status manually:");
    println!("  zlayer ps | grep manager");

    Ok(())
}

/// Stop the manager
#[allow(clippy::unnecessary_wraps)]
pub(crate) fn handle_manager_stop(force: bool) -> Result<()> {
    info!(force = force, "Stopping zlayer-manager");

    // TODO: Actually stop the manager service
    // This would find and stop the manager container
    if force {
        println!("Force stopping zlayer-manager...");
    } else {
        println!("Gracefully stopping zlayer-manager...");
    }

    println!();
    println!("Manager stop not yet implemented.");
    println!("To stop manually:");
    println!("  zlayer stop zlayer-manager");

    Ok(())
}
