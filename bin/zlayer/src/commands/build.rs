use anyhow::{Context, Result};
use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::Arc;
use tracing::{info, warn};

/// Build a container image from a Dockerfile or runtime template
#[allow(
    clippy::too_many_arguments,
    clippy::fn_params_excessive_bools,
    clippy::too_many_lines
)]
pub(crate) async fn handle_build(
    context: PathBuf,
    file: Option<PathBuf>,
    zimagefile: Option<PathBuf>,
    tags: Vec<String>,
    runtime: Option<String>,
    runtime_auto: bool,
    build_args: Vec<String>,
    target: Option<String>,
    no_cache: bool,
    pull: String,
    no_pull: bool,
    push: bool,
    no_tui: bool,
    verbose_build: bool,
    platform: Option<String>,
    update_bottles: bool,
    backend: Option<String>,
    host_network: bool,
) -> Result<()> {
    use std::str::FromStr;
    use zlayer_builder::{
        detect_runtime, BuildEvent, ImageBuilder, ImageOs, PlainLogger, PullBaseMode, Runtime,
    };
    use zlayer_types::builder::BuilderBackendKind;

    // Parse the optional --backend override into a typed discriminator. We
    // keep clap's value as `Option<String>` for clean error formatting and
    // do the typed parse here so callers get an `anyhow` error with the same
    // remediation hint regardless of which front-end fed the build (CLI vs
    // pipeline vs daemon RPC).
    let backend_override: Option<BuilderBackendKind> = match backend.as_deref() {
        Some(b) => Some(BuilderBackendKind::from_str(b).map_err(|e| {
            anyhow::anyhow!(
                "invalid --backend value '{b}': {e} (expected one of: \
                 buildah-cli, buildah-sidecar, sandbox, hcs)"
            )
        })?),
        None => None,
    };

    // L-2: Parse the CLI `--platform` flag into an `ImageOs` pin. When set,
    // this takes precedence over any OS inferred from the ZImagefile. When
    // unset, `ImageBuilder` will peek at the ZImagefile's `os:`/`platform:`
    // fields during `build()` and fall back to Linux if nothing hints
    // otherwise.
    let cli_target_os = match platform.as_deref() {
        Some(p) => Some(ImageOs::from_str(p).with_context(|| {
            format!("invalid --platform value '{p}' (expected linux[/arch] or windows[/arch])")
        })?),
        None => None,
    };

    // Resolve pull mode: --no-pull wins as an explicit override
    let pull_mode = if no_pull {
        PullBaseMode::Never
    } else {
        match pull.as_str() {
            "always" => PullBaseMode::Always,
            "never" => PullBaseMode::Never,
            _ => PullBaseMode::Newer,
        }
    };

    info!(
        context = %context.display(),
        tags = ?tags,
        runtime = ?runtime,
        runtime_auto = runtime_auto,
        "Starting build"
    );

    // Resolve runtime
    let resolved_runtime = if runtime_auto {
        info!("Auto-detecting runtime from project files");
        detect_runtime(&context)
    } else if let Some(name) = runtime {
        if let Some(rt) = Runtime::from_name(&name) {
            info!(runtime = %rt, "Using specified runtime template");
            Some(rt)
        } else {
            // List available runtimes in error message
            let available: Vec<_> = Runtime::all().iter().map(|r| r.name).collect();
            anyhow::bail!(
                "Unknown runtime: '{}'. Available runtimes: {}",
                name,
                available.join(", ")
            );
        }
    } else {
        None
    };

    // Parse build args
    let build_args_map: HashMap<String, String> = build_args
        .iter()
        .filter_map(|arg| {
            let parts: Vec<&str> = arg.splitn(2, '=').collect();
            if parts.len() == 2 {
                Some((parts[0].to_string(), parts[1].to_string()))
            } else {
                warn!(arg = %arg, "Invalid build-arg format, expected KEY=VALUE");
                eprintln!("Warning: invalid build-arg '{arg}', expected KEY=VALUE");
                None
            }
        })
        .collect();

    // ------------------------------------------------------------------
    // macOS Linux-build routing: bring up the `zlayer-buildd` VZ sidecar,
    // stage the build context into its shared mount, and wire the sidecar
    // backend via env. The image lands in the buildd's in-guest storage;
    // we export + import it into ZLayer's local registry after the build
    // (the library's own local-registry import path can't reach the
    // in-guest store, so we skip it here and do it ourselves).
    // ------------------------------------------------------------------
    // `context` is only mutated on the macOS sidecar path below
    // (`setup_macos_sidecar(&mut context)`); on every other target the binding
    // is read-only, so silence the platform-specific `unused_mut`.
    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
    let mut context = context;
    #[cfg(target_os = "macos")]
    let mac_sidecar = {
        let target_is_linux =
            matches!(cli_target_os, None | Some(ImageOs::Linux));
        if target_is_linux {
            Some(setup_macos_sidecar(&mut context).await?)
        } else {
            None
        }
    };

    // Create event channel for progress updates
    let (event_tx, event_rx) = mpsc::channel::<BuildEvent>();

    // Build the ImageBuilder. Pass the CLI-supplied target OS (if any) so the
    // backend is detected for the correct OS up front — avoids a throw-away
    // Linux-backend probe when the user asked for Windows.
    let mut builder = ImageBuilder::new_with_os(&context, cli_target_os)
        .await
        .context("Failed to create image builder")?
        .with_events(event_tx);

    // Wire up local registry + blob cache so built images are stored locally.
    // A Permission denied here is fatal: it means the daemon's data_dir is not
    // writable by this UID, which will produce a "successful" build that the
    // daemon can't resolve — the image never lands in the local registry and
    // the runtime falls through to docker.io/library/* with a cryptic 401.
    // Better to fail loud with a remediation hint than silently corrupt the
    // deploy path. Other errors (corrupt DB, disk full) stay warn-and-continue.
    let data_dir = zlayer_paths::ZLayerDirs::detect_data_dir();

    let cache_path = data_dir.join("cache").join("blobs.redb");
    match zlayer_registry::PersistentBlobCache::open(&cache_path).await {
        Ok(cache) => {
            let backend: Arc<Box<dyn zlayer_registry::cache::BlobCacheBackend>> =
                Arc::new(Box::new(cache));
            builder = builder.with_cache_backend(backend);
        }
        Err(e) => {
            if looks_like_permission_denied(&e.to_string()) && is_system_data_dir(&data_dir) {
                anyhow::bail!(
                    "cannot write to {}: Permission denied.\n\
                     The build data directory is owned by another user or needs re-provisioning.\n\
                     Fix: sudo zlayer daemon install   (re-runs ownership setup)\n\
                     Verify with: ls -ld {}",
                    cache_path.display(),
                    data_dir.display(),
                );
            }
            warn!(
                "Failed to open persistent blob cache, builds will not populate local cache: {e}"
            );
        }
    }

    // On the macOS sidecar path the built image lives in the in-guest
    // buildd store, not a host buildah store — the library's local-registry
    // import (host `buildah push`) would fail. Skip it; we export + import
    // from the shared mount ourselves after the build.
    #[cfg(target_os = "macos")]
    let wire_local_registry = mac_sidecar.is_none();
    #[cfg(not(target_os = "macos"))]
    let wire_local_registry = true;

    let registry_path = data_dir.join("registry");
    match zlayer_registry::LocalRegistry::new(registry_path.clone()).await {
        Ok(registry) => {
            if wire_local_registry {
                builder = builder.with_local_registry(registry);
            }
        }
        Err(e) => {
            if looks_like_permission_denied(&e.to_string()) && is_system_data_dir(&data_dir) {
                anyhow::bail!(
                    "cannot write to {}: Permission denied.\n\
                     The build data directory is owned by another user or needs re-provisioning.\n\
                     Fix: sudo zlayer daemon install   (re-runs ownership setup)\n\
                     Verify with: ls -ld {}",
                    registry_path.display(),
                    data_dir.display(),
                );
            }
            warn!("Failed to open local registry, built images will not be stored locally: {e}");
        }
    }

    // Apply Dockerfile path if specified
    if let Some(dockerfile) = file {
        builder = builder.dockerfile(dockerfile);
    }

    // Apply ZImagefile path if specified
    if let Some(zimage) = zimagefile {
        builder = builder.zimagefile(zimage);
    }

    // Apply runtime template if resolved
    if let Some(rt) = resolved_runtime {
        builder = builder.runtime(rt);
    }

    // Apply tags
    for tag in &tags {
        builder = builder.tag(tag);
    }

    // Apply build args
    builder = builder.build_args(build_args_map);

    // Apply target stage
    if let Some(t) = target {
        builder = builder.target(t);
    }

    // Apply no-cache
    if no_cache {
        builder = builder.no_cache();
    }

    // Apply pull strategy (default Newer is a no-op but harmless to set explicitly)
    builder = builder.pull(pull_mode);

    // Forward --update-bottles to the macOS sandbox backend (no-op on Linux/Windows)
    builder = builder.update_bottles(update_bottles);

    // Forward --host-network to the builder so every `buildah run` gets
    // `--net=host`. This matches Docker's `--network host` build mode and
    // bypasses buildah's CNI / netavark plumbing entirely.
    builder = builder.with_host_network(host_network);

    // Forward --backend (if supplied). `None` keeps the auto-detected
    // default; `Some(kind)` will force that backend at dispatch time. The
    // value is plumbed onto `BuildOptions::backend_override`; the
    // `detect_backend()` path will honour it in a follow-up (task 4.1).
    builder = builder.with_backend_override(backend_override);

    // Apply push
    if push {
        builder = builder.push_without_auth();
    }

    // Determine if we should use TUI or plain output
    let use_tui = !no_tui && std::io::stdout().is_terminal();

    let result = if use_tui {
        // TUI mode - run build with interactive progress display
        use zlayer_builder::BuildTui;

        // Spawn build in background
        let build_handle = tokio::spawn(async move { builder.build().await });

        // Run TUI (blocking on the current thread)
        // We need to spawn a blocking task for this
        let tui_result = tokio::task::spawn_blocking(move || {
            let mut tui = BuildTui::new(event_rx);
            tui.run()
        })
        .await
        .context("TUI task panicked")?;

        if let Err(e) = tui_result {
            warn!(error = %e, "TUI error");
        }

        // Wait for build result
        build_handle
            .await
            .context("Build task panicked")?
            .context("Build failed")?
    } else {
        // Plain output mode (CI or --no-tui)
        let logger = PlainLogger::new(verbose_build);

        // Spawn build in background
        let build_handle = tokio::spawn(async move { builder.build().await });

        // Process events in the main thread until build completes
        while let Ok(event) = event_rx.recv() {
            let is_terminal = matches!(
                event,
                BuildEvent::BuildComplete { .. } | BuildEvent::BuildFailed { .. }
            );
            logger.handle_event(&event);
            if is_terminal {
                break;
            }
        }

        // Wait for build result
        build_handle
            .await
            .context("Build task panicked")?
            .context("Build failed")?
    };

    println!("\nBuilt image: {}", result.image_id);
    for tag in &result.tags {
        println!("  Tagged: {tag}");
    }
    println!("Build time: {}ms", result.build_time_ms);

    // macOS sidecar path: the built image is in the in-guest buildd store.
    // Export it to an OCI archive in the shared mount and import it into
    // ZLayer's local registry so `zlayer run <tag>` can resolve it. Then
    // clean up the staged context.
    #[cfg(target_os = "macos")]
    if let Some(sidecar) = mac_sidecar {
        finalize_macos_sidecar(&sidecar, &result, &data_dir).await?;
    }

    Ok(())
}

