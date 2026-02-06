use anyhow::{Context, Result};
use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::mpsc;
use tracing::{info, warn};

/// Build a container image from a Dockerfile or runtime template
#[allow(clippy::too_many_arguments)]
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
    push: bool,
    no_tui: bool,
    verbose_build: bool,
) -> Result<()> {
    use zlayer_builder::{detect_runtime, BuildEvent, ImageBuilder, PlainLogger, Runtime};

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
        match Runtime::from_name(&name) {
            Some(rt) => {
                info!(runtime = %rt, "Using specified runtime template");
                Some(rt)
            }
            None => {
                // List available runtimes in error message
                let available: Vec<_> = Runtime::all().iter().map(|r| r.name).collect();
                anyhow::bail!(
                    "Unknown runtime: '{}'. Available runtimes: {}",
                    name,
                    available.join(", ")
                );
            }
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
                eprintln!("Warning: invalid build-arg '{}', expected KEY=VALUE", arg);
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
            println!("  Tagged: {}", tag);
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
            println!("  Tagged: {}", tag);
        }
        println!("Build time: {}ms", result.build_time_ms);
    }

    Ok(())
}

/// List available runtime templates
pub(crate) async fn handle_runtimes() -> Result<()> {
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
