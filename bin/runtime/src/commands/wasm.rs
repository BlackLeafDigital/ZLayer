use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::info;

use zlayer_builder::wasm_builder::{build_wasm, WasiTarget, WasmBuildConfig, WasmLanguage};
use zlayer_registry::wasm::{extract_wasm_binary_info, validate_wasm_magic, WasiVersion};
use zlayer_registry::{export_wasm_as_oci, BlobCache, ImagePuller, RegistryAuth, WasmExportConfig};

use crate::cli::{Cli, WasmCommands};

pub(crate) async fn handle_wasm(cli: &Cli, cmd: &WasmCommands) -> Result<()> {
    match cmd {
        WasmCommands::Build {
            context,
            language,
            target,
            output,
            optimize,
            wit,
        } => {
            handle_wasm_build(
                context.clone(),
                language.clone(),
                target.clone(),
                output.clone(),
                *optimize,
                wit.clone(),
            )
            .await
        }
        WasmCommands::Export {
            wasm_file,
            name,
            output,
        } => handle_wasm_export(wasm_file, name, output.clone()).await,
        WasmCommands::Push {
            wasm_file,
            reference,
            username,
            password,
        } => {
            handle_wasm_push(
                cli,
                wasm_file,
                reference,
                username.clone(),
                password.clone(),
            )
            .await
        }
        WasmCommands::Validate { wasm_file } => handle_wasm_validate(wasm_file).await,
        WasmCommands::Info { wasm_file } => handle_wasm_info(wasm_file).await,
    }
}

/// Build WASM from source code
pub(crate) async fn handle_wasm_build(
    context: PathBuf,
    language: Option<String>,
    target: String,
    output: Option<PathBuf>,
    optimize: bool,
    wit: Option<PathBuf>,
) -> Result<()> {
    info!(
        context = %context.display(),
        language = ?language,
        target = %target,
        optimize = optimize,
        "Building WASM"
    );

    // Parse target
    let wasi_target = match target.to_lowercase().as_str() {
        "preview1" | "wasip1" | "p1" => WasiTarget::Preview1,
        "preview2" | "wasip2" | "p2" => WasiTarget::Preview2,
        _ => {
            anyhow::bail!(
                "Invalid WASI target '{}'. Valid options: preview1, preview2, wasip1, wasip2, p1, p2",
                target
            );
        }
    };

    // Parse language if specified
    let wasm_language = match language.as_deref() {
        Some(lang) => {
            let lang_lower = lang.to_lowercase();
            match lang_lower.as_str() {
                "rust" => Some(WasmLanguage::Rust),
                "rust-component" | "cargo-component" => Some(WasmLanguage::RustComponent),
                "go" | "tinygo" => Some(WasmLanguage::Go),
                "python" | "py" => Some(WasmLanguage::Python),
                "typescript" | "ts" => Some(WasmLanguage::TypeScript),
                "assemblyscript" | "as" => Some(WasmLanguage::AssemblyScript),
                "c" => Some(WasmLanguage::C),
                "zig" => Some(WasmLanguage::Zig),
                _ => {
                    anyhow::bail!(
                        "Unknown language '{}'. Supported: rust, rust-component, go, python, typescript, assemblyscript, c, zig",
                        lang
                    );
                }
            }
        }
        None => None, // Auto-detect
    };

    // Build config
    let mut config = WasmBuildConfig::new()
        .target(wasi_target)
        .optimize(optimize);

    if let Some(lang) = wasm_language {
        config = config.language(lang);
    }

    if let Some(wit_path) = wit {
        config = config.wit_path(wit_path);
    }

    if let Some(out_path) = output.clone() {
        config = config.output_path(out_path);
    }

    println!("Building WASM from {}...", context.display());
    println!("  Target: {}", wasi_target);
    if let Some(lang) = &wasm_language {
        println!("  Language: {}", lang);
    } else {
        println!("  Language: auto-detect");
    }
    if optimize {
        println!("  Mode: release (optimized)");
    } else {
        println!("  Mode: debug");
    }

    let result = build_wasm(&context, config)
        .await
        .context("WASM build failed")?;

    println!("\nBuild successful!");
    println!("  Output: {}", result.wasm_path.display());
    println!("  Language: {}", result.language);
    println!("  Target: {}", result.target);
    println!("  Size: {} bytes", result.size);

    Ok(())
}

