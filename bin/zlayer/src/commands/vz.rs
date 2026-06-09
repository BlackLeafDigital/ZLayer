//! `zlayer vz` — macOS Apple-Virtualization (VZ) base-image tooling.
//!
//! Builds a Tart-style base bundle (`disk.img`, `hardware-model.bin`,
//! `aux.img`) from a macOS `.ipsw` via
//! [`zlayer_agent::runtimes::macos_vz_build`], then optionally packs it into a
//! single `tar+zstd` OCI layer and pushes it to a registry. The pushed manifest
//! carries the routing annotation `com.zlayer.runtime=vz`
//! ([`ZLAYER_RUNTIME_ANNOTATION`]=[`ZLAYER_RUNTIME_VZ`]) that the agent's
//! composite runtime reads to prefer the VZ runtime for that image (see
//! `crates/zlayer-agent/src/runtimes/composite.rs`).

use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use tracing::info;

use zlayer_agent::runtimes::macos_vz_build::{
    build_base_image_bundle, BuildBaseParams, IpswSource,
};
use zlayer_registry::pack::{pack_files_tar_zstd, DEFAULT_ZSTD_LEVEL};
use zlayer_registry::{
    ArtifactLayer, BlobCache, ImagePuller, RegistryAuth, ZLAYER_RUNTIME_ANNOTATION,
    ZLAYER_RUNTIME_VZ,
};

use crate::cli::{Cli, VzCommands};

/// OCI artifact type for a published macOS VZ base bundle.
const VZ_ARTIFACT_TYPE: &str = "application/vnd.zlayer.macos-vz-bundle.v1";
/// The standard OCI empty config descriptor.
const OCI_EMPTY_CONFIG_MEDIA_TYPE: &str = "application/vnd.oci.empty.v1+json";

/// The three files that make up a VZ base bundle, in layer order.
const BUNDLE_FILES: [&str; 3] = ["disk.img", "hardware-model.bin", "aux.img"];

pub(crate) async fn handle_vz(cli: &Cli, cmd: &VzCommands) -> Result<()> {
    match cmd {
        VzCommands::BuildBase {
            ipsw,
            latest,
            output,
            disk_size_gib,
            cpus,
            memory_mib,
            push,
            username,
            password,
        } => {
            let source = match (ipsw, latest) {
                (Some(s), false) => {
                    if s.starts_with("http://") || s.starts_with("https://") {
                        IpswSource::Url(s.clone())
                    } else {
                        IpswSource::Local(std::path::PathBuf::from(s))
                    }
                }
                (None, true) => IpswSource::Latest,
                (Some(_), true) => bail!("--ipsw and --latest are mutually exclusive"),
                (None, false) => {
                    bail!("provide a restore image with --ipsw <PATH_OR_URL> or use --latest")
                }
            };

            println!("Building macOS VZ base image...");
            println!("  Source:    {source:?}");
            println!("  Output:    {}", output.display());
            println!("  Disk size: {disk_size_gib} GiB");

            let bundle = build_base_image_bundle(BuildBaseParams {
                source,
                output_dir: output.clone(),
                disk_size_gib: *disk_size_gib,
                cpu_count: *cpus,
                memory_mib: *memory_mib,
            })
            .await
            .context("Failed to build VZ base image")?;

            println!("\nBuild complete:");
            println!("  Directory:    {}", bundle.dir.display());
            println!(
                "  macOS:        {} (build {})",
                bundle.os_version, bundle.build_version
            );
            for f in BUNDLE_FILES {
                println!("    - {f}");
            }

            if let Some(reference) = push {
                push_bundle(cli, &bundle, reference, username.clone(), password.clone()).await?;
            } else {
                println!(
                    "\nNot pushed (no --push). To deploy this bundle locally, reference the \
                     directory as the service image."
                );
            }

            Ok(())
        }
    }
}

/// Pack the bundle into a single `tar+zstd` OCI layer and push it, stamping the
/// VZ routing annotation onto the manifest.
async fn push_bundle(
    cli: &Cli,
    bundle: &zlayer_agent::runtimes::macos_vz_build::BuiltBundle,
    reference: &str,
    username: Option<String>,
    password: Option<String>,
) -> Result<()> {
    println!("\nPacking bundle into a tar+zstd layer (this can take a while for large disks)...");

    let dir = bundle.dir.clone();
    let (layer_bytes, media_type) = tokio::task::spawn_blocking(move || {
        pack_files_tar_zstd(&dir, &BUNDLE_FILES, DEFAULT_ZSTD_LEVEL)
    })
    .await
    .context("pack task panicked")?
    .context("Failed to pack bundle layer")?;

    println!("  Layer: {} bytes ({media_type})", layer_bytes.len());

    let auth = resolve_auth(username, password);

    let cache_dir = cli.effective_data_dir().join("cache");
    tokio::fs::create_dir_all(&cache_dir)
        .await
        .context("Failed to create cache directory")?;
    let cache = BlobCache::open(&cache_dir).context("Failed to create blob cache")?;
    let puller = ImagePuller::new(cache);

    let mut annotations = BTreeMap::new();
    annotations.insert(
        ZLAYER_RUNTIME_ANNOTATION.to_string(),
        ZLAYER_RUNTIME_VZ.to_string(),
    );
    annotations.insert(
        "org.opencontainers.image.version".to_string(),
        bundle.os_version.clone(),
    );
    annotations.insert(
        "com.zlayer.macos.build-version".to_string(),
        bundle.build_version.clone(),
    );

    let layers = vec![ArtifactLayer {
        data: layer_bytes,
        media_type,
        title: Some("macos-vz-bundle.tar.zst".to_string()),
    }];

    println!("Pushing to {reference}...");
    let result = puller
        .push_artifact(
            reference,
            VZ_ARTIFACT_TYPE,
            OCI_EMPTY_CONFIG_MEDIA_TYPE,
            b"{}",
            &layers,
            annotations,
            &auth,
        )
        .await
        .context("Failed to push VZ base bundle")?;

    info!(reference = %result.reference, digest = %result.manifest_digest, "pushed VZ base bundle");
    println!("\nPush successful!");
    println!("  Reference:       {}", result.reference);
    println!("  Manifest digest: {}", result.manifest_digest);
    println!("  Blobs pushed:    {}", result.blobs_pushed.len());
    Ok(())
}

/// Resolve registry auth from flags, falling back to environment variables.
fn resolve_auth(username: Option<String>, password: Option<String>) -> RegistryAuth {
    match (username, password) {
        (Some(user), Some(pass)) => RegistryAuth::Basic(user, pass),
        (Some(user), None) => {
            if let Ok(pass) = std::env::var("ZLAYER_REGISTRY_PASSWORD")
                .or_else(|_| std::env::var("REGISTRY_PASSWORD"))
            {
                RegistryAuth::Basic(user, pass)
            } else {
                RegistryAuth::Anonymous
            }
        }
        _ => {
            if let (Ok(user), Ok(pass)) = (
                std::env::var("ZLAYER_REGISTRY_USERNAME")
                    .or_else(|_| std::env::var("REGISTRY_USERNAME")),
                std::env::var("ZLAYER_REGISTRY_PASSWORD")
                    .or_else(|_| std::env::var("REGISTRY_PASSWORD")),
            ) {
                RegistryAuth::Basic(user, pass)
            } else {
                RegistryAuth::Anonymous
            }
        }
    }
}
