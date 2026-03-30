//! macOS Seatbelt sandbox build backend.
//!
//! Uses [`SandboxImageBuilder`] when buildah is not available on macOS.

use std::path::{Path, PathBuf};

use crate::builder::{BuildOptions, BuiltImage, RegistryAuth};
use crate::dockerfile::Dockerfile;
use crate::error::{BuildError, Result};
use crate::sandbox_builder::SandboxImageBuilder;
use crate::tui::BuildEvent;
use zlayer_paths::ZLayerDirs;

use super::BuildBackend;

/// macOS sandbox build backend.
///
/// Builds images using the native macOS Seatbelt sandbox, producing a rootfs
/// directory + `config.json` rather than OCI images. Push support requires the
/// `cache` feature (which provides the `zlayer-registry` dependency).
pub struct SandboxBackend {
    /// Base data directory for storing images (e.g. `~/.zlayer`).
    data_dir: PathBuf,
}

impl SandboxBackend {
    /// Create a new sandbox backend with the given data directory.
    #[must_use]
    pub fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }

    /// Resolve the on-disk image directory for a given tag.
    fn image_dir_for_tag(&self, tag: &str) -> PathBuf {
        let sanitized = crate::sandbox_builder::sanitize_image_name(tag);
        self.data_dir.join("images").join(sanitized)
    }
}

impl Default for SandboxBackend {
    fn default() -> Self {
        let data_dir = ZLayerDirs::default_data_dir();
        Self { data_dir }
    }
}

#[async_trait::async_trait]
impl BuildBackend for SandboxBackend {
    async fn build_image(
        &self,
        context: &Path,
        dockerfile: &Dockerfile,
        options: &BuildOptions,
        event_tx: Option<std::sync::mpsc::Sender<BuildEvent>>,
    ) -> Result<BuiltImage> {
        let mut builder = SandboxImageBuilder::new(context.to_path_buf(), self.data_dir.clone());

        // Transfer build args
        if !options.build_args.is_empty() {
            builder = builder.with_build_args(options.build_args.clone());
        }

        // Attach event channel if provided
        if let Some(tx) = event_tx {
            builder = builder.with_events(tx);
        }

        // Forward pre-computed source hash for deterministic cache invalidation
        if let Some(ref hash) = options.source_hash {
            builder = builder.with_source_hash(hash.clone());
        }

        // Run the build
        let result = builder.build(dockerfile, &options.tags).await?;

        // Convert SandboxBuildResult → BuiltImage
        Ok(BuiltImage {
            image_id: result.image_id,
            tags: result.tags,
            layer_count: 1, // sandbox builds produce a single rootfs
            size: 0,        // not computed by sandbox builder
            build_time_ms: result.build_time_ms,
            is_manifest: false,
        })
    }

    async fn push_image(&self, tag: &str, auth: Option<&RegistryAuth>) -> Result<()> {
        sandbox_push::push_image(&self.data_dir, tag, auth).await
    }

    async fn tag_image(&self, image: &str, new_tag: &str) -> Result<()> {
        let src = self.image_dir_for_tag(image);
        let dst = self.image_dir_for_tag(new_tag);

        if !src.exists() {
            return Err(BuildError::IoError(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("image not found: {image}"),
            )));
        }

        if dst.exists() {
            tokio::fs::remove_dir_all(&dst).await?;
        }

        // Copy the directory tree (symlinks don't work well across mounts)
        copy_dir_recursive(&src, &dst).await?;
        Ok(())
    }

    async fn manifest_create(&self, name: &str) -> Result<()> {
        sandbox_push::manifest_create(&self.data_dir, name).await
    }

    async fn manifest_add(&self, manifest: &str, image: &str) -> Result<()> {
        sandbox_push::manifest_add(&self.data_dir, manifest, image).await
    }

    async fn manifest_push(&self, name: &str, destination: &str) -> Result<()> {
        sandbox_push::manifest_push(&self.data_dir, name, destination, None).await
    }

    async fn is_available(&self) -> bool {
        // The sandbox backend is always available on macOS
        true
    }

    fn name(&self) -> &'static str {
        "sandbox"
    }
}