/// State carried across the macOS sidecar build: the live buildd handle plus
/// the staged-context dir we must clean up afterward.
#[cfg(target_os = "macos")]
struct MacSidecar {
    handle: crate::commands::buildd_manager::BuilddHandle,
    staged_host_dir: PathBuf,
}

/// Bring up the `zlayer-buildd` VZ sidecar, stage `*context` into its shared
/// mount, repoint `*context` at the staged dir, and export the sidecar
/// wiring via env vars so `detect_backend` selects the remote sidecar.
///
/// Returns the [`MacSidecar`] state for post-build export + cleanup.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
async fn setup_macos_sidecar(context: &mut PathBuf) -> Result<MacSidecar> {
    use crate::commands::buildd_manager;

    info!("macOS Linux build: routing through zlayer-buildd VZ sidecar");
    let handle = buildd_manager::ensure_buildd()
        .await
        .context("ensuring zlayer-buildd sidecar is running")?;

    // Drop leftover vfs layers from any prior build so the RAM-backed graph
    // doesn't run out of space on a reused sidecar.
    buildd_manager::prune_build_storage(&handle).await;

    // Stage the real context into the shared mount; the in-guest buildah
    // reads it at the returned guest path.
    let canonical = std::fs::canonicalize(&context).unwrap_or_else(|_| context.clone());
    let (staged_host_dir, _guest_dir) = buildd_manager::stage_context(&handle, &canonical)
        .context("staging build context into sidecar shared mount")?;

    // The library reads the Dockerfile from this (host) path AND hands it to
    // the backend as `context_dir`; the backend then rewrites the host
    // prefix to the guest prefix via `context_mount` (set in env below).
    context.clone_from(&staged_host_dir);

    // Wire the sidecar backend via env (read by detect_backend on macOS).
    let (host_prefix, guest_prefix) = handle.context_mount();
    // SAFETY: single-threaded setup phase before any build task is spawned.
    unsafe {
        std::env::set_var("ZLAYER_BUILDD_ADDR", &handle.addr);
        std::env::set_var("ZLAYER_BUILDD_TLS_DIR", &handle.tls_dir);
        std::env::set_var(
            "ZLAYER_BUILDD_CONTEXT_MOUNT",
            format!("{}:{}", host_prefix.display(), guest_prefix.display()),
        );
    }

    Ok(MacSidecar {
        handle,
        staged_host_dir,
    })
}

