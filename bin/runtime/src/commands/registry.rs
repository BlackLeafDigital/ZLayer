use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tracing::info;

use crate::cli::Cli;
use crate::util::parse_image_reference;

/// Handle export command - export image to OCI tar archive
pub(crate) async fn handle_export(cli: &Cli, image: &str, output: &Path, gzip: bool) -> Result<()> {
    use zlayer_registry::{export_image, LocalRegistry};

    let registry_path = cli.state_dir.join("registry");
    let registry = LocalRegistry::new(registry_path)
        .await
        .context("Failed to open local registry")?;

    // If gzip flag is set and output doesn't end in .gz, append it
    let output = if gzip && output.extension().is_none_or(|e| e != "gz") {
        output.with_extension("tar.gz")
    } else {
        output.to_path_buf()
    };

    info!("Exporting {} to {}", image, output.display());
    println!("Exporting {} to {}...", image, output.display());

    let export_info = export_image(&registry, image, &output)
        .await
        .context("Failed to export image")?;

    println!("Exported successfully!");
    println!("  Digest: {}", export_info.digest);
    println!("  Layers: {}", export_info.layers);
    println!("  Size: {} bytes", export_info.size);
    println!("  Output: {}", export_info.output_path.display());

    Ok(())
}

/// Handle import command - import image from OCI tar archive
pub(crate) async fn handle_import(cli: &Cli, input: &Path, tag: Option<String>) -> Result<()> {
    use zlayer_registry::{import_image, LocalRegistry};

    let registry_path = cli.state_dir.join("registry");
    let registry = LocalRegistry::new(registry_path)
        .await
        .context("Failed to open local registry")?;

    info!("Importing from {}", input.display());
    println!("Importing from {}...", input.display());

    let import_info = import_image(&registry, input, tag.as_deref())
        .await
        .context("Failed to import image")?;

    println!("Imported successfully!");
    println!("  Digest: {}", import_info.digest);
    println!("  Layers: {}", import_info.layers);
    if let Some(tag) = import_info.tag {
        println!("  Tagged as: {}", tag);
    }

    Ok(())
}

/// Handle pull command - pull an image from a remote registry to local cache
pub(crate) async fn handle_pull(image: &str) -> Result<()> {
    use zlayer_registry::{BlobCache, ImagePuller, LocalRegistry, RegistryAuth};

    info!(image = %image, "Pulling image from registry");
    println!("Pulling {}...", image);

    // Set up directories
    let data_dir = PathBuf::from("/var/lib/zlayer");
    let cache_dir = data_dir.join("cache");
    let registry_path = data_dir.join("registry");

    tokio::fs::create_dir_all(&cache_dir)
        .await
        .context("Failed to create cache directory")?;

    // Create blob cache and image puller
    let cache = BlobCache::open(&cache_dir).context("Failed to create blob cache")?;
    let puller = ImagePuller::new(cache);

    // Use anonymous auth (for public images) or check for credentials
    // TODO: Support authenticated registries via config or env vars
    let auth = RegistryAuth::Anonymous;

    // Pull manifest first to show what we're pulling
    println!("Fetching manifest...");
    let (manifest, manifest_digest) = puller
        .pull_manifest(image, &auth)
        .await
        .context("Failed to pull manifest")?;

    println!(
        "  Manifest: {} ({} layers)",
        &manifest_digest[..19],
        manifest.layers.len()
    );

    // Calculate total size
    let total_size: i64 = manifest.layers.iter().map(|l| l.size).sum();
    let total_mb = total_size as f64 / 1024.0 / 1024.0;
    println!("  Total size: {:.2} MB", total_mb);

    // Pull all layers
    println!("\nPulling layers...");
    let layers = puller
        .pull_image(image, &auth)
        .await
        .context("Failed to pull image layers")?;

    println!("  Downloaded {} layers", layers.len());

    // Store in local registry
    let registry = LocalRegistry::new(registry_path)
        .await
        .context("Failed to open local registry")?;

    // Store each layer blob
    for (i, (layer_data, _media_type)) in layers.iter().enumerate() {
        let digest = registry
            .put_blob(layer_data)
            .await
            .context("Failed to store layer blob")?;
        println!(
            "  Layer {}: {} ({:.2} MB)",
            i + 1,
            &digest[..19],
            layer_data.len() as f64 / 1024.0 / 1024.0
        );
    }

    // Store config blob
    let config_digest = &manifest.config.digest;
    let config_data = puller
        .pull_blob(image, config_digest, &auth)
        .await
        .context("Failed to pull config blob")?;
    registry
        .put_blob(&config_data)
        .await
        .context("Failed to store config blob")?;

    // Store manifest with proper name/tag
    // Parse image reference to get name and tag
    let (name, tag) = parse_image_reference(image);
    let manifest_json = serde_json::to_vec(&manifest).context("Failed to serialize manifest")?;
    registry
        .put_manifest(&name, &tag, &manifest_json)
        .await
        .context("Failed to store manifest")?;

    println!("\nPull complete!");
    println!("  Image: {}:{}", name, tag);
    println!("  Digest: {}", manifest_digest);
    println!("  Layers: {}", layers.len());

    Ok(())
}
