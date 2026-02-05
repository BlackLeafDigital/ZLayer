//! zlayer-build — Lightweight container image builder CLI
//!
//! Thin CLI wrapper around the `zlayer-builder` crate, providing three
//! subcommands: `build`, `runtimes`, and `validate`.

use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use tracing::{error, info};

use zlayer_builder::{BuildTui, BuildahInstaller, Dockerfile, ImageBuilder, PlainLogger, Runtime};

// ---------------------------------------------------------------------------
// CLI definitions
// ---------------------------------------------------------------------------

/// Lightweight container image builder - ZImagefile, Dockerfile, and runtime templates
#[derive(Parser)]
#[command(name = "zlayer-build", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Build an image from Dockerfile, ZImagefile, or runtime template
    Build(BuildArgs),
    /// Build multiple images from a pipeline manifest
    Pipeline(PipelineArgs),
    /// List available runtime templates
    Runtimes(RuntimesArgs),
    /// Parse and validate a Dockerfile or ZImagefile without building
    Validate(ValidateArgs),
}

#[derive(Parser)]
struct BuildArgs {
    /// Build context directory
    #[arg(default_value = ".")]
    context: PathBuf,

    /// Path to Dockerfile or ZImagefile (auto-detect if omitted)
    #[arg(short = 'f', long = "file")]
    file: Option<PathBuf>,

    /// Image tag (repeatable)
    #[arg(short = 't', long = "tag")]
    tag: Vec<String>,

    /// Build argument KEY=VALUE (repeatable)
    #[arg(long = "build-arg")]
    build_arg: Vec<String>,

    /// Target stage for multi-stage builds
    #[arg(long)]
    target: Option<String>,

    /// Use a runtime template instead of a file
    #[arg(long)]
    runtime: Option<String>,

    /// Disable layer caching
    #[arg(long)]
    no_cache: bool,

    /// Push after build
    #[arg(long)]
    push: bool,

    /// Output format: oci or docker (default: oci)
    #[arg(long, default_value = "oci")]
    format: String,

    /// Enable TUI progress display
    #[arg(long, conflicts_with = "no_tui")]
    tui: bool,

    /// Disable TUI progress display
    #[arg(long, conflicts_with = "tui")]
    no_tui: bool,
}

#[derive(Parser)]
struct RuntimesArgs {
    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
struct ValidateArgs {
    /// Path to Dockerfile or ZImagefile to validate
    path: PathBuf,
}

#[derive(Parser)]
struct PipelineArgs {
    /// Path to pipeline file
    #[arg(short = 'f', long = "file", default_value = "ZPipeline.yaml")]
    file: PathBuf,

    /// Set pipeline variables (KEY=VALUE, repeatable)
    #[arg(long = "set")]
    set: Vec<String>,

    /// Push all images after successful builds
    #[arg(long)]
    push: bool,

    /// Stop on first build failure (default: true)
    #[arg(long, default_value = "true")]
    fail_fast: bool,

    /// Disable TUI progress display
    #[arg(long)]
    no_tui: bool,

    /// Only build specific images (comma-separated)
    #[arg(long)]
    only: Option<String>,
}

// ---------------------------------------------------------------------------
// Entrypoint
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> ExitCode {
    // Initialize tracing from RUST_LOG env (default: info)
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Build(args) => cmd_build(args).await,
        Commands::Pipeline(args) => cmd_pipeline(args).await,
        Commands::Runtimes(args) => cmd_runtimes(args),
        Commands::Validate(args) => cmd_validate(args).await,
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            error!("{e:#}");
            ExitCode::FAILURE
        }
    }
}

// ---------------------------------------------------------------------------
// build subcommand
// ---------------------------------------------------------------------------

