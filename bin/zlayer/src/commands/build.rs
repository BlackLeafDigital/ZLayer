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
) -> Result<()> {
    use zlayer_builder::{
        detect_runtime, BuildEvent, ImageBuilder, PlainLogger, PullBaseMode, Runtime,
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

    // Create event channel for progress updates
    let (event_tx, event_rx) = mpsc::channel::<BuildEvent>();

    // Build the ImageBuilder
    let mut builder = ImageBuilder::new(&context)
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

    let registry_path = data_dir.join("registry");
    match zlayer_registry::LocalRegistry::new(registry_path.clone()).await {
        Ok(registry) => {
            builder = builder.with_local_registry(registry);
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

    // Apply push
    if push {
        builder = builder.push_without_auth();
    }

    // Determine if we should use TUI or plain output
    let use_tui = !no_tui && std::io::stdout().is_terminal();

    if use_tui {
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
        let result = build_handle
            .await
            .context("Build task panicked")?
            .context("Build failed")?;

        println!("\nBuilt image: {}", result.image_id);
        for tag in &result.tags {
            println!("  Tagged: {tag}");
        }
        println!("Build time: {}ms", result.build_time_ms);
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
        let result = build_handle
            .await
            .context("Build task panicked")?
            .context("Build failed")?;

        println!("\nBuilt image: {}", result.image_id);
        for tag in &result.tags {
            println!("  Tagged: {tag}");
        }
        println!("Build time: {}ms", result.build_time_ms);
    }

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
