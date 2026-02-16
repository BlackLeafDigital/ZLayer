//! Pipeline build command -- builds multiple images from a pipeline manifest.
//!
//! Ported from `bin/zlayer-build/src/main.rs`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use tracing::info;

use zlayer_builder::{parse_pipeline, BuildahExecutor, PipelineExecutor};

/// Discover pipeline file from explicit path or well-known defaults.
fn discover_pipeline_file(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path);
    }
    let candidates = [
        "ZPipeline.yaml",
        "ZPipeline.yml",
        "zlayer-pipeline.yaml",
        "zlayer-pipeline.yml",
    ];
    for candidate in &candidates {
        let path = Path::new(candidate);
        if path.exists() {
            return Ok(path.to_path_buf());
        }
    }
    bail!(
        "No pipeline file found. Create a ZPipeline.yaml (or zlayer-pipeline.yaml), \
         or specify a path with -f."
    );
}

/// Execute the pipeline build command.
pub async fn cmd_pipeline(
    file: Option<PathBuf>,
    set: Vec<String>,
    push: bool,
    fail_fast: bool,
    _no_tui: bool,
    only: Option<String>,
) -> Result<()> {
    let file = discover_pipeline_file(file)?;

    // 1. Read and parse pipeline file
    let content = tokio::fs::read_to_string(&file)
        .await
        .with_context(|| format!("Failed to read pipeline file: {}", file.display()))?;

    let mut pipeline = parse_pipeline(&content).context("Failed to parse pipeline")?;

    // 2. Merge --set variables
    for s in &set {
        let (key, value) = s
            .split_once('=')
            .with_context(|| format!("Invalid --set format '{}', expected KEY=VALUE", s))?;
        pipeline.vars.insert(key.to_string(), value.to_string());
    }

    // 3. Filter --only images
    if let Some(ref only_filter) = only {
        let names: HashSet<&str> = only_filter.split(',').map(|s| s.trim()).collect();
        pipeline.images.retain(|k, _| names.contains(k.as_str()));
        if pipeline.images.is_empty() {
            bail!("No images match --only filter: {}", only_filter);
        }
    }

    // 4. Create executor
    let buildah = BuildahExecutor::new_async()
        .await
        .context("Failed to initialize buildah")?;

    let base_dir = file
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .canonicalize()
        .context("Failed to resolve pipeline directory")?;

    // 5. Run pipeline
    info!(
        "Building {} images from {}",
        pipeline.images.len(),
        file.display()
    );

    let executor = PipelineExecutor::new(pipeline, base_dir, buildah)
        .fail_fast(fail_fast)
        .push(push);

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