/// Export the freshly-built image out of the in-guest buildd store into an
/// OCI archive in the shared mount, import it into `ZLayer`'s local registry,
/// then clean up the staged context + export tar.
#[cfg(target_os = "macos")]
async fn finalize_macos_sidecar(
    sidecar: &MacSidecar,
    result: &zlayer_builder::BuiltImage,
    data_dir: &std::path::Path,
) -> Result<()> {
    use crate::commands::buildd_manager;

    if result.tags.is_empty() {
        warn!("sidecar build produced no tags; skipping import into local registry");
        buildd_manager::cleanup_staged(&sidecar.staged_host_dir);
        return Ok(());
    }

    let registry_path = data_dir.join("registry");
    let registry = zlayer_registry::LocalRegistry::new(registry_path.clone())
        .await
        .with_context(|| {
            format!("opening local registry at {}", registry_path.display())
        })?;
    let cache_path = data_dir.join("cache").join("blobs.redb");
    let blob_cache = zlayer_registry::PersistentBlobCache::open(&cache_path)
        .await
        .with_context(|| format!("opening blob cache at {}", cache_path.display()))?;

    // Export once using the first tag; import that tar under every tag.
    let primary = &result.tags[0];
    println!("Exporting {primary} from sidecar store...");
    let tar = buildd_manager::export_image(&sidecar.handle, primary)
        .await
        .context("exporting built image from sidecar")?;

    for tag in &result.tags {
        let info = zlayer_registry::import_image(&registry, &tar, Some(tag.as_str()), Some(&blob_cache))
            .await
            .with_context(|| format!("importing '{tag}' into local registry"))?;
        println!("  Imported {tag} (digest {})", info.digest);
    }

    // Best-effort cleanup of the export tar + staged context.
    if let Err(e) = std::fs::remove_file(&tar) {
        warn!(path = %tar.display(), "failed to remove export tar: {e}");
    }
    buildd_manager::cleanup_staged(&sidecar.staged_host_dir);

    Ok(())
}