async fn cmd_build(args: BuildArgs) -> Result<()> {
    // 1. Check buildah is installed
    let installer = BuildahInstaller::new();
    let installation = installer
        .ensure()
        .await
        .context("Buildah is required for building images")?;
    info!(
        "Using buildah {} at {}",
        installation.version,
        installation.path.display()
    );

    // 2. Parse build arguments
    let build_args = parse_build_args(&args.build_arg)?;

    // 3. Create the ImageBuilder
    let context = args
        .context
        .canonicalize()
        .with_context(|| format!("Build context not found: {}", args.context.display()))?;

    let mut builder = ImageBuilder::new(&context)
        .await
        .context("Failed to initialize image builder")?;

    // Apply file path (detect ZImagefile vs Dockerfile)
    if let Some(ref file) = args.file {
        if is_likely_zimagefile(file) {
            builder = builder.zimagefile(file);
        } else {
            builder = builder.dockerfile(file);
        }
    }

    // Apply runtime template
    if let Some(ref runtime_name) = args.runtime {
        let rt = Runtime::from_name(runtime_name)
            .with_context(|| format!("Unknown runtime: {runtime_name}"))?;
        builder = builder.runtime(rt);
    }

    // Apply tags
    for tag in &args.tag {
        builder = builder.tag(tag);
    }

    // Apply build args
    builder = builder.build_args(build_args);

    // Apply target stage
    if let Some(ref target) = args.target {
        builder = builder.target(target);
    }

    // Apply no-cache
    if args.no_cache {
        builder = builder.no_cache();
    }

    // Apply push
    if args.push {
        builder = builder.push_without_auth();
    }

    // Apply format
    builder = builder.format(&args.format);

    // 4. Decide TUI vs plain logger
    let use_tui = if args.tui {
        true
    } else if args.no_tui {
        false
    } else {
        std::io::stderr().is_terminal()
    };

    if use_tui {
        // Run with TUI progress
        let (tx, rx) = mpsc::channel();
        builder = builder.with_events(tx);

        // Spawn the build on a separate task
        let build_handle = tokio::spawn(async move { builder.build().await });

        // Run TUI on the main thread (it needs terminal access)
        let mut tui = BuildTui::new(rx);
        if let Err(e) = tui.run() {
            // TUI errors are non-fatal — the build may have still succeeded
            tracing::warn!("TUI error: {e}");
        }

        let result = build_handle
            .await
            .context("Build task panicked")?
            .context("Build failed")?;

        print_build_result(&result);
    } else {
        // Run with plain logger
        let (tx, rx) = mpsc::channel();
        builder = builder.with_events(tx);

        let logger = PlainLogger::new(true);

        // Spawn logger on a separate thread
        let logger_handle = std::thread::spawn(move || {
            while let Ok(event) = rx.recv() {
                logger.handle_event(&event);
            }
        });

        let result = builder.build().await.context("Build failed")?;
        drop(result.tags.clone()); // ensure tx is dropped via builder drop
        let _ = logger_handle.join();

        print_build_result(&result);
    }

    Ok(())
}

fn print_build_result(result: &zlayer_builder::BuiltImage) {
    println!();
    println!("Image ID: {}", result.image_id);
    if !result.tags.is_empty() {
        println!("Tags:     {}", result.tags.join(", "));
    }
    println!("Layers:   {}", result.layer_count);
    println!("Time:     {:.1}s", result.build_time_ms as f64 / 1000.0);
}

// ---------------------------------------------------------------------------
// pipeline subcommand
// ---------------------------------------------------------------------------

async fn cmd_pipeline(args: PipelineArgs) -> Result<()> {
    use std::collections::HashSet;
    use zlayer_builder::{parse_pipeline, BuildahExecutor, PipelineExecutor};

    // 1. Read and parse pipeline file
    let content = tokio::fs::read_to_string(&args.file)
        .await
        .with_context(|| format!("Failed to read pipeline file: {}", args.file.display()))?;

    let mut pipeline = parse_pipeline(&content).context("Failed to parse pipeline")?;

    // 2. Merge --set variables
    for s in &args.set {
        let (key, value) = s
            .split_once('=')
            .with_context(|| format!("Invalid --set format '{}', expected KEY=VALUE", s))?;
        pipeline.vars.insert(key.to_string(), value.to_string());
    }

    // 3. Filter --only images
    if let Some(ref only) = args.only {
        let names: HashSet<&str> = only.split(',').map(|s| s.trim()).collect();
        pipeline.images.retain(|k, _| names.contains(k.as_str()));
        if pipeline.images.is_empty() {
            bail!("No images match --only filter: {}", only);
        }
    }

    // 4. Create executor
    let buildah = BuildahExecutor::new_async()
        .await
        .context("Failed to initialize buildah")?;

    let base_dir = args
        .file
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .canonicalize()
        .context("Failed to resolve pipeline directory")?;

    // 5. Run pipeline
    info!(
        "Building {} images from {}",
        pipeline.images.len(),
        args.file.display()
    );

    let executor = PipelineExecutor::new(pipeline, base_dir, buildah)
        .fail_fast(args.fail_fast)
        .push(args.push);

    let result = executor.run().await?;

    // 6. Print summary
    println!();
    println!(
        "Pipeline complete in {:.1}s",
        result.total_time_ms as f64 / 1000.0
    );
    println!("  Succeeded: {}", result.succeeded.len());
    if !result.failed.is_empty() {
        println!("  Failed: {}", result.failed.len());
        for (name, err) in &result.failed {
            println!("    {}: {}", name, err);
        }
        bail!("Pipeline failed with {} errors", result.failed.len());
    }

    Ok(())
}