/// Recursively copy a directory.
async fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    tokio::fs::create_dir_all(dst).await?;
    let mut entries = tokio::fs::read_dir(src).await?;
    while let Some(entry) = entries.next_entry().await? {
        let ty = entry.file_type().await?;
        let dest_path = dst.join(entry.file_name());
        if ty.is_dir() {
            Box::pin(copy_dir_recursive(&entry.path(), &dest_path)).await?;
        } else {
            tokio::fs::copy(entry.path(), &dest_path).await?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Push implementation — requires zlayer-registry (behind the `cache` feature)
// ---------------------------------------------------------------------------

#[cfg(feature = "cache")]
mod sandbox_push {
    use super::*;
    use crate::sandbox_builder::SandboxImageConfig;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use oci_client::manifest::{
        ImageIndexEntry, OciDescriptor, OciImageIndex, OciImageManifest, Platform,
    };
    use sha2::{Digest, Sha256};
    use std::io::Write;
    use tar::Builder;
    use tracing::{debug, info};
    use zlayer_core::auth::DockerConfigAuth;
    use zlayer_registry::{BlobCache, ImagePuller, RegistryAuth as OciRegistryAuth};

    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
    struct SandboxManifestIndex {
        entries: Vec<SandboxManifestEntry>,
    }

    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
    struct SandboxManifestEntry {
        image_tag: String,
        manifest_digest: String,
        manifest_size: i64,
        layer_digest: String,
        layer_size: i64,
        config_digest: String,
        config_size: i64,
        os: String,
        architecture: String,
    }

    struct PreparedImage {
        layer_blob: Vec<u8>,
        layer_digest: String,
        layer_size: i64,
        config_blob: Vec<u8>,
        config_digest: String,
        config_size: i64,
        manifest_bytes: Vec<u8>,
        manifest_digest: String,
        manifest_size: i64,
    }

    /// Prepare a sandbox-built image for push by computing all blobs, digests, and
    /// the OCI manifest.
    ///
    /// Resolves the image directory from `tag`, reads the rootfs and sandbox config,
    /// creates the gzipped tar layer, builds the OCI config and manifest, and returns
    /// everything packaged in a [`PreparedImage`].
    async fn prepare_image_for_push(data_dir: &Path, tag: &str) -> Result<PreparedImage> {
        let sanitized = crate::sandbox_builder::sanitize_image_name(tag);
        let image_dir = data_dir.join("images").join(&sanitized);
        let rootfs_dir = image_dir.join("rootfs");
        let config_path = image_dir.join("config.json");

        if !rootfs_dir.exists() {
            return Err(BuildError::IoError(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("rootfs not found for image: {tag}"),
            )));
        }

        // Read sandbox config
        let sandbox_config: SandboxImageConfig = if config_path.exists() {
            let data = tokio::fs::read_to_string(&config_path).await?;
            serde_json::from_str(&data).map_err(|e| {
                BuildError::IoError(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("invalid config.json: {e}"),
                ))
            })?
        } else {
            SandboxImageConfig::default()
        };

        // 1. Create the layer: tar + gzip the rootfs
        info!("Creating layer from rootfs for {}", tag);
        let layer_blob = create_tar_gz_layer(&rootfs_dir).await?;
        let layer_digest = format!("sha256:{}", hex_digest(&layer_blob));
        let layer_size = layer_blob.len() as i64;
        debug!(
            digest = %layer_digest,
            size = layer_size,
            "layer created"
        );

        // 2. Build OCI image config
        let oci_config = build_oci_config(&sandbox_config, &layer_digest, layer_size);
        let config_blob = serde_json::to_vec(&oci_config).map_err(|e| {
            BuildError::IoError(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("failed to serialize OCI config: {e}"),
            ))
        })?;
        let config_digest = format!("sha256:{}", hex_digest(&config_blob));
        let config_size = config_blob.len() as i64;

        // 3. Build OCI manifest
        let manifest = OciImageManifest {
            schema_version: 2,
            media_type: Some("application/vnd.oci.image.manifest.v1+json".to_string()),
            config: OciDescriptor {
                media_type: "application/vnd.oci.image.config.v1+json".to_string(),
                digest: config_digest.clone(),
                size: config_size,
                urls: None,
                annotations: None,
            },
            layers: vec![OciDescriptor {
                media_type: "application/vnd.oci.image.layer.v1.tar+gzip".to_string(),
                digest: layer_digest.clone(),
                size: layer_size,
                urls: None,
                annotations: None,
            }],
            subject: None,
            artifact_type: None,
            annotations: None,
        };

        // 4. Serialize manifest and compute digest
        let manifest_bytes = serde_json::to_vec(&manifest).map_err(|e| {
            BuildError::IoError(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("failed to serialize OCI manifest: {e}"),
            ))
        })?;
        let manifest_digest = format!("sha256:{}", hex_digest(&manifest_bytes));
        let manifest_size = manifest_bytes.len() as i64;

        Ok(PreparedImage {
            layer_blob,
            layer_digest,
            layer_size,
            config_blob,
            config_digest,
            config_size,
            manifest_bytes,
            manifest_digest,
            manifest_size,
        })
    }

    /// Push a sandbox-built image to a remote OCI registry.
    ///
    /// Converts the rootfs directory into a single gzipped tar layer, builds an
    /// OCI image config and manifest, then pushes everything via the registry client.
    pub(super) async fn push_image(
        data_dir: &Path,
        tag: &str,
        auth: Option<&RegistryAuth>,
    ) -> Result<()> {
        let prepared = prepare_image_for_push(data_dir, tag).await?;

        // Resolve auth
        let oci_auth = resolve_auth(tag, auth)?;

        // Push via registry client
        let cache = BlobCache::new();
        let puller = ImagePuller::new(cache);

        info!("Pushing layer {} to {}", prepared.layer_digest, tag);
        puller
            .push_blob(
                tag,
                &prepared.layer_digest,
                &prepared.layer_blob,
                "application/vnd.oci.image.layer.v1.tar+gzip",
                &oci_auth,
            )
            .await
            .map_err(|e| {
                BuildError::IoError(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("failed to push layer: {e}"),
                ))
            })?;

        info!("Pushing config {} to {}", prepared.config_digest, tag);
        puller
            .push_blob(
                tag,
                &prepared.config_digest,
                &prepared.config_blob,
                "application/vnd.oci.image.config.v1+json",
                &oci_auth,
            )
            .await
            .map_err(|e| {
                BuildError::IoError(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("failed to push config: {e}"),
                ))
            })?;

        info!("Pushing manifest for {}", tag);
        let manifest: OciImageManifest =
            serde_json::from_slice(&prepared.manifest_bytes).map_err(|e| {
                BuildError::IoError(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("failed to deserialize manifest: {e}"),
                ))
            })?;
        puller
            .push_manifest_to_registry(tag, &manifest, &oci_auth)
            .await
            .map_err(|e| {
                BuildError::IoError(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("failed to push manifest: {e}"),
                ))
            })?;

        info!("Successfully pushed {}", tag);
        Ok(())
    }

    /// Create a new, empty manifest list on disk.
    ///
    /// Initialises the manifest directory under `{data_dir}/manifests/{name}/`
    /// with an empty `index.json` and a `blobs/` subdirectory.
    pub(super) async fn manifest_create(data_dir: &Path, name: &str) -> Result<()> {
        let sanitized = crate::sandbox_builder::sanitize_image_name(name);
        let manifest_dir = data_dir.join("manifests").join(sanitized);

        tokio::fs::create_dir_all(manifest_dir.join("blobs"))
            .await
            .map_err(|e| {
                BuildError::IoError(std::io::Error::new(
                    e.kind(),
                    format!("failed to create manifest directory: {e}"),
                ))
            })?;

        let index = SandboxManifestIndex { entries: vec![] };
        let index_json = serde_json::to_vec_pretty(&index).map_err(|e| {
            BuildError::IoError(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("failed to serialize manifest index: {e}"),
            ))
        })?;
        tokio::fs::write(manifest_dir.join("index.json"), &index_json)
            .await
            .map_err(|e| {
                BuildError::IoError(std::io::Error::new(
                    e.kind(),
                    format!("failed to write manifest index.json: {e}"),
                ))
            })?;

        Ok(())
    }

    /// Add an image to an existing manifest list.
    ///
    /// Prepares the image for push, writes all blobs (layer, config, manifest) into
    /// the manifest's `blobs/` directory, and appends an entry to `index.json`.
    pub(super) async fn manifest_add(
        data_dir: &Path,
        manifest_name: &str,
        image_tag: &str,
    ) -> Result<()> {
        let sanitized = crate::sandbox_builder::sanitize_image_name(manifest_name);
        let manifest_dir = data_dir.join("manifests").join(sanitized);
        let index_path = manifest_dir.join("index.json");

        // Read existing index
        let index_bytes = tokio::fs::read(&index_path).await.map_err(|e| {
            BuildError::IoError(std::io::Error::new(
                e.kind(),
                format!("failed to read manifest index.json: {e}"),
            ))
        })?;
        let mut index: SandboxManifestIndex =
            serde_json::from_slice(&index_bytes).map_err(|e| {
                BuildError::IoError(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("failed to deserialize manifest index: {e}"),
                ))
            })?;

        // Prepare the image
        let prepared = prepare_image_for_push(data_dir, image_tag).await?;

        let blobs_dir = manifest_dir.join("blobs");

        // Helper to strip the "sha256:" prefix for filenames
        let strip_prefix = |digest: &str| -> String {
            digest.strip_prefix("sha256:").unwrap_or(digest).to_string()
        };

        // Write blobs to disk
        tokio::fs::write(
            blobs_dir.join(strip_prefix(&prepared.layer_digest)),
            &prepared.layer_blob,
        )
        .await
        .map_err(|e| {
            BuildError::IoError(std::io::Error::new(
                e.kind(),
                format!("failed to write layer blob: {e}"),
            ))
        })?;

        tokio::fs::write(
            blobs_dir.join(strip_prefix(&prepared.config_digest)),
            &prepared.config_blob,
        )
        .await
        .map_err(|e| {
            BuildError::IoError(std::io::Error::new(
                e.kind(),
                format!("failed to write config blob: {e}"),
            ))
        })?;

        tokio::fs::write(
            blobs_dir.join(strip_prefix(&prepared.manifest_digest)),
            &prepared.manifest_bytes,
        )
        .await
        .map_err(|e| {
            BuildError::IoError(std::io::Error::new(
                e.kind(),
                format!("failed to write manifest blob: {e}"),
            ))
        })?;

        // Append entry
        index.entries.push(SandboxManifestEntry {
            image_tag: image_tag.to_string(),
            manifest_digest: prepared.manifest_digest,
            manifest_size: prepared.manifest_size,
            layer_digest: prepared.layer_digest,
            layer_size: prepared.layer_size,
            config_digest: prepared.config_digest,
            config_size: prepared.config_size,
            os: "linux".to_string(),
            architecture: go_arch_name().to_string(),
        });

        // Write updated index
        let updated_json = serde_json::to_vec_pretty(&index).map_err(|e| {
            BuildError::IoError(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("failed to serialize updated manifest index: {e}"),
            ))
        })?;
        tokio::fs::write(&index_path, &updated_json)
            .await
            .map_err(|e| {
                BuildError::IoError(std::io::Error::new(
                    e.kind(),
                    format!("failed to write updated manifest index.json: {e}"),
                ))
            })?;

        Ok(())
    }

    /// Push a manifest list (and all referenced images) to a remote OCI registry.
    ///
    /// Reads the on-disk manifest index produced by [`manifest_create`] /
    /// [`manifest_add`], pushes every layer, config, and per-platform manifest
    /// blob, then uploads an OCI image index pointing at all of them.
    pub(super) async fn manifest_push(
        data_dir: &Path,
        name: &str,
        destination: &str,
        auth: Option<&RegistryAuth>,
    ) -> Result<()> {
        let sanitized = crate::sandbox_builder::sanitize_image_name(name);
        let manifest_dir = data_dir.join("manifests").join(sanitized);
        let index_path = manifest_dir.join("index.json");

        // Read the manifest index
        let index_bytes = tokio::fs::read(&index_path).await.map_err(|e| {
            BuildError::IoError(std::io::Error::new(
                e.kind(),
                format!("failed to read manifest index.json: {e}"),
            ))
        })?;
        let index: SandboxManifestIndex = serde_json::from_slice(&index_bytes).map_err(|e| {
            BuildError::IoError(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("failed to deserialize manifest index: {e}"),
            ))
        })?;

        // Resolve auth against the destination reference
        let oci_auth = resolve_auth(destination, auth)?;

        let cache = BlobCache::new();
        let puller = ImagePuller::new(cache);

        let blobs_dir = manifest_dir.join("blobs");

        // Helper to strip the "sha256:" prefix for filenames
        let strip_prefix = |digest: &str| -> String {
            digest.strip_prefix("sha256:").unwrap_or(digest).to_string()
        };

        for entry in &index.entries {
            // Push layer blob
            let layer_data = tokio::fs::read(blobs_dir.join(strip_prefix(&entry.layer_digest)))
                .await
                .map_err(|e| {
                    BuildError::IoError(std::io::Error::new(
                        e.kind(),
                        format!("failed to read layer blob {}: {e}", entry.layer_digest),
                    ))
                })?;

            info!(
                "Pushing layer {} for {} to {}",
                entry.layer_digest, entry.image_tag, destination
            );
            puller
                .push_blob(
                    destination,
                    &entry.layer_digest,
                    &layer_data,
                    "application/vnd.oci.image.layer.v1.tar+gzip",
                    &oci_auth,
                )
                .await
                .map_err(|e| {
                    BuildError::IoError(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("failed to push layer {}: {e}", entry.layer_digest),
                    ))
                })?;

            // Push config blob
            let config_data = tokio::fs::read(blobs_dir.join(strip_prefix(&entry.config_digest)))
                .await
                .map_err(|e| {
                    BuildError::IoError(std::io::Error::new(
                        e.kind(),
                        format!("failed to read config blob {}: {e}", entry.config_digest),
                    ))
                })?;

            info!(
                "Pushing config {} for {} to {}",
                entry.config_digest, entry.image_tag, destination
            );
            puller
                .push_blob(
                    destination,
                    &entry.config_digest,
                    &config_data,
                    "application/vnd.oci.image.config.v1+json",
                    &oci_auth,
                )
                .await
                .map_err(|e| {
                    BuildError::IoError(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("failed to push config {}: {e}", entry.config_digest),
                    ))
                })?;

            // Push manifest blob
            let manifest_data =
                tokio::fs::read(blobs_dir.join(strip_prefix(&entry.manifest_digest)))
                    .await
                    .map_err(|e| {
                        BuildError::IoError(std::io::Error::new(
                            e.kind(),
                            format!(
                                "failed to read manifest blob {}: {e}",
                                entry.manifest_digest
                            ),
                        ))
                    })?;

            let manifest: OciImageManifest =
                serde_json::from_slice(&manifest_data).map_err(|e| {
                    BuildError::IoError(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "failed to deserialize manifest {}: {e}",
                            entry.manifest_digest
                        ),
                    ))
                })?;

            info!(
                "Pushing manifest {} for {} to {}",
                entry.manifest_digest, entry.image_tag, destination
            );
            puller
                .push_manifest_to_registry(destination, &manifest, &oci_auth)
                .await
                .map_err(|e| {
                    BuildError::IoError(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("failed to push manifest {}: {e}", entry.manifest_digest),
                    ))
                })?;
        }

        // Build and push the image index
        let image_index = OciImageIndex {
            schema_version: 2,
            media_type: Some("application/vnd.oci.image.index.v1+json".to_string()),
            manifests: index
                .entries
                .iter()
                .map(|e| ImageIndexEntry {
                    media_type: "application/vnd.oci.image.manifest.v1+json".to_string(),
                    digest: e.manifest_digest.clone(),
                    size: e.manifest_size,
                    platform: Some(Platform {
                        architecture: e.architecture.clone(),
                        os: e.os.clone(),
                        os_version: None,
                        os_features: None,
                        variant: None,
                        features: None,
                    }),
                    annotations: None,
                })
                .collect(),
            artifact_type: None,
            annotations: None,
        };

        info!("Pushing image index to {}", destination);
        puller
            .push_image_index_to_registry(destination, &image_index, &oci_auth)
            .await
            .map_err(|e| {
                BuildError::IoError(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("failed to push image index: {e}"),
                ))
            })?;

        info!("Successfully pushed manifest {} to {}", name, destination);
        Ok(())
    }

    /// Map the Rust `std::env::consts::ARCH` value to the Go/OCI architecture name.
    fn go_arch_name() -> &'static str {
        match std::env::consts::ARCH {
            "aarch64" => "arm64",
            "x86_64" => "amd64",
            other => other,
        }
    }

    /// Create a gzipped tar archive of the rootfs directory.
    async fn create_tar_gz_layer(rootfs_dir: &Path) -> Result<Vec<u8>> {
        let rootfs = rootfs_dir.to_path_buf();
        tokio::task::spawn_blocking(move || {
            let mut buf = Vec::new();
            {
                let encoder = GzEncoder::new(&mut buf, Compression::default());
                let mut archive = Builder::new(encoder);
                archive.append_dir_all(".", &rootfs).map_err(|e| {
                    BuildError::IoError(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("failed to create tar archive: {e}"),
                    ))
                })?;
                let encoder = archive.into_inner().map_err(|e| {
                    BuildError::IoError(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("failed to finalize tar archive: {e}"),
                    ))
                })?;
                encoder.finish().map_err(|e| {
                    BuildError::IoError(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("failed to finish gzip: {e}"),
                    ))
                })?;
            }
            Ok(buf)
        })
        .await
        .map_err(|e| {
            BuildError::IoError(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("layer creation task failed: {e}"),
            ))
        })?
    }

    /// Build an OCI image config from our sandbox config.
    fn build_oci_config(
        sandbox_config: &SandboxImageConfig,
        layer_digest: &str,
        layer_size: i64,
    ) -> serde_json::Value {
        let mut config = serde_json::json!({
            "architecture": std::env::consts::ARCH,
            "os": "linux",
            "config": {
                "Env": sandbox_config.env,
                "WorkingDir": sandbox_config.working_dir,
            },
            "rootfs": {
                "type": "layers",
                "diff_ids": [layer_digest],
            },
        });

        let config_obj = config["config"].as_object_mut().unwrap();

        if let Some(ref ep) = sandbox_config.entrypoint {
            config_obj.insert("Entrypoint".to_string(), serde_json::to_value(ep).unwrap());
        }
        if let Some(ref cmd) = sandbox_config.cmd {
            config_obj.insert("Cmd".to_string(), serde_json::to_value(cmd).unwrap());
        }
        if let Some(ref user) = sandbox_config.user {
            config_obj.insert("User".to_string(), serde_json::Value::String(user.clone()));
        }
        if !sandbox_config.exposed_ports.is_empty() {
            config_obj.insert(
                "ExposedPorts".to_string(),
                serde_json::to_value(&sandbox_config.exposed_ports).unwrap(),
            );
        }
        if !sandbox_config.labels.is_empty() {
            config_obj.insert(
                "Labels".to_string(),
                serde_json::to_value(&sandbox_config.labels).unwrap(),
            );
        }
        if !sandbox_config.volumes.is_empty() {
            let vols: std::collections::HashMap<&str, serde_json::Value> = sandbox_config
                .volumes
                .iter()
                .map(|v| (v.as_str(), serde_json::json!({})))
                .collect();
            config_obj.insert("Volumes".to_string(), serde_json::to_value(&vols).unwrap());
        }

        let _ = layer_size; // available for future use in history entries

        config
    }

    /// Compute the hex-encoded SHA-256 digest of data.
    fn hex_digest(data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        format!("{:x}", hasher.finalize())
    }

    /// Resolve OCI registry auth from the provided credentials or Docker config.
    fn resolve_auth(tag: &str, auth: Option<&RegistryAuth>) -> Result<OciRegistryAuth> {
        // Use explicitly provided credentials first
        if let Some(auth) = auth {
            return Ok(OciRegistryAuth::Basic(
                auth.username.clone(),
                auth.password.clone(),
            ));
        }

        // Fall back to Docker config
        if let Ok(docker_config) = DockerConfigAuth::load() {
            // Extract registry hostname from the tag
            let registry = extract_registry(tag);
            if let Some((username, password)) = docker_config.get_credentials(&registry) {
                return Ok(OciRegistryAuth::Basic(username, password));
            }
        }

        // No auth — try anonymous
        Ok(OciRegistryAuth::Anonymous)
    }

    /// Extract the registry hostname from an image reference.
    ///
    /// e.g. `ghcr.io/foo/bar:latest` → `ghcr.io`
    fn extract_registry(tag: &str) -> String {
        // Strip tag/digest
        let without_tag = tag.split(':').next().unwrap_or(tag);
        let without_digest = without_tag.split('@').next().unwrap_or(without_tag);

        // First path component is the registry if it contains a dot or colon
        if let Some(first) = without_digest.split('/').next() {
            if first.contains('.') || first.contains(':') {
                return first.to_string();
            }
        }

        // Default to Docker Hub
        "docker.io".to_string()
    }
}

#[cfg(not(feature = "cache"))]
mod sandbox_push {
    use super::*;

    pub(super) async fn push_image(
        _data_dir: &Path,
        _tag: &str,
        _auth: Option<&RegistryAuth>,
    ) -> Result<()> {
        Err(BuildError::NotSupported {
            operation: "push".into(),
        })
    }

    pub(super) async fn manifest_create(_data_dir: &Path, _name: &str) -> Result<()> {
        Err(BuildError::NotSupported {
            operation: "manifest_create".into(),
        })
    }

    pub(super) async fn manifest_add(
        _data_dir: &Path,
        _manifest_name: &str,
        _image_tag: &str,
    ) -> Result<()> {
        Err(BuildError::NotSupported {
            operation: "manifest_add".into(),
        })
    }

    pub(super) async fn manifest_push(
        _data_dir: &Path,
        _name: &str,
        _destination: &str,
        _auth: Option<&RegistryAuth>,
    ) -> Result<()> {
        Err(BuildError::NotSupported {
            operation: "manifest_push".into(),
        })
    }
}