/// List available runtime templates
#[allow(clippy::unnecessary_wraps)]
pub(crate) fn handle_runtimes() -> Result<()> {
    use zlayer_builder::{list_templates, Runtime};

    println!("Available runtime templates:\n");

    for info in list_templates() {
        println!("  {:12} - {}", info.name, info.description);
        println!("               Detects: {}", info.detect_files.join(", "));
        println!();
    }

    println!("Usage:");
    println!("  zlayer build --runtime node20 .");
    println!("  zlayer build --runtime-auto .   # auto-detect from project files");
    println!();
    println!(
        "All runtimes: {}",
        Runtime::all()
            .iter()
            .map(|r| r.name)
            .collect::<Vec<_>>()
            .join(", ")
    );

    Ok(())
}

/// Match EACCES messages across redb, `std::io`, and our local-registry wrapper.
/// redb surfaces "readonly database" on EROFS-ish opens; `std::io::Error`
/// formats `PermissionDenied` as "Permission denied".
fn looks_like_permission_denied(msg: &str) -> bool {
    let lower = msg.to_ascii_lowercase();
    lower.contains("permission denied") || lower.contains("readonly database")
}

/// True if `data_dir` is a system-owned install (rather than a user-local one
/// like `~/.zlayer`). We only auto-elevate EACCES to a hard error in the
/// system case — a user's own `~/.zlayer` being unreadable is weirder and
/// usually deserves the raw error.
fn is_system_data_dir(data_dir: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        data_dir.starts_with("/var/lib/") || data_dir.starts_with("/usr/")
    }
    #[cfg(not(unix))]
    {
        let _ = data_dir;
        false
    }
}