/// Export WASM binary as OCI artifact
pub(crate) async fn handle_wasm_export(
    wasm_file: &Path,
    name: &str,
    output: Option<PathBuf>,
) -> Result<()> {
    info!(
        wasm_file = %wasm_file.display(),
        name = %name,
        "Exporting WASM as OCI artifact"
    );

    // Parse name to extract module name (strip tag/digest if present)
    let module_name = name
        .rsplit('/')
        .next()
        .unwrap_or(name)
        .split(':')
        .next()
        .unwrap_or(name)
        .split('@')
        .next()
        .unwrap_or(name);

    let config = WasmExportConfig {
        wasm_path: wasm_file.to_path_buf(),
        module_name: module_name.to_string(),
        wasi_version: None, // Auto-detect
        annotations: HashMap::new(),
    };

    println!("Exporting WASM as OCI artifact...");
    println!("  Input: {}", wasm_file.display());
    println!("  Name: {}", name);

    let result = export_wasm_as_oci(&config)
        .await
        .context("Failed to export WASM as OCI artifact")?;

    // Determine output directory
    let output_dir = output.unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    // Create OCI layout directory structure
    let artifact_dir = output_dir.join(format!("{}-oci", module_name));
    tokio::fs::create_dir_all(&artifact_dir)
        .await
        .context("Failed to create output directory")?;

    // Write oci-layout file
    let oci_layout = serde_json::json!({
        "imageLayoutVersion": "1.0.0"
    });
    tokio::fs::write(
        artifact_dir.join("oci-layout"),
        serde_json::to_string_pretty(&oci_layout)?,
    )
    .await
    .context("Failed to write oci-layout")?;

    // Create blobs directory
    let blobs_dir = artifact_dir.join("blobs").join("sha256");
    tokio::fs::create_dir_all(&blobs_dir)
        .await
        .context("Failed to create blobs directory")?;

    // Write config blob
    let config_hash = result
        .config_digest
        .strip_prefix("sha256:")
        .unwrap_or(&result.config_digest);
    tokio::fs::write(blobs_dir.join(config_hash), &result.config_blob)
        .await
        .context("Failed to write config blob")?;

    // Write WASM layer blob
    let wasm_hash = result
        .wasm_layer_digest
        .strip_prefix("sha256:")
        .unwrap_or(&result.wasm_layer_digest);
    tokio::fs::write(blobs_dir.join(wasm_hash), &result.wasm_binary)
        .await
        .context("Failed to write WASM blob")?;

    // Write manifest blob
    let manifest_hash = result
        .manifest_digest
        .strip_prefix("sha256:")
        .unwrap_or(&result.manifest_digest);
    tokio::fs::write(blobs_dir.join(manifest_hash), &result.manifest_json)
        .await
        .context("Failed to write manifest blob")?;

    // Write index.json
    let index = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.index.v1+json",
        "manifests": [{
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "digest": result.manifest_digest,
            "size": result.manifest_size,
            "annotations": {
                "org.opencontainers.image.ref.name": name
            }
        }]
    });
    tokio::fs::write(
        artifact_dir.join("index.json"),
        serde_json::to_string_pretty(&index)?,
    )
    .await
    .context("Failed to write index.json")?;

    println!("\nExport successful!");
    println!("  Output directory: {}", artifact_dir.display());
    println!("  WASI version: {}", result.wasi_version);
    println!("  Artifact type: {}", result.artifact_type);
    println!("  Manifest digest: {}", result.manifest_digest);
    println!("  WASM layer digest: {}", result.wasm_layer_digest);
    println!("  WASM size: {} bytes", result.wasm_size);

    Ok(())
}

/// Push WASM artifact to registry
pub(crate) async fn handle_wasm_push(
    cli: &Cli,
    wasm_file: &Path,
    reference: &str,
    username: Option<String>,
    password: Option<String>,
) -> Result<()> {
    info!(
        wasm_file = %wasm_file.display(),
        reference = %reference,
        "Pushing WASM to registry"
    );

    // Parse reference to extract module name
    let module_name = reference
        .rsplit('/')
        .next()
        .unwrap_or(reference)
        .split(':')
        .next()
        .unwrap_or(reference)
        .split('@')
        .next()
        .unwrap_or(reference);

    // Export WASM as OCI artifact first
    let config = WasmExportConfig {
        wasm_path: wasm_file.to_path_buf(),
        module_name: module_name.to_string(),
        wasi_version: None, // Auto-detect
        annotations: HashMap::new(),
    };

    println!("Preparing WASM artifact for push...");
    println!("  Input: {}", wasm_file.display());
    println!("  Reference: {}", reference);

    let export_result = export_wasm_as_oci(&config)
        .await
        .context("Failed to prepare WASM artifact")?;

    println!("  WASI version: {}", export_result.wasi_version);
    println!("  Artifact type: {}", export_result.artifact_type);
    println!("  WASM size: {} bytes", export_result.wasm_size);

    // Setup authentication
    let auth = match (username, password) {
        (Some(user), Some(pass)) => {
            println!("  Auth: Basic (username provided)");
            RegistryAuth::Basic(user, pass)
        }
        (Some(user), None) => {
            // Try to get password from environment
            let pass = std::env::var("ZLAYER_REGISTRY_PASSWORD")
                .or_else(|_| std::env::var("REGISTRY_PASSWORD"))
                .context(
                    "Password not provided. Use --password or set ZLAYER_REGISTRY_PASSWORD env var",
                )?;
            println!("  Auth: Basic (password from env)");
            RegistryAuth::Basic(user, pass)
        }
        (None, Some(_)) => {
            anyhow::bail!("Password provided without username");
        }
        (None, None) => {
            // Try to get credentials from environment
            if let (Ok(user), Ok(pass)) = (
                std::env::var("ZLAYER_REGISTRY_USERNAME")
                    .or_else(|_| std::env::var("REGISTRY_USERNAME")),
                std::env::var("ZLAYER_REGISTRY_PASSWORD")
                    .or_else(|_| std::env::var("REGISTRY_PASSWORD")),
            ) {
                println!("  Auth: Basic (from env)");
                RegistryAuth::Basic(user, pass)
            } else {
                println!("  Auth: Anonymous");
                RegistryAuth::Anonymous
            }
        }
    };

    // Create blob cache and image puller
    let cache_dir = cli.state_dir.join("cache");
    tokio::fs::create_dir_all(&cache_dir)
        .await
        .context("Failed to create cache directory")?;

    let cache = BlobCache::open(&cache_dir).context("Failed to create blob cache")?;
    let puller = ImagePuller::new(cache);

    println!("\nPushing to registry...");

    let push_result = puller
        .push_wasm(reference, &export_result, &auth)
        .await
        .context("Failed to push WASM to registry")?;

    println!("\nPush successful!");
    println!("  Reference: {}", push_result.reference);
    println!("  Manifest digest: {}", push_result.manifest_digest);
    println!("  Blobs pushed: {}", push_result.blobs_pushed.len());
    for blob in &push_result.blobs_pushed {
        println!("    - {}", blob);
    }

    Ok(())
}