fn parse_build_args(raw: &[String]) -> Result<HashMap<String, String>> {
    let mut map = HashMap::new();
    for arg in raw {
        let (key, value) = arg
            .split_once('=')
            .with_context(|| format!("Invalid build arg (expected KEY=VALUE): {arg}"))?;
        map.insert(key.to_string(), value.to_string());
    }
    Ok(map)
}

/// Heuristic: if the filename contains "ZImagefile" or the file extension is
/// `.yml`/`.yaml`, treat it as a ZImagefile; otherwise treat as Dockerfile.
fn is_likely_zimagefile(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name.contains("ZImagefile") {
        return true;
    }
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("yml" | "yaml")
    )
}

// ---------------------------------------------------------------------------
// runtimes subcommand
// ---------------------------------------------------------------------------

fn cmd_runtimes(args: RuntimesArgs) -> Result<()> {
    let templates = zlayer_builder::list_templates();

    if args.json {
        let items: Vec<serde_json::Value> = templates
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "detect_files": t.detect_files,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items)?);
    } else {
        println!("{:<14} {:<12} DESCRIPTION", "RUNTIME", "DETECT");
        println!("{}", "-".repeat(72));
        for t in &templates {
            let detect = t.detect_files.join(", ");
            println!("{:<14} {:<12} {}", t.name, detect, t.description);
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// validate subcommand
// ---------------------------------------------------------------------------

async fn cmd_validate(args: ValidateArgs) -> Result<()> {
    let path = &args.path;
    if !path.exists() {
        bail!("File not found: {}", path.display());
    }

    let content = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("Failed to read {}", path.display()))?;

    // Detect format: try YAML (ZImagefile) first, then Dockerfile
    let is_yaml = is_likely_zimagefile(&args.path) || looks_like_yaml(&content);

    if is_yaml {
        match zlayer_builder::zimage::parse_zimagefile(&content) {
            Ok(zimage) => {
                println!("Valid ZImagefile: {}", path.display());
                print_zimagefile_summary(&zimage);
                Ok(())
            }
            Err(e) => {
                eprintln!("Invalid ZImagefile: {}", path.display());
                eprintln!("Error: {e}");
                bail!("Validation failed");
            }
        }
    } else {
        match Dockerfile::parse(&content) {
            Ok(dockerfile) => {
                println!("Valid Dockerfile: {}", path.display());
                println!("  Stages: {}", dockerfile.stages.len());
                for stage in &dockerfile.stages {
                    let name = stage.name.as_deref().unwrap_or("(unnamed)");
                    println!(
                        "    [{}] {} - FROM {} ({} instructions)",
                        stage.index,
                        name,
                        stage.base_image.to_string_ref(),
                        stage.instructions.len()
                    );
                }
                Ok(())
            }
            Err(e) => {
                eprintln!("Invalid Dockerfile: {}", path.display());
                eprintln!("Error: {e}");
                bail!("Validation failed");
            }
        }
    }
}

fn print_zimagefile_summary(zimage: &zlayer_builder::zimage::ZImage) {
    if let Some(ref runtime) = zimage.runtime {
        println!("  Mode: runtime template ({runtime})");
    } else if let Some(ref stages) = zimage.stages {
        println!("  Mode: multi-stage ({} stages)", stages.len());
        for (name, stage) in stages {
            println!(
                "    [{name}] FROM {} ({} steps)",
                stage.base.as_deref().unwrap_or("scratch"),
                stage.steps.len()
            );
        }
    } else if let Some(ref base) = zimage.base {
        println!("  Mode: single-stage (FROM {base})");
        println!("  Steps: {}", zimage.steps.len());
    } else if zimage.wasm.is_some() {
        println!("  Mode: WASM");
    } else {
        println!("  Mode: empty (no build instructions)");
    }
}

/// Quick heuristic: content starts with a YAML-ish key.
fn looks_like_yaml(content: &str) -> bool {
    let trimmed = content.trim_start();
    // ZImagefiles typically start with version:, runtime:, base:, stages:, or wasm:
    trimmed.starts_with("version:")
        || trimmed.starts_with("runtime:")
        || trimmed.starts_with("base:")
        || trimmed.starts_with("stages:")
        || trimmed.starts_with("wasm:")
}