/// Validate a WASM binary
pub(crate) async fn handle_wasm_validate(wasm_file: &Path) -> Result<()> {
    info!(wasm_file = %wasm_file.display(), "Validating WASM binary");

    println!("Validating WASM binary: {}", wasm_file.display());

    // Read the file
    let data = tokio::fs::read(wasm_file)
        .await
        .with_context(|| format!("Failed to read file: {}", wasm_file.display()))?;

    // Check magic bytes first
    if !validate_wasm_magic(&data) {
        println!("\nValidation FAILED!");
        println!("  Error: Not a valid WASM binary (invalid magic bytes)");
        println!("  Expected: \\0asm (0x00, 0x61, 0x73, 0x6d)");
        if data.len() >= 4 {
            println!(
                "  Got: {:02x} {:02x} {:02x} {:02x}",
                data[0], data[1], data[2], data[3]
            );
        } else {
            println!("  Got: file too short ({} bytes)", data.len());
        }
        anyhow::bail!("Invalid WASM binary");
    }

    // Extract full info
    match extract_wasm_binary_info(&data) {
        Ok(info) => {
            println!("\nValidation PASSED!");
            println!("  WASI version: {}", info.wasi_version);
            println!(
                "  Type: {}",
                if info.is_component {
                    "Component (WASIp2)"
                } else {
                    "Core Module (WASIp1)"
                }
            );
            println!("  Binary version: {}", info.binary_version);
            println!("  Size: {} bytes", info.size);
            Ok(())
        }
        Err(e) => {
            println!("\nValidation FAILED!");
            println!("  Error: {}", e);
            anyhow::bail!("Invalid WASM binary: {}", e);
        }
    }
}

/// Show information about a WASM binary
pub(crate) async fn handle_wasm_info(wasm_file: &Path) -> Result<()> {
    info!(wasm_file = %wasm_file.display(), "Getting WASM info");

    // Read the file
    let data = tokio::fs::read(wasm_file)
        .await
        .with_context(|| format!("Failed to read file: {}", wasm_file.display()))?;

    let info = extract_wasm_binary_info(&data)
        .with_context(|| format!("Failed to parse WASM binary: {}", wasm_file.display()))?;

    println!("WASM Binary Information");
    println!("=======================");
    println!();
    println!("File: {}", wasm_file.display());
    println!(
        "Size: {} bytes ({:.2} KB)",
        info.size,
        info.size as f64 / 1024.0
    );
    println!();
    println!("Format:");
    println!(
        "  Type: {}",
        if info.is_component {
            "Component Model (WASIp2)"
        } else {
            "Core Module (WASIp1)"
        }
    );
    println!("  WASI Version: {}", info.wasi_version);
    println!("  Binary Version: {}", info.binary_version);
    println!();
    println!("OCI Artifact:");
    println!("  Media Type: {}", info.wasi_version.artifact_type());
    println!(
        "  Target Triple: {}",
        match info.wasi_version {
            WasiVersion::Preview1 => "wasm32-wasip1",
            WasiVersion::Preview2 => "wasm32-wasip2",
            WasiVersion::Unknown => "wasm32-wasi",
        }
    );

    Ok(())
}
