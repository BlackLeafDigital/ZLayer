//! macOS Seatbelt sandbox image builder
//!
//! This module provides a native macOS image builder that uses the Seatbelt sandbox
//! instead of buildah. It is used as a fallback when buildah is not available on macOS.
//!
//! ## Build Flow
//!
//! 1. Parse Dockerfile (reuses existing parser)
//! 2. Pull base image from registry and extract layers to a rootfs directory
//! 3. For each `RUN` instruction: fork a sandboxed child process that executes
//!    the configured shell command inside the rootfs
//! 4. For `COPY`/`ADD`: copy files from the build context into the rootfs
//! 5. For `ENV`/`WORKDIR`/`ENTRYPOINT`/`CMD`/`EXPOSE`: track as OCI config metadata
//! 6. Save the final rootfs + config JSON to the images directory
//!
//! ## Output Format
//!
//! The output is a raw rootfs directory plus a `config.json` file, matching the
//! layout used by the macOS sandbox runtime in `zlayer-agent`:
//!
//! ```text
//! {data_dir}/images/{sanitized_tag}/
//!   rootfs/          -- filesystem contents
//!   config.json      -- OCI-like image configuration
//! ```
//!
//! This module is only compiled on macOS (`#[cfg(target_os = "macos")]`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tracing::{debug, info, warn};

use crate::dockerfile::{
    expand_variables, AddInstruction, CopyInstruction, Dockerfile, HealthcheckInstruction,
    ImageRef, Instruction, ShellOrExec,
};
use crate::error::{BuildError, Result};
use crate::tui::BuildEvent;

#[cfg(feature = "local-registry")]
use zlayer_registry::LocalRegistry;

// ---------------------------------------------------------------------------
// OCI-like image config (stored as config.json alongside the rootfs)
// ---------------------------------------------------------------------------

/// Image configuration metadata, modeled after the OCI image config spec.
///
/// This is serialized to `config.json` next to the rootfs directory so that
/// the macOS sandbox runtime knows the entrypoint, environment, etc.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SandboxImageConfig {
    /// Environment variables (`KEY=VALUE` pairs).
    pub env: Vec<String>,
    /// Working directory inside the rootfs.
    pub working_dir: String,
    /// Entrypoint command.
    pub entrypoint: Option<Vec<String>>,
    /// Default command arguments.
    pub cmd: Option<Vec<String>>,
    /// Exposed ports (informational).
    pub exposed_ports: HashMap<String, serde_json::Value>,
    /// Image labels.
    pub labels: HashMap<String, String>,
    /// User to run as.
    pub user: Option<String>,
    /// Volumes (informational).
    pub volumes: Vec<String>,
    /// Stop signal.
    pub stop_signal: Option<String>,
    /// Custom shell for RUN instructions (SHELL instruction).
    pub shell: Option<Vec<String>>,
    /// Healthcheck configuration.
    pub healthcheck: Option<SandboxHealthcheck>,
}

/// Healthcheck configuration stored in the image config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxHealthcheck {
    /// Command to execute.
    pub command: Vec<String>,
    /// Interval between checks in seconds.
    pub interval_secs: Option<u64>,
    /// Timeout for each check in seconds.
    pub timeout_secs: Option<u64>,
    /// Start period before first check in seconds.
    pub start_period_secs: Option<u64>,
    /// Number of retries before marking unhealthy.
    pub retries: Option<u32>,
}

// ---------------------------------------------------------------------------
// Seatbelt profile generation for build-time commands
// ---------------------------------------------------------------------------

/// Generate a permissive Seatbelt profile suitable for **build-time** RUN commands.
///
/// This profile is more permissive than the runtime profile because build commands
/// need network access (e.g., `apt-get update`, `pip install`) and write access to
/// the entire rootfs plus common system paths.
fn generate_build_seatbelt_profile(_rootfs_dir: &Path, _tmp_dir: &Path) -> String {
    let mut profile = String::with_capacity(2048);

    // Header
    profile.push_str("(version 1)\n");
    profile.push_str("(deny default)\n\n");

    // Base process rules
    profile.push_str("; --- Base process rules ---\n");
    profile.push_str("(allow process-exec)\n");
    profile.push_str("(allow process-fork)\n");
    profile.push_str("(allow signal (target same-sandbox))\n");
    profile.push_str("(allow process-info* (target self))\n");
    profile.push_str("(allow process-info-pidinfo)\n");
    profile.push_str("(allow process-info-rusage)\n\n");

    // Filesystem (build-time: broad access)
    // RUN instructions use absolute host paths since there is no chroot on
    // macOS.  Build-time FS isolation is not a security goal — the sandbox
    // prevents accidental mach/IPC/keychain abuse, not file writes.
    profile.push_str("; --- Filesystem (build-time: broad access) ---\n");
    profile.push_str("; RUN instructions use absolute host paths since there is no chroot on\n");
    profile.push_str("; macOS.  Build-time FS isolation is not a security goal — the sandbox\n");
    profile.push_str("; prevents accidental mach/IPC/keychain abuse, not file writes.\n");
    profile.push_str("(allow file-read* file-write* file-map-executable)\n");
    profile.push_str("(allow pseudo-tty)\n");
    profile.push_str("(allow file-read* file-write* file-ioctl (literal \"/dev/ptmx\"))\n\n");

    // System info
    profile.push_str("; --- System info ---\n");
    profile.push_str("(allow sysctl-read)\n");
    profile.push_str("(allow system-info)\n\n");

    // Mach IPC (build-time: broad access)
    // Brew, ruby, git, and many macOS tools use XPC / CoreFoundation services.
    // Enumerating every required mach service is fragile; allow all during build.
    profile.push_str("; --- Mach IPC (build-time: broad access) ---\n");
    profile.push_str("(allow mach-lookup)\n\n");

    // Network: full access for build-time operations (package managers, etc.)
    profile.push_str("; --- Network: full access (build-time) ---\n");
    profile.push_str("(allow network-outbound)\n");
    profile.push_str("(allow network-inbound)\n");
    profile.push_str("(allow network-bind)\n");
    profile.push_str("(allow system-socket)\n\n");

    // IPC
    profile.push_str("; --- IPC ---\n");
    profile.push_str("(allow ipc-posix-sem)\n");
    profile.push_str("(allow ipc-posix-shm)\n\n");

    // User preferences (needed by some tools)
    profile.push_str("; --- User preferences ---\n");
    profile.push_str("(allow user-preference-read)\n\n");

    profile
}

// ---------------------------------------------------------------------------
// Sandbox builder
// ---------------------------------------------------------------------------

/// macOS-native image builder using the Seatbelt sandbox.
///
/// This builder creates container images as raw rootfs directories with a
/// `config.json` metadata file. It does NOT require buildah.
pub struct SandboxImageBuilder {
    /// Build context directory (contains Dockerfile and files to COPY).
    context: PathBuf,
    /// Data directory for storing built images.
    data_dir: PathBuf,
    /// Build arguments (ARG values).
    build_args: HashMap<String, String>,
    /// Event sender for TUI progress updates.
    event_tx: Option<std::sync::mpsc::Sender<BuildEvent>>,
    /// Local OCI registry for resolving pipeline-built base images.
    #[cfg(feature = "local-registry")]
    local_registry: Option<LocalRegistry>,
}

impl SandboxImageBuilder {
    /// Create a new sandbox image builder.
    ///
    /// # Arguments
    ///
    /// * `context` - Path to the build context directory
    /// * `data_dir` - Base data directory for storing images (e.g. `~/.zlayer`)
    #[must_use]
    pub fn new(context: PathBuf, data_dir: PathBuf) -> Self {
        Self {
            context,
            data_dir,
            build_args: HashMap::new(),
            event_tx: None,
            #[cfg(feature = "local-registry")]
            local_registry: None,
        }
    }

    /// Set build arguments.
    #[must_use]
    pub fn with_build_args(mut self, args: HashMap<String, String>) -> Self {
        self.build_args = args;
        self
    }

    /// Set the event sender for progress updates.
    #[must_use]
    pub fn with_events(mut self, tx: std::sync::mpsc::Sender<BuildEvent>) -> Self {
        self.event_tx = Some(tx);
        self
    }

    /// Set a local registry for resolving base images from pipeline-built images.
    #[cfg(feature = "local-registry")]
    #[must_use]
    pub fn with_local_registry(mut self, registry: LocalRegistry) -> Self {
        self.local_registry = Some(registry);
        self
    }

    /// Send a build event if the channel is configured.
    fn send_event(&self, event: BuildEvent) {
        if let Some(ref tx) = self.event_tx {
            let _ = tx.send(event);
        }
    }

    /// Build an image from a parsed Dockerfile.
    ///
    /// Supports multi-stage builds. All stages (or up to a target stage) are built
    /// sequentially. `COPY --from=stage` resolves from previously-built stage rootfs
    /// directories.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The Dockerfile has no stages
    /// - The base image cannot be pulled
    /// - A RUN command fails
    /// - File operations fail
    #[allow(clippy::too_many_lines)]
    pub async fn build(
        &self,
        dockerfile: &Dockerfile,
        tags: &[String],
    ) -> Result<SandboxBuildResult> {
        let start_time = std::time::Instant::now();

        if dockerfile.stages.is_empty() {
            return Err(BuildError::InvalidInstruction {
                instruction: "build".to_string(),
                reason: "Dockerfile has no stages".to_string(),
            });
        }

        let tag = tags
            .first()
            .cloned()
            .unwrap_or_else(|| format!("sandbox-build:{}", generate_build_id()));

        let sanitized = sanitize_image_name(&tag);
        let image_dir = self.data_dir.join("images").join(&sanitized);
        let rootfs_dir = image_dir.join("rootfs");
        let tmp_dir = image_dir.join("tmp");

        // Clean up any previous build. Use `rm -rf` because Go's module cache
        // sets files read-only, which prevents tokio::fs::remove_dir_all.
        if image_dir.exists() {
            let status = tokio::process::Command::new("rm")
                .args(["-rf"])
                .arg(&image_dir)
                .status()
                .await?;
            if !status.success() {
                return Err(BuildError::IoError(std::io::Error::other(format!(
                    "failed to clean previous build at {}",
                    image_dir.display()
                ))));
            }
        }

        let home_dir = image_dir.join("home");

        tokio::fs::create_dir_all(&rootfs_dir).await?;
        tokio::fs::create_dir_all(&tmp_dir).await?;
        tokio::fs::create_dir_all(&home_dir).await?;

        // Track stage rootfs directories for COPY --from resolution
        let mut stage_rootfs_map: HashMap<String, PathBuf> = HashMap::new();

        // Build all stages sequentially
        let num_stages = dockerfile.stages.len();
        let mut final_config = SandboxImageConfig {
            working_dir: "/".to_string(),
            ..Default::default()
        };

        for (stage_idx, stage) in dockerfile.stages.iter().enumerate() {
            let is_final_stage = stage_idx == num_stages - 1;

            // Determine rootfs directory for this stage
            let stage_rootfs = if is_final_stage {
                rootfs_dir.clone()
            } else {
                let stage_id = stage.identifier();
                let stage_dir = self
                    .data_dir
                    .join("images")
                    .join(format!("__stage_{sanitized}_{stage_id}"));
                if stage_dir.exists() {
                    // chmod first — Go module cache sets files read-only
                    let _ = tokio::process::Command::new("chmod")
                        .args(["-R", "u+w"])
                        .arg(&stage_dir)
                        .status()
                        .await;
                    let _ = tokio::process::Command::new("rm")
                        .args(["-rf"])
                        .arg(&stage_dir)
                        .status()
                        .await;
                }
                tokio::fs::create_dir_all(stage_dir.join("rootfs")).await?;
                stage_dir.join("rootfs")
            };

            self.send_event(BuildEvent::StageStarted {
                index: stage_idx,
                name: stage.name.clone(),
                base_image: stage.base_image.to_string_ref(),
            });

            // Step 1: Set up the rootfs from the base image
            self.setup_base_image(&stage.base_image, &stage_rootfs)
                .await?;

            // Step 1b: Load base image config (ENV, WORKDIR, USER, etc.)
            let mut config = self
                .load_base_image_config(&stage.base_image)
                .await
                .unwrap_or_else(|e| {
                    debug!("Could not load base image config: {e}");
                    SandboxImageConfig {
                        working_dir: "/".to_string(),
                        ..Default::default()
                    }
                });

            // If this stage uses a toolchain, inject its env vars into the config.
            // Skip if the config already has these vars (e.g., loaded from cached image config.json).
            if let Some(spec) =
                crate::macos_toolchain::detect_toolchain(&stage.base_image.to_string_ref())
            {
                let has_toolchain_env = spec
                    .env
                    .keys()
                    .any(|key| config.env.iter().any(|e| e.starts_with(&format!("{key}="))));
                if !has_toolchain_env {
                    for (key, value) in &spec.env {
                        config.env.push(format!("{key}={value}"));
                    }
                    // Add toolchain PATH dirs to the config's PATH
                    for dir in &spec.path_dirs {
                        let existing_path = config
                            .env
                            .iter()
                            .find(|e| e.starts_with("PATH="))
                            .map(|e| e.strip_prefix("PATH=").unwrap_or("").to_string());
                        if let Some(path) = existing_path {
                            config.env.retain(|e| !e.starts_with("PATH="));
                            config.env.push(format!("PATH={dir}:{path}"));
                        } else {
                            config.env.push(format!("PATH={dir}"));
                        }
                    }
                }
            }

            // Track ARG values for variable expansion
            let mut arg_values = self.build_args.clone();
            for global_arg in &dockerfile.global_args {
                if !arg_values.contains_key(&global_arg.name) {
                    if let Some(ref default) = global_arg.default {
                        arg_values.insert(global_arg.name.clone(), default.clone());
                    }
                }
            }

            // Build env map from base config for variable expansion
            let mut env_values: HashMap<String, String> = HashMap::new();
            for env_entry in &config.env {
                if let Some((k, v)) = env_entry.split_once('=') {
                    env_values.insert(k.to_string(), v.to_string());
                }
            }

            for (inst_idx, instruction) in stage.instructions.iter().enumerate() {
                self.send_event(BuildEvent::InstructionStarted {
                    stage: stage_idx,
                    index: inst_idx,
                    instruction: format!("{instruction:?}"),
                });

                self.execute_instruction(
                    instruction,
                    &stage_rootfs,
                    &tmp_dir,
                    &home_dir,
                    &mut config,
                    &mut arg_values,
                    &mut env_values,
                    &stage_rootfs_map,
                )
                .await?;

                self.send_event(BuildEvent::InstructionComplete {
                    stage: stage_idx,
                    index: inst_idx,
                    cached: false,
                });
            }

            // Register this stage's rootfs for COPY --from resolution
            let stage_id = stage.identifier();
            stage_rootfs_map.insert(stage_id.clone(), stage_rootfs.clone());
            if let Some(ref name) = stage.name {
                stage_rootfs_map.insert(name.clone(), stage_rootfs.clone());
            }
            stage_rootfs_map.insert(stage_idx.to_string(), stage_rootfs.clone());

            self.send_event(BuildEvent::StageComplete { index: stage_idx });

            if is_final_stage {
                final_config = config;
            }
        }

        // Step 3: Write config.json
        let config_path = image_dir.join("config.json");
        let config_json = serde_json::to_string_pretty(&final_config).map_err(|e| {
            BuildError::IoError(std::io::Error::other(format!(
                "failed to serialize image config: {e}"
            )))
        })?;
        tokio::fs::write(&config_path, config_json).await?;

        // Clean up tmp dir, home dir, and intermediate stage directories.
        // chmod first — Go module cache sets files read-only.
        let _ = tokio::process::Command::new("chmod")
            .args(["-R", "u+w"])
            .arg(&home_dir)
            .status()
            .await;
        let _ = tokio::process::Command::new("rm")
            .args(["-rf"])
            .arg(&tmp_dir)
            .arg(&home_dir)
            .status()
            .await;
        for stage_path in stage_rootfs_map.values() {
            if *stage_path != rootfs_dir {
                if let Some(parent) = stage_path.parent() {
                    // chmod first — Go module cache sets files read-only
                    let _ = tokio::process::Command::new("chmod")
                        .args(["-R", "u+w"])
                        .arg(parent)
                        .status()
                        .await;
                    let _ = tokio::process::Command::new("rm")
                        .args(["-rf"])
                        .arg(parent)
                        .status()
                        .await;
                }
            }
        }

        #[allow(clippy::cast_possible_truncation)]
        let build_time_ms = start_time.elapsed().as_millis() as u64;

        self.send_event(BuildEvent::BuildComplete {
            image_id: sanitized.clone(),
        });

        info!(
            "Sandbox build completed in {}ms: {} -> {}",
            build_time_ms,
            tag,
            image_dir.display()
        );

        Ok(SandboxBuildResult {
            image_id: sanitized,
            image_dir,
            rootfs_dir,
            config_path,
            tags: tags.to_vec(),
            build_time_ms,
        })
    }

    /// Set up the rootfs from a base image reference.
    ///
    /// For `scratch`, creates an empty rootfs. For registry images, checks if
    /// we already have the image locally; otherwise pulls via `zlayer-registry`.
    #[allow(clippy::too_many_lines)]
    async fn setup_base_image(&self, image_ref: &ImageRef, rootfs_dir: &Path) -> Result<()> {
        match image_ref {
            ImageRef::Scratch => {
                info!("Using scratch base image (empty rootfs)");
                Ok(())
            }
            ImageRef::Stage(name) => {
                // Look for a previously built stage in the images directory
                let sanitized = sanitize_image_name(name);
                let stage_rootfs = self.data_dir.join("images").join(&sanitized).join("rootfs");

                if stage_rootfs.exists() {
                    info!(
                        "Copying stage '{}' rootfs from {}",
                        name,
                        stage_rootfs.display()
                    );
                    copy_directory_recursive(&stage_rootfs, rootfs_dir).await?;
                    Ok(())
                } else {
                    Err(BuildError::StageNotFound { name: name.clone() })
                }
            }
            ImageRef::Registry { .. } => {
                let raw_ref = image_ref.to_string_ref();

                // macOS 3-tier resolution for known images
                #[cfg(target_os = "macos")]
                if let Some(rewritten) =
                    crate::macos_image_resolver::rewrite_image_for_macos(&raw_ref)
                {
                    info!("macOS image rewrite: {} -> {}", raw_ref, rewritten);
                    let sanitized = sanitize_image_name(&rewritten);
                    let cached_image = self.data_dir.join("images").join(&sanitized);
                    let cached_rootfs = cached_image.join("rootfs");

                    // TIER 1: Local image cache
                    if cached_rootfs.exists() && has_content(&cached_rootfs) {
                        info!("Tier 1: using cached macOS image {}", rewritten);
                        copy_directory_recursive(&cached_rootfs, rootfs_dir).await?;
                        return Ok(());
                    }

                    // TIER 2: Pull from GHCR
                    #[cfg(feature = "cache")]
                    {
                        match crate::macos_image_resolver::try_pull_zlayer_image(
                            &rewritten,
                            &cached_image,
                            rootfs_dir,
                        )
                        .await
                        {
                            Ok(true) => {
                                info!("Tier 2: pulled {} from GHCR", rewritten);
                                return Ok(());
                            }
                            Ok(false) => {
                                info!("Tier 2: {} not on GHCR, building locally", rewritten);
                            }
                            Err(e) => {
                                warn!("Tier 2 error for {}: {}, building locally", rewritten, e);
                            }
                        }
                    }

                    // TIER 3: Build locally
                    if let Some(spec) = crate::macos_toolchain::detect_toolchain(&raw_ref) {
                        info!(
                            "Tier 3: building {} locally ({} {})",
                            rewritten, spec.language, spec.version
                        );
                        let image_dir = crate::macos_image_resolver::build_toolchain_as_image(
                            &spec,
                            &rewritten,
                            &self.data_dir,
                        )
                        .await?;
                        copy_directory_recursive(&image_dir.join("rootfs"), rootfs_dir).await?;
                        return Ok(());
                    }

                    // Base distro (ubuntu/alpine/debian) — just build base rootfs
                    info!("Tier 3: building base image for {}", rewritten);
                    let image_dir =
                        crate::macos_image_resolver::build_base_image(&rewritten, &self.data_dir)
                            .await?;
                    copy_directory_recursive(&image_dir.join("rootfs"), rootfs_dir).await?;
                    return Ok(());
                }

                // --- NOT a rewritable image: existing resolution logic ---
                let qualified = image_ref.qualify();
                let full_ref = qualified.to_string_ref();

                // Check 1: exact qualified name in local cache
                let sanitized = sanitize_image_name(&full_ref);
                let cached_rootfs = self.data_dir.join("images").join(&sanitized).join("rootfs");
                if cached_rootfs.exists() && has_content(&cached_rootfs) {
                    info!(
                        "Using cached base image rootfs: {}",
                        cached_rootfs.display()
                    );
                    copy_directory_recursive(&cached_rootfs, rootfs_dir).await?;
                    return Ok(());
                }

                // Check 2: Local OCI registry (for pipeline-built images)
                #[cfg(feature = "local-registry")]
                if let Some(ref registry) = self.local_registry {
                    // Try the raw (unqualified) reference first
                    let (name, tag) = parse_image_name_tag(&raw_ref);
                    if registry.has_manifest(&name, &tag).await {
                        info!("Found '{}' in local registry, extracting layers", raw_ref);
                        return self
                            .extract_from_local_registry(registry, &name, &tag, rootfs_dir)
                            .await;
                    }
                    // Also try the fully-qualified reference
                    let (qname, qtag) = parse_image_name_tag(&full_ref);
                    if (qname != name || qtag != tag) && registry.has_manifest(&qname, &qtag).await
                    {
                        info!("Found '{}' in local registry, extracting layers", full_ref);
                        return self
                            .extract_from_local_registry(registry, &qname, &qtag, rootfs_dir)
                            .await;
                    }
                }

                // Check 3: unqualified name in local cache (before Docker Hub
                // qualification, e.g. "zlayer/base:latest" → "zlayer_base_latest")
                let raw_sanitized = sanitize_image_name(&raw_ref);
                if raw_sanitized != sanitized {
                    let raw_cached = self
                        .data_dir
                        .join("images")
                        .join(&raw_sanitized)
                        .join("rootfs");
                    if raw_cached.exists() && has_content(&raw_cached) {
                        info!("Using cached base image rootfs: {}", raw_cached.display());
                        copy_directory_recursive(&raw_cached, rootfs_dir).await?;
                        return Ok(());
                    }
                }

                // Check 4: scan local images for a pipeline-built image whose
                // sanitized name ends with the unqualified portion. This handles
                // e.g. "zlayer/base:latest" matching a locally-built image tagged
                // "ghcr.io/blackleafdigital/zlayer/base:latest".
                let images_dir = self.data_dir.join("images");
                if images_dir.exists() {
                    if let Ok(entries) = std::fs::read_dir(&images_dir) {
                        for entry in entries.flatten() {
                            let name = entry.file_name().to_string_lossy().to_string();
                            if name.ends_with(&raw_sanitized) && name != raw_sanitized {
                                let candidate = entry.path().join("rootfs");
                                if candidate.exists() && has_content(&candidate) {
                                    info!(
                                        "Using locally-built image '{}' for base '{}'",
                                        name, raw_ref
                                    );
                                    copy_directory_recursive(&candidate, rootfs_dir).await?;
                                    return Ok(());
                                }
                            }
                        }
                    }
                }

                // Fall back to pulling from registry
                self.pull_and_extract_image(&full_ref, &cached_rootfs, rootfs_dir)
                    .await
            }
        }
    }

    /// Extract an image from the local registry into a rootfs directory.
    #[cfg(feature = "local-registry")]
    async fn extract_from_local_registry(
        &self,
        registry: &LocalRegistry,
        name: &str,
        tag: &str,
        rootfs_dir: &Path,
    ) -> Result<()> {
        use zlayer_registry::LayerUnpacker;

        // Get the manifest
        let manifest_data =
            registry
                .get_manifest(name, tag)
                .await
                .map_err(|e| BuildError::BaseImageNotFound {
                    image: format!("{name}:{tag}: {e}"),
                })?;

        // Parse manifest to get layer digests
        let manifest: serde_json::Value =
            serde_json::from_slice(&manifest_data).map_err(|e| BuildError::BaseImageNotFound {
                image: format!("{name}:{tag}: invalid manifest: {e}"),
            })?;

        let layer_descriptors = manifest
            .get("layers")
            .and_then(|v| v.as_array())
            .ok_or_else(|| BuildError::BaseImageNotFound {
                image: format!("{name}:{tag}: manifest has no layers array"),
            })?;

        // Extract each layer from the registry
        let mut layers = Vec::new();
        for layer_desc in layer_descriptors {
            let digest = layer_desc
                .get("digest")
                .and_then(|v| v.as_str())
                .ok_or_else(|| BuildError::RegistryError {
                    message: format!("layer descriptor missing digest in {name}:{tag}"),
                })?;

            let media_type = layer_desc
                .get("mediaType")
                .and_then(|v| v.as_str())
                .unwrap_or("application/vnd.oci.image.layer.v1.tar+gzip");

            let blob = registry
                .get_blob(digest)
                .await
                .map_err(|e| BuildError::RegistryError {
                    message: format!("failed to get layer blob {digest}: {e}"),
                })?;
            layers.push((blob, media_type.to_string()));
        }

        // Unpack layers to rootfs
        let mut unpacker = LayerUnpacker::new(rootfs_dir.to_path_buf());
        unpacker
            .unpack_layers(&layers)
            .await
            .map_err(|e| BuildError::RegistryError {
                message: format!("failed to unpack layers: {e}"),
            })?;

        Ok(())
    }

    /// Pull a registry image and extract its layers to the rootfs.
    #[cfg(feature = "cache")]
    async fn pull_and_extract_image(
        &self,
        image_ref: &str,
        cached_rootfs: &Path,
        rootfs_dir: &Path,
    ) -> Result<()> {
        use zlayer_registry::{BlobCache, ImagePuller, LayerUnpacker, RegistryAuth};

        info!("Pulling base image from registry: {}", image_ref);
        self.send_event(BuildEvent::Output {
            line: format!("Pulling base image: {image_ref}"),
            is_stderr: false,
        });

        let cache = BlobCache::new().map_err(|e| BuildError::RegistryError {
            message: format!("failed to create blob cache: {e}"),
        })?;
        let puller = ImagePuller::new(cache);
        let auth = RegistryAuth::Anonymous;

        // Pull all layers
        let layers = puller.pull_image(image_ref, &auth).await.map_err(|e| {
            BuildError::BaseImageNotFound {
                image: format!("{image_ref}: {e}"),
            }
        })?;

        info!(
            "Pulled {} layers for {}, extracting to rootfs",
            layers.len(),
            image_ref
        );

        // Extract layers to the cached rootfs location first
        if let Some(parent) = cached_rootfs.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::create_dir_all(cached_rootfs).await?;

        let mut unpacker = LayerUnpacker::new(cached_rootfs.to_path_buf());
        let layer_refs: Vec<(Vec<u8>, String)> = layers;
        unpacker
            .unpack_layers(&layer_refs)
            .await
            .map_err(|e| BuildError::RegistryError {
                message: format!("failed to unpack layers: {e}"),
            })?;

        // Also pull and cache the image config (ENV, WORKDIR, etc.)
        if let Some(parent) = cached_rootfs.parent() {
            match puller.pull_image_config(image_ref, &auth).await {
                Ok(ic) => {
                    if let Ok(json) = serde_json::to_string_pretty(&ic) {
                        let _ = tokio::fs::write(parent.join("image_config.json"), json).await;
                    }
                }
                Err(e) => debug!("Could not pull image config for caching: {e}"),
            }
        }

        // Copy from cache to the actual rootfs
        copy_directory_recursive(cached_rootfs, rootfs_dir).await?;

        info!("Base image extracted successfully: {}", image_ref);
        Ok(())
    }

    /// Fallback when `cache` feature is not enabled -- returns a helpful error.
    #[cfg(not(feature = "cache"))]
    #[allow(clippy::unused_async)]
    async fn pull_and_extract_image(
        &self,
        image_ref: &str,
        _cached_rootfs: &Path,
        _rootfs_dir: &Path,
    ) -> Result<()> {
        Err(BuildError::BaseImageNotFound {
            image: format!(
                "{image_ref} -- registry pull requires the 'cache' feature. \
                 Pre-pull with: zlayer pull {image_ref}"
            ),
        })
    }

    /// Load the base image's OCI config and convert it to a `SandboxImageConfig`.
    ///
    /// Checks for a cached `image_config.json` alongside the rootfs first;
    /// falls back to pulling from the registry when the `cache` feature is
    /// enabled.
    async fn load_base_image_config(&self, image_ref: &ImageRef) -> Result<SandboxImageConfig> {
        match image_ref {
            ImageRef::Scratch => Ok(SandboxImageConfig {
                working_dir: "/".to_string(),
                ..Default::default()
            }),
            ImageRef::Stage { .. } => {
                // For stage references, there's no external config to load —
                // the config is built up from Dockerfile instructions.
                Ok(SandboxImageConfig {
                    working_dir: "/".to_string(),
                    ..Default::default()
                })
            }
            ImageRef::Registry { .. } => {
                let raw_ref = image_ref.to_string_ref();

                // Check rewritten macOS image config first
                #[cfg(target_os = "macos")]
                if let Some(rewritten) =
                    crate::macos_image_resolver::rewrite_image_for_macos(&raw_ref)
                {
                    let sanitized = sanitize_image_name(&rewritten);
                    let config_path = self
                        .data_dir
                        .join("images")
                        .join(&sanitized)
                        .join("config.json");
                    if config_path.exists() {
                        if let Ok(data) = tokio::fs::read_to_string(&config_path).await {
                            if let Ok(config) = serde_json::from_str::<SandboxImageConfig>(&data) {
                                debug!("Loaded config from macOS image: {}", rewritten);
                                return Ok(config);
                            }
                        }
                    }
                }

                let qualified = image_ref.qualify();
                let full_ref = qualified.to_string_ref();

                // Check for cached config (qualified name first, then raw)
                for name in [&full_ref, &raw_ref] {
                    let sanitized = sanitize_image_name(name);
                    let config_path = self
                        .data_dir
                        .join("images")
                        .join(&sanitized)
                        .join("image_config.json");
                    if config_path.exists() {
                        if let Ok(data) = tokio::fs::read_to_string(&config_path).await {
                            if let Ok(ic) =
                                serde_json::from_str::<zlayer_registry::ImageConfig>(&data)
                            {
                                debug!("Loaded base image config from cache: {}", name);
                                return Ok(image_config_to_sandbox(&ic));
                            }
                        }
                    }
                }

                // Try pulling config from the registry
                #[cfg(feature = "cache")]
                {
                    use zlayer_registry::{BlobCache, ImagePuller, RegistryAuth};

                    if let Ok(cache) = BlobCache::new() {
                        let puller = ImagePuller::new(cache);
                        let auth = RegistryAuth::Anonymous;
                        if let Ok(ic) = puller.pull_image_config(&full_ref, &auth).await {
                            debug!("Pulled base image config from registry: {}", full_ref);
                            // Cache it for next time
                            let sanitized = sanitize_image_name(&full_ref);
                            let cache_dir = self.data_dir.join("images").join(&sanitized);
                            if cache_dir.exists() {
                                if let Ok(json) = serde_json::to_string_pretty(&ic) {
                                    let _ =
                                        tokio::fs::write(cache_dir.join("image_config.json"), json)
                                            .await;
                                }
                            }
                            return Ok(image_config_to_sandbox(&ic));
                        }
                    }
                }

                debug!("No base image config found for {}, using defaults", raw_ref);
                Ok(SandboxImageConfig {
                    working_dir: "/".to_string(),
                    ..Default::default()
                })
            }
        }
    }

    /// Execute a single Dockerfile instruction against the rootfs.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn execute_instruction(
        &self,
        instruction: &Instruction,
        rootfs_dir: &Path,
        tmp_dir: &Path,
        home_dir: &Path,
        config: &mut SandboxImageConfig,
        arg_values: &mut HashMap<String, String>,
        env_values: &mut HashMap<String, String>,
        stage_rootfs_map: &HashMap<String, PathBuf>,
    ) -> Result<()> {
        match instruction {
            Instruction::Run(run) => {
                self.execute_run(
                    run, rootfs_dir, tmp_dir, home_dir, config, arg_values, env_values,
                )
                .await
            }
            Instruction::Copy(copy) => {
                self.execute_copy(
                    copy,
                    rootfs_dir,
                    config,
                    arg_values,
                    env_values,
                    stage_rootfs_map,
                )
                .await
            }
            Instruction::Add(add) => {
                self.execute_add(add, rootfs_dir, config, arg_values, env_values)
                    .await
            }
            Instruction::Env(env) => {
                for (key, value) in &env.vars {
                    let expanded_value = substitute_args(value, arg_values, env_values);
                    // Remove any existing entry for this key
                    config.env.retain(|e| !e.starts_with(&format!("{key}=")));
                    config.env.push(format!("{key}={expanded_value}"));
                    env_values.insert(key.clone(), expanded_value);
                }
                Ok(())
            }
            Instruction::Workdir(dir) => {
                let dir = substitute_args(dir.trim(), arg_values, env_values);
                if dir.starts_with('/') {
                    config.working_dir = dir;
                } else {
                    // Relative workdir
                    let current = config.working_dir.clone();
                    config.working_dir = if current.ends_with('/') {
                        format!("{current}{dir}")
                    } else {
                        format!("{current}/{dir}")
                    };
                }
                // Create the directory in the rootfs
                let abs_workdir = rootfs_dir.join(
                    config
                        .working_dir
                        .strip_prefix('/')
                        .unwrap_or(&config.working_dir),
                );
                tokio::fs::create_dir_all(&abs_workdir).await?;
                Ok(())
            }
            Instruction::Entrypoint(cmd) => {
                config.entrypoint = Some(shell_or_exec_to_vec(cmd));
                Ok(())
            }
            Instruction::Cmd(cmd) => {
                config.cmd = Some(shell_or_exec_to_vec(cmd));
                Ok(())
            }
            Instruction::Expose(expose) => {
                let proto = match expose.protocol {
                    crate::dockerfile::ExposeProtocol::Tcp => "tcp",
                    crate::dockerfile::ExposeProtocol::Udp => "udp",
                };
                config
                    .exposed_ports
                    .insert(format!("{}/{proto}", expose.port), serde_json::json!({}));
                Ok(())
            }
            Instruction::Label(labels) => {
                for (key, value) in labels {
                    let expanded = substitute_args(value, arg_values, env_values);
                    config.labels.insert(key.clone(), expanded);
                }
                Ok(())
            }
            Instruction::User(user) => {
                let expanded = substitute_args(user, arg_values, env_values);
                config.user = Some(expanded);
                Ok(())
            }
            Instruction::Volume(paths) => {
                config.volumes.extend(paths.clone());
                Ok(())
            }
            Instruction::Stopsignal(signal) => {
                config.stop_signal = Some(signal.clone());
                Ok(())
            }
            Instruction::Arg(arg) => {
                if !arg_values.contains_key(&arg.name) {
                    if let Some(ref default) = arg.default {
                        let expanded = substitute_args(default, arg_values, env_values);
                        arg_values.insert(arg.name.clone(), expanded);
                    }
                }
                Ok(())
            }
            Instruction::Shell(shell_args) => {
                config.shell = Some(shell_args.clone());
                Ok(())
            }
            Instruction::Healthcheck(hc) => {
                match hc {
                    HealthcheckInstruction::None => {
                        config.healthcheck = None;
                    }
                    HealthcheckInstruction::Check {
                        command,
                        interval,
                        timeout,
                        start_period,
                        retries,
                        ..
                    } => {
                        config.healthcheck = Some(SandboxHealthcheck {
                            command: shell_or_exec_to_vec(command),
                            interval_secs: interval.map(|d| d.as_secs()),
                            timeout_secs: timeout.map(|d| d.as_secs()),
                            start_period_secs: start_period.map(|d| d.as_secs()),
                            retries: *retries,
                        });
                    }
                }
                Ok(())
            }
            Instruction::Onbuild(_) => {
                // ONBUILD triggers are for downstream builds; skip in sandbox builder
                debug!("Skipping ONBUILD instruction");
                Ok(())
            }
        }
    }

    /// Execute a RUN instruction by spawning a sandboxed process.
    #[allow(clippy::too_many_lines, clippy::too_many_arguments)]
    async fn execute_run(
        &self,
        run: &crate::dockerfile::RunInstruction,
        rootfs_dir: &Path,
        tmp_dir: &Path,
        home_dir: &Path,
        config: &SandboxImageConfig,
        arg_values: &HashMap<String, String>,
        env_values: &HashMap<String, String>,
    ) -> Result<()> {
        let command_str = match &run.command {
            ShellOrExec::Shell(s) => substitute_args(s, arg_values, env_values),
            ShellOrExec::Exec(args) => args
                .iter()
                .map(|a| substitute_args(a, arg_values, env_values))
                .collect::<Vec<_>>()
                .join(" "),
        };

        info!("RUN: {}", command_str);

        // Generate the seatbelt profile for this build step
        let profile = generate_build_seatbelt_profile(rootfs_dir, tmp_dir);
        let profile_path = tmp_dir.join("build-sandbox.sb");
        tokio::fs::write(&profile_path, &profile).await?;

        // Build environment variables from the image config
        let mut env_map: HashMap<String, String> = HashMap::new();
        for env_entry in &config.env {
            if let Some((k, v)) = env_entry.split_once('=') {
                env_map.insert(k.to_string(), v.to_string());
            }
        }
        // Build PATH: always include rootfs bin dirs (so binaries installed in
        // the image layer are found) and Homebrew/system paths for macOS host
        // tooling. If the image already defines PATH, prefix it with rootfs
        // equivalents of each component and append macOS host paths.
        {
            let rootfs = rootfs_dir.display();
            let macos_host_paths = "/opt/homebrew/bin:/opt/homebrew/sbin:\
                 /usr/local/bin:/usr/local/sbin:\
                 /usr/bin:/usr/sbin:/bin:/sbin";
            let path_value = if let Some(image_path) = env_map.remove("PATH") {
                // Prefix each image PATH component with the rootfs directory so
                // that binaries from the extracted image layers are found first.
                let rootfs_prefixed: Vec<String> = image_path
                    .split(':')
                    .map(|component| {
                        let stripped = component.strip_prefix('/').unwrap_or(component);
                        format!("{rootfs}/{stripped}")
                    })
                    .collect();
                format!(
                    "{}:{}:{}",
                    rootfs_prefixed.join(":"),
                    image_path,
                    macos_host_paths,
                )
            } else {
                format!(
                    "{rootfs}/usr/local/bin:{rootfs}/usr/bin:{rootfs}/bin:\
                     {rootfs}/usr/sbin:{rootfs}/sbin:\
                     {macos_host_paths}",
                )
            };
            env_map.insert("PATH".to_string(), path_value);
        }
        // Always use the build-scoped home directory. Image configs often set
        // HOME=/root which doesn't exist on macOS and isn't writable.
        env_map.insert("HOME".to_string(), home_dir.display().to_string());

        // If USER is set, inject it as an environment variable for RUN commands
        if let Some(ref user) = config.user {
            env_map
                .entry("USER".to_string())
                .or_insert_with(|| resolve_user_name(user, rootfs_dir));
        }

        // Resolve all absolute-path env values to rootfs-relative paths.
        // Container paths like "/usr/local/go" → "{rootfs}/usr/local/go".
        // Skip PATH (colon-separated, already handled) and HOME (build-scoped).
        let rootfs_str = rootfs_dir.display().to_string();
        let home_str = home_dir.display().to_string();
        let tmp_str = tmp_dir.display().to_string();
        let skip_keys: std::collections::HashSet<&str> =
            ["PATH", "HOME", "USER", "TMPDIR"].into_iter().collect();
        for (key, value) in &mut env_map {
            if skip_keys.contains(key.as_str()) {
                continue;
            }
            if value.starts_with('/')
                && !value.starts_with(&rootfs_str)
                && !value.starts_with(&home_str)
                && !value.starts_with(&tmp_str)
                && !value.starts_with("/opt/homebrew")
                && !value.starts_with("/usr/local/bin")
                && !value.contains(':')
            {
                let resolved = rootfs_dir
                    .join(value.strip_prefix('/').unwrap_or(value))
                    .display()
                    .to_string();
                *value = resolved;
            }
        }

        // Ensure directories exist for resolved path-like env values
        // (GOPATH, CARGO_HOME, JAVA_HOME, etc.)
        for value in env_map.values() {
            if value.starts_with(&rootfs_str) && !value.contains(':') {
                let _ = tokio::fs::create_dir_all(value).await;
            }
        }

        // Execute the command under the sandbox
        // We use sandbox-exec to apply the seatbelt profile
        let shell_cmd = match &run.command {
            ShellOrExec::Shell(s) => substitute_args(s, arg_values, env_values),
            ShellOrExec::Exec(args) => {
                // For exec form, join args with proper quoting
                args.iter()
                    .map(|a| {
                        let expanded = substitute_args(a, arg_values, env_values);
                        if expanded.contains(' ') || expanded.contains('"') {
                            format!("'{expanded}'")
                        } else {
                            expanded
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            }
        };

        // Determine working directory
        let workdir = if config.working_dir.is_empty() || config.working_dir == "/" {
            rootfs_dir.to_path_buf()
        } else {
            rootfs_dir.join(
                config
                    .working_dir
                    .strip_prefix('/')
                    .unwrap_or(&config.working_dir),
            )
        };

        // Ensure workdir exists
        tokio::fs::create_dir_all(&workdir).await?;

        // Determine which shell to use
        let (shell_bin, shell_flag) = if let Some(ref custom_shell) = config.shell {
            if custom_shell.len() >= 2 {
                (custom_shell[0].clone(), custom_shell[1..].to_vec())
            } else if custom_shell.len() == 1 {
                (custom_shell[0].clone(), vec!["-c".to_string()])
            } else {
                ("/bin/sh".to_string(), vec!["-c".to_string()])
            }
        } else {
            ("/bin/sh".to_string(), vec!["-c".to_string()])
        };

        // macOS: intercept package manager installs and fetch Homebrew bottles
        #[cfg(target_os = "macos")]
        {
            let trimmed = shell_cmd.trim();
            let (packages, distro) = extract_package_install_packages(trimmed);
            if !packages.is_empty() {
                let mapped = crate::macos_image_resolver::map_linux_packages(
                    &packages.iter().map(String::as_str).collect::<Vec<_>>(),
                    distro,
                    &self.data_dir.join("cache"),
                )
                .await;
                for (formula, skipped) in &mapped {
                    if *skipped {
                        debug!("Skipping Linux-only package: {}", formula);
                        continue;
                    }
                    info!("Installing {} via Homebrew bottle", formula);
                    match crate::macos_image_resolver::fetch_and_extract_bottle(
                        formula, rootfs_dir, tmp_dir,
                    )
                    .await
                    {
                        Ok(()) => info!("Installed {} successfully", formula),
                        Err(e) => warn!("Failed to install {}: {} (continuing)", formula, e),
                    }
                }
                // Still run the translated command (which will be "true"/no-op)
                // so the RUN instruction completes successfully
            }
        }

        // Translate Linux-specific commands (package managers, user mgmt) to
        // macOS equivalents before execution.
        let translated = crate::macos_compat::translate_linux_command(&shell_cmd);
        if translated.was_modified {
            for msg in &translated.translations {
                info!("macOS compat: {}", msg);
            }
        }
        let final_cmd = &translated.command;

        // Rewrite absolute paths in the command to rootfs-relative paths.
        // This makes `/out/foo` -> `{rootfs}/out/foo` so container paths work
        // without chroot.
        let final_cmd = rewrite_command_paths(final_cmd, rootfs_dir);

        // Pre-create directories for rewritten output paths
        // (e.g., {rootfs}/out/ for `-o /out/task-executor`)
        let rootfs_prefix = rootfs_dir.display().to_string();
        for segment in final_cmd.split_whitespace() {
            if segment.starts_with(&rootfs_prefix) && segment.contains('/') {
                if let Some(parent) = std::path::Path::new(segment).parent() {
                    let _ = tokio::fs::create_dir_all(parent).await;
                }
            }
        }

        // Run through the configured shell (both shell and exec forms use it
        // since we have already assembled shell_cmd as a single string)
        let mut cmd = Command::new("sandbox-exec");
        cmd.arg("-f").arg(&profile_path);
        cmd.arg(&shell_bin);
        for flag in &shell_flag {
            cmd.arg(flag);
        }
        cmd.arg(&final_cmd);

        cmd.current_dir(&workdir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_clear();

        // Apply environment
        for (key, value) in &env_map {
            cmd.env(key, value);
        }

        // Set TMPDIR to our controlled tmp directory
        cmd.env("TMPDIR", tmp_dir);

        let output = cmd.output().await.map_err(|e| {
            BuildError::IoError(std::io::Error::new(
                e.kind(),
                format!("failed to execute sandbox-exec: {e}"),
            ))
        })?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !stdout.is_empty() {
            self.send_event(BuildEvent::Output {
                line: stdout.to_string(),
                is_stderr: false,
            });
        }
        if !stderr.is_empty() {
            self.send_event(BuildEvent::Output {
                line: stderr.to_string(),
                is_stderr: true,
            });
        }

        if !output.status.success() {
            let exit_code = output.status.code().unwrap_or(-1);

            // Detect Linux ELF binaries that can't execute on macOS
            if cfg!(target_os = "macos") && (exit_code == 126 || exit_code == 127) {
                let first_word = command_str.split_whitespace().next().unwrap_or("");
                // Check rootfs for the binary (both absolute and relative to rootfs)
                let check_path = if first_word.starts_with('/') {
                    rootfs_dir.join(first_word.trim_start_matches('/'))
                } else {
                    rootfs_dir.join("usr/bin").join(first_word)
                };
                if check_path.exists() {
                    if let Ok(bytes) = std::fs::read(&check_path) {
                        if bytes.len() >= 4 && bytes[..4] == [0x7f, b'E', b'L', b'F'] {
                            warn!(
                                "Linux ELF binary detected: {} — cannot execute on macOS. \
                                 Use zlayer/ base images (e.g., zlayer/golang, zlayer/rust) \
                                 instead of Alpine/Debian for macOS sandbox builds.",
                                first_word
                            );
                            return Err(BuildError::RunFailed {
                                command: format!(
                                    "{command_str} \
                                     (Linux binary cannot execute on macOS — \
                                     use zlayer/ base images instead of Alpine/Debian)"
                                ),
                                exit_code,
                            });
                        }
                    }
                }
            }

            return Err(BuildError::RunFailed {
                command: if stderr.is_empty() {
                    command_str
                } else {
                    format!("{command_str}\n  stderr: {}", stderr.trim())
                },
                exit_code,
            });
        }

        Ok(())
    }

    /// Execute a COPY instruction by copying files from the build context (or a
    /// previously-built stage) into the rootfs.
    #[allow(clippy::too_many_arguments)]
    async fn execute_copy(
        &self,
        copy: &CopyInstruction,
        rootfs_dir: &Path,
        config: &SandboxImageConfig,
        arg_values: &HashMap<String, String>,
        env_values: &HashMap<String, String>,
        stage_rootfs_map: &HashMap<String, PathBuf>,
    ) -> Result<()> {
        let dest_raw = substitute_args(&copy.destination, arg_values, env_values);
        let dest = resolve_dest_path(rootfs_dir, &config.working_dir, &dest_raw);
        let dest_is_dir = is_dir_destination(&dest_raw, copy.sources.len());

        // Resolve the source root: either a previous stage or the build context
        let source_root = if let Some(ref from) = copy.from {
            // COPY --from=stage: resolve from previously-built stage rootfs
            if let Some(stage_rootfs) = stage_rootfs_map.get(from) {
                stage_rootfs.clone()
            } else {
                return Err(BuildError::StageNotFound { name: from.clone() });
            }
        } else {
            self.context.clone()
        };

        // Create destination: if it's a directory destination, create the dir;
        // otherwise create just the parent directory.
        if dest_is_dir {
            tokio::fs::create_dir_all(&dest).await?;
        } else if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        for source in &copy.sources {
            let expanded_source = substitute_args(source, arg_values, env_values);
            // Strip leading '/' so Path::join doesn't replace the entire path
            let relative_source = expanded_source
                .strip_prefix('/')
                .unwrap_or(&expanded_source);
            let source_path = source_root.join(relative_source);

            if !source_path.exists() {
                return Err(BuildError::ContextRead {
                    path: source_path,
                    source: std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!("COPY source not found: {expanded_source}"),
                    ),
                });
            }

            if source_path.is_dir() {
                copy_directory_recursive(&source_path, &dest).await?;
            } else {
                let target = if dest_is_dir {
                    let file_name = source_path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy();
                    dest.join(file_name.as_ref())
                } else {
                    dest.clone()
                };

                tokio::fs::copy(&source_path, &target).await.map_err(|e| {
                    BuildError::IoError(std::io::Error::new(
                        e.kind(),
                        format!(
                            "failed to copy {} -> {}: {e}",
                            source_path.display(),
                            target.display()
                        ),
                    ))
                })?;
            }
        }

        // Apply --chown and --chmod after copying
        apply_chown_chmod(&dest, copy.chown.as_ref(), copy.chmod.as_ref()).await?;

        Ok(())
    }

    /// Execute an ADD instruction: copies local files (with archive auto-extraction)
    /// or downloads URLs to the destination.
    async fn execute_add(
        &self,
        add: &AddInstruction,
        rootfs_dir: &Path,
        config: &SandboxImageConfig,
        arg_values: &HashMap<String, String>,
        env_values: &HashMap<String, String>,
    ) -> Result<()> {
        let dest_raw = substitute_args(&add.destination, arg_values, env_values);
        let dest = resolve_dest_path(rootfs_dir, &config.working_dir, &dest_raw);
        let dest_is_dir = is_dir_destination(&dest_raw, add.sources.len());

        if dest_is_dir {
            tokio::fs::create_dir_all(&dest).await?;
        } else if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        for source in &add.sources {
            let expanded_source = substitute_args(source, arg_values, env_values);

            // Handle URL sources
            if expanded_source.starts_with("http://") || expanded_source.starts_with("https://") {
                self.download_url_source(&expanded_source, &dest, &dest_raw)
                    .await?;
                continue;
            }

            let relative_source = expanded_source
                .strip_prefix('/')
                .unwrap_or(&expanded_source);
            let source_path = self.context.join(relative_source);

            if !source_path.exists() {
                return Err(BuildError::ContextRead {
                    path: source_path,
                    source: std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!("ADD source not found: {expanded_source}"),
                    ),
                });
            }

            if source_path.is_dir() {
                copy_directory_recursive(&source_path, &dest).await?;
            } else if is_extractable_archive(&source_path) {
                // ADD auto-extracts recognized archives to the dest directory
                tokio::fs::create_dir_all(&dest).await?;
                extract_archive(&source_path, &dest).await?;
            } else {
                let target = if dest_is_dir {
                    let file_name = source_path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy();
                    dest.join(file_name.as_ref())
                } else {
                    dest.clone()
                };

                tokio::fs::copy(&source_path, &target).await.map_err(|e| {
                    BuildError::IoError(std::io::Error::new(
                        e.kind(),
                        format!(
                            "failed to add {} -> {}: {e}",
                            source_path.display(),
                            target.display()
                        ),
                    ))
                })?;
            }
        }

        // Apply --chown and --chmod after adding
        apply_chown_chmod(&dest, add.chown.as_ref(), add.chmod.as_ref()).await?;

        Ok(())
    }

    /// Download a URL source for ADD instruction, with auto-extraction for archives.
    async fn download_url_source(&self, url: &str, dest_dir: &Path, dest_raw: &str) -> Result<()> {
        info!("ADD (URL): {}", url);
        self.send_event(BuildEvent::Output {
            line: format!("Downloading: {url}"),
            is_stderr: false,
        });

        let response = reqwest::get(url).await.map_err(|e| {
            BuildError::IoError(std::io::Error::other(format!(
                "failed to download URL {url}: {e}"
            )))
        })?;

        if !response.status().is_success() {
            return Err(BuildError::IoError(std::io::Error::other(format!(
                "HTTP {} downloading {url}",
                response.status()
            ))));
        }

        let bytes = response.bytes().await.map_err(|e| {
            BuildError::IoError(std::io::Error::other(format!(
                "failed to read response from {url}: {e}"
            )))
        })?;

        // Determine filename from URL
        let file_name = url
            .rsplit('/')
            .next()
            .and_then(|s| s.split('?').next())
            .unwrap_or("download");

        // Check if the downloaded file is an extractable archive
        if is_extractable_archive_name(file_name) {
            // Write to a temp file, then extract
            let tmp_path = dest_dir.join(format!(".tmp_{file_name}"));
            tokio::fs::write(&tmp_path, &bytes).await?;
            let result = extract_archive(&tmp_path, dest_dir).await;
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return result;
        }

        // Not an archive -- save directly
        let target = if dest_raw.ends_with('/') {
            dest_dir.join(file_name)
        } else {
            dest_dir.to_path_buf()
        };

        tokio::fs::write(&target, &bytes).await.map_err(|e| {
            BuildError::IoError(std::io::Error::new(
                e.kind(),
                format!(
                    "failed to write downloaded file to {}: {e}",
                    target.display()
                ),
            ))
        })?;

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Build result
// ---------------------------------------------------------------------------

/// Result of a successful sandbox image build.
#[derive(Debug, Clone)]
pub struct SandboxBuildResult {
    /// Image identifier (sanitized tag).
    pub image_id: String,
    /// Path to the image directory (contains `rootfs/` and `config.json`).
    pub image_dir: PathBuf,
    /// Path to the rootfs directory.
    pub rootfs_dir: PathBuf,
    /// Path to the config.json file.
    pub config_path: PathBuf,
    /// Tags applied to this image.
    pub tags: Vec<String>,
    /// Build duration in milliseconds.
    pub build_time_ms: u64,
}

// ---------------------------------------------------------------------------
// ARG/ENV variable substitution
// ---------------------------------------------------------------------------

/// Convert an OCI `ImageConfig` to the sandbox builder's `SandboxImageConfig`.
fn image_config_to_sandbox(ic: &zlayer_registry::ImageConfig) -> SandboxImageConfig {
    SandboxImageConfig {
        env: ic.env.clone().unwrap_or_default(),
        working_dir: ic.working_dir.clone().unwrap_or_else(|| "/".to_string()),
        entrypoint: ic.entrypoint.clone(),
        cmd: ic.cmd.clone(),
        exposed_ports: ic.exposed_ports.clone().unwrap_or_default(),
        labels: ic.labels.clone().unwrap_or_default(),
        user: ic.user.clone(),
        volumes: ic
            .volumes
            .as_ref()
            .map(|v| v.keys().cloned().collect())
            .unwrap_or_default(),
        stop_signal: ic.stop_signal.clone(),
        shell: ic.shell.clone(),
        healthcheck: ic.healthcheck.as_ref().map(|hc| {
            // OCI spec stores durations in nanoseconds; convert to seconds.
            const NS_PER_SEC: u64 = 1_000_000_000;
            SandboxHealthcheck {
                command: hc.test.clone().unwrap_or_default(),
                interval_secs: hc.interval.map(|ns| ns / NS_PER_SEC),
                timeout_secs: hc.timeout.map(|ns| ns / NS_PER_SEC),
                start_period_secs: hc.start_period.map(|ns| ns / NS_PER_SEC),
                retries: hc.retries,
            }
        }),
    }
}

/// Substitute `${VAR}`, `${VAR:-default}`, and `$VAR` patterns in a string
/// using the current ARG values and ENV values. Delegates to the existing
/// `expand_variables` implementation in the `dockerfile::variable` module.
fn substitute_args(
    input: &str,
    arg_values: &HashMap<String, String>,
    env_values: &HashMap<String, String>,
) -> String {
    expand_variables(input, arg_values, env_values)
}

// ---------------------------------------------------------------------------
// User resolution
// ---------------------------------------------------------------------------

/// Resolve a USER instruction value. If it is a numeric UID, return it as-is.
/// If it is a username, attempt to look it up in the rootfs `/etc/passwd`.
/// Falls back to the original string if resolution fails.
fn resolve_user_name(user: &str, rootfs_dir: &Path) -> String {
    // If numeric, return as-is
    if user.chars().all(|c| c.is_ascii_digit()) {
        return user.to_string();
    }

    // Strip group portion if present (user:group)
    let username = user.split(':').next().unwrap_or(user);

    // Try to resolve from rootfs /etc/passwd
    let passwd_path = rootfs_dir.join("etc/passwd");
    if let Ok(contents) = std::fs::read_to_string(&passwd_path) {
        for line in contents.lines() {
            let fields: Vec<&str> = line.split(':').collect();
            if fields.len() >= 3 && fields[0] == username {
                return username.to_string();
            }
        }
    }

    username.to_string()
}

// ---------------------------------------------------------------------------
// Archive detection and extraction
// ---------------------------------------------------------------------------

/// Check if a file path has an extractable archive extension.
#[allow(clippy::case_sensitive_file_extension_comparisons)]
fn is_extractable_archive(path: &Path) -> bool {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    is_extractable_archive_name(&name)
}

/// Check if a filename string represents an extractable archive.
/// Note: input is already lowercased, so case-insensitive comparison is unnecessary.
#[allow(clippy::case_sensitive_file_extension_comparisons)]
fn is_extractable_archive_name(name: &str) -> bool {
    let name = name.to_lowercase();
    name.ends_with(".tar")
        || name.ends_with(".tar.gz")
        || name.ends_with(".tgz")
        || name.ends_with(".tar.bz2")
        || name.ends_with(".tbz2")
        || name.ends_with(".tar.xz")
        || name.ends_with(".txz")
        || name.ends_with(".zip")
}

/// Extract an archive to the destination directory.
/// Supports tar, tar.gz/tgz, tar.bz2, tar.xz, and zip formats.
async fn extract_archive(archive_path: &Path, dest: &Path) -> Result<()> {
    let name = archive_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();

    let archive_path = archive_path.to_path_buf();
    let dest = dest.to_path_buf();

    #[allow(clippy::case_sensitive_file_extension_comparisons)]
    tokio::task::spawn_blocking(move || {
        if name.ends_with(".zip") {
            extract_zip(&archive_path, &dest)
        } else {
            extract_tar(&archive_path, &dest, &name)
        }
    })
    .await
    .map_err(|e| BuildError::IoError(std::io::Error::other(format!("join error: {e}"))))?
}

/// Extract a tar archive (plain, gzip, bzip2, or xz compressed).
/// Note: `name` is already lowercased, so case-insensitive comparison is unnecessary.
#[allow(clippy::case_sensitive_file_extension_comparisons)]
fn extract_tar(archive_path: &Path, dest: &Path, name: &str) -> Result<()> {
    use std::fs::File;
    use std::io::BufReader;

    let file = File::open(archive_path).map_err(|e| {
        BuildError::IoError(std::io::Error::new(
            e.kind(),
            format!("failed to open archive {}: {e}", archive_path.display()),
        ))
    })?;
    let reader = BufReader::new(file);

    std::fs::create_dir_all(dest)?;

    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        let decoder = flate2::read::GzDecoder::new(reader);
        let mut archive = tar::Archive::new(decoder);
        archive.unpack(dest).map_err(|e| {
            BuildError::IoError(std::io::Error::new(
                e.kind(),
                format!("failed to extract tar.gz archive: {e}"),
            ))
        })?;
    } else if name.ends_with(".tar.bz2") || name.ends_with(".tbz2") {
        let decoder = bzip2::read::BzDecoder::new(reader);
        let mut archive = tar::Archive::new(decoder);
        archive.unpack(dest).map_err(|e| {
            BuildError::IoError(std::io::Error::new(
                e.kind(),
                format!("failed to extract tar.bz2 archive: {e}"),
            ))
        })?;
    } else if name.ends_with(".tar.xz") || name.ends_with(".txz") {
        let decoder = xz2::read::XzDecoder::new(reader);
        let mut archive = tar::Archive::new(decoder);
        archive.unpack(dest).map_err(|e| {
            BuildError::IoError(std::io::Error::new(
                e.kind(),
                format!("failed to extract tar.xz archive: {e}"),
            ))
        })?;
    } else {
        // Plain tar
        let mut archive = tar::Archive::new(reader);
        archive.unpack(dest).map_err(|e| {
            BuildError::IoError(std::io::Error::new(
                e.kind(),
                format!("failed to extract tar archive: {e}"),
            ))
        })?;
    }

    Ok(())
}

/// Extract a zip archive.
fn extract_zip(archive_path: &Path, dest: &Path) -> Result<()> {
    use std::fs::File;
    use std::io::Read;

    let file = File::open(archive_path).map_err(|e| {
        BuildError::IoError(std::io::Error::new(
            e.kind(),
            format!("failed to open zip archive {}: {e}", archive_path.display()),
        ))
    })?;

    let mut archive = zip::ZipArchive::new(file).map_err(|e| {
        BuildError::IoError(std::io::Error::other(format!(
            "failed to read zip archive: {e}"
        )))
    })?;

    std::fs::create_dir_all(dest)?;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| {
            BuildError::IoError(std::io::Error::other(format!(
                "failed to read zip entry {i}: {e}"
            )))
        })?;

        let Some(enclosed_name) = entry.enclosed_name() else {
            warn!("Skipping potentially unsafe zip entry");
            continue;
        };
        let out_path = dest.join(enclosed_name);

        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)?;
        } else {
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out_file = File::create(&out_path).map_err(|e| {
                BuildError::IoError(std::io::Error::new(
                    e.kind(),
                    format!("failed to create file {}: {e}", out_path.display()),
                ))
            })?;
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf).map_err(|e| {
                BuildError::IoError(std::io::Error::new(
                    e.kind(),
                    format!("failed to read zip entry: {e}"),
                ))
            })?;
            std::io::Write::write_all(&mut out_file, &buf)?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// chown / chmod helpers
// ---------------------------------------------------------------------------

/// Apply `--chown` and `--chmod` flags to a destination path after copying.
async fn apply_chown_chmod(
    dest: &Path,
    chown: Option<&String>,
    chmod: Option<&String>,
) -> Result<()> {
    #[cfg(unix)]
    {
        if let Some(mode_str) = chmod {
            if let Ok(mode) = u32::from_str_radix(mode_str, 8) {
                apply_permissions_recursive(dest, mode).await?;
            } else {
                warn!("Invalid chmod mode: {}", mode_str);
            }
        }

        if let Some(owner) = chown {
            apply_chown_recursive(dest, owner).await?;
        }

        // Suppress unused warnings when both are None
        let _ = (dest, chown, chmod);
    }

    #[cfg(not(unix))]
    {
        let _ = (dest, chown, chmod);
    }

    Ok(())
}

/// Recursively apply permissions to a path.
#[cfg(unix)]
async fn apply_permissions_recursive(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    if path.is_file() {
        let perms = std::fs::Permissions::from_mode(mode);
        tokio::fs::set_permissions(path, perms).await?;
    } else if path.is_dir() {
        let mut entries = tokio::fs::read_dir(path).await?;
        while let Some(entry) = entries.next_entry().await? {
            let entry_path = entry.path();
            if entry_path.is_dir() {
                Box::pin(apply_permissions_recursive(&entry_path, mode)).await?;
            } else {
                let perms = std::fs::Permissions::from_mode(mode);
                tokio::fs::set_permissions(&entry_path, perms).await?;
            }
        }
    }
    Ok(())
}

/// Recursively apply chown to a path. Parses `user:group` or `uid:gid` format.
#[cfg(unix)]
async fn apply_chown_recursive(path: &Path, owner: &str) -> Result<()> {
    let (uid, gid) = parse_chown(owner);

    let path_owned = path.to_path_buf();
    tokio::task::spawn_blocking(move || chown_recursive_sync(&path_owned, uid, gid))
        .await
        .map_err(|e| BuildError::IoError(std::io::Error::other(format!("join error: {e}"))))?
}

/// Parse a `user:group` or `uid:gid` string into (uid, gid) for chown.
/// Returns `(Option<u32>, Option<u32>)`.
#[cfg(unix)]
fn parse_chown(owner: &str) -> (Option<u32>, Option<u32>) {
    let parts: Vec<&str> = owner.split(':').collect();
    let uid = parts.first().and_then(|s| s.parse::<u32>().ok());
    let gid = parts.get(1).and_then(|s| s.parse::<u32>().ok());
    (uid, gid)
}

/// Synchronous recursive chown.
#[cfg(unix)]
#[allow(clippy::similar_names)]
fn chown_recursive_sync(path: &Path, uid: Option<u32>, gid: Option<u32>) -> Result<()> {
    if uid.is_none() && gid.is_none() {
        return Ok(());
    }

    // Use nix::unistd::chown for proper system call
    let owner_uid = uid.map(nix::unistd::Uid::from_raw);
    let owner_gid = gid.map(nix::unistd::Gid::from_raw);

    nix::unistd::chown(path, owner_uid, owner_gid).map_err(|e| {
        BuildError::IoError(std::io::Error::other(format!(
            "chown failed on {}: {e}",
            path.display()
        )))
    })?;

    if path.is_dir() {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let entry_path = entry.path();
            chown_recursive_sync(&entry_path, uid, gid)?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert a `ShellOrExec` to a `Vec<String>`.
fn shell_or_exec_to_vec(cmd: &ShellOrExec) -> Vec<String> {
    match cmd {
        ShellOrExec::Shell(s) => vec!["/bin/sh".to_string(), "-c".to_string(), s.clone()],
        ShellOrExec::Exec(args) => args.clone(),
    }
}

/// Determine if a COPY/ADD destination refers to a directory.
///
/// A destination is considered a directory when:
/// - It ends with `/`
/// - It is `.` or `..`
/// - There are multiple sources (Docker always treats multi-source dest as dir)
fn is_dir_destination(dest: &str, num_sources: usize) -> bool {
    dest.ends_with('/') || dest == "." || dest == ".." || num_sources > 1
}

/// Resolve a destination path relative to the rootfs and working directory.
fn resolve_dest_path(rootfs_dir: &Path, working_dir: &str, dest: &str) -> PathBuf {
    if dest.starts_with('/') {
        rootfs_dir.join(dest.strip_prefix('/').unwrap_or(dest))
    } else {
        let wd = working_dir.strip_prefix('/').unwrap_or(working_dir);
        rootfs_dir.join(wd).join(dest)
    }
}

/// Sanitize an image name for use as a filesystem directory name.
fn sanitize_image_name(image: &str) -> String {
    image.replace(['/', ':', '@'], "_")
}

/// Parse an image reference into (name, tag) parts.
fn parse_image_name_tag(reference: &str) -> (String, String) {
    if let Some((name, tag)) = reference.rsplit_once(':') {
        (name.to_string(), tag.to_string())
    } else {
        (reference.to_string(), "latest".to_string())
    }
}

/// Generate a short build ID for unique naming.
fn generate_build_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("{ts:x}")
}

/// Check if a directory has any content.
fn has_content(path: &Path) -> bool {
    path.read_dir()
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false)
}

/// Extract package names from a Linux package manager install command.
///
/// Parses commands like:
/// - `apt-get install -y curl git nginx` -> `["curl", "git", "nginx"]`
/// - `apk add --no-cache python3 py3-pip` -> `["python3", "py3-pip"]`
/// - `yum install -y wget` -> `["wget"]`
///
/// Handles compound commands (separated by `&&`), strips `sudo`, and ignores
/// flags (words starting with `-`). Returns an empty vec if the command does
/// not contain a recognized package manager install invocation.
/// Returns `(packages, distro_hint)` — the packages to install and a hint
/// for which Repology distro map to use.
fn extract_package_install_packages(cmd: &str) -> (Vec<String>, &'static str) {
    let mut packages = Vec::new();
    let mut distro = "debian_12"; // default

    for segment in cmd.split("&&") {
        let segment = segment.trim();
        // Strip leading sudo
        let segment = segment
            .strip_prefix("sudo ")
            .unwrap_or(segment)
            .trim_start();

        // Detect package manager install commands and find where the package
        // list begins.
        let tail = if let Some(rest) = segment.strip_prefix("apt-get ") {
            distro = "debian_12";
            find_after_subcommand(rest.trim(), "install")
        } else if let Some(rest) = segment.strip_prefix("apt ") {
            distro = "debian_12";
            find_after_subcommand(rest.trim(), "install")
        } else if let Some(rest) = segment.strip_prefix("apk ") {
            distro = "alpine_3_20";
            find_after_subcommand(rest.trim(), "add")
        } else if let Some(rest) = segment.strip_prefix("yum ") {
            distro = "centos_8";
            find_after_subcommand(rest.trim(), "install")
        } else if let Some(rest) = segment.strip_prefix("dnf ") {
            distro = "fedora_42";
            find_after_subcommand(rest.trim(), "install")
        } else {
            None
        };

        if let Some(args) = tail {
            for word in args.split_whitespace() {
                // Skip flags
                if word.starts_with('-') {
                    continue;
                }
                // Skip redirections and pipes
                if word == "|" || word == ">" || word == ">>" || word == "2>&1" || word == ";" {
                    break;
                }
                packages.push(word.to_string());
            }
        }
    }

    (packages, distro)
}

/// Find the portion of a command string after the given subcommand keyword.
///
/// Skips any leading flags before the subcommand. For example, given
/// `"-y install curl git"` and subcommand `"install"`, returns `Some("curl git")`.
fn find_after_subcommand<'a>(args: &'a str, subcommand: &str) -> Option<&'a str> {
    let mut remaining = args;
    loop {
        let word_end = remaining
            .find(char::is_whitespace)
            .unwrap_or(remaining.len());
        let word = &remaining[..word_end];

        if word == subcommand {
            let after = &remaining[word_end..];
            let trimmed = after.trim_start();
            if trimmed.is_empty() {
                return None;
            }
            return Some(trimmed);
        }

        // If the word is a flag, skip it and continue looking for the subcommand
        if word.starts_with('-') {
            remaining = remaining[word_end..].trim_start();
            if remaining.is_empty() {
                return None;
            }
            continue;
        }

        // Non-flag, non-matching word — not a recognized install command
        return None;
    }
}

/// Recursively copy a directory tree.
async fn copy_directory_recursive(src: &Path, dst: &Path) -> Result<()> {
    tokio::fs::create_dir_all(dst).await?;

    let mut entries = tokio::fs::read_dir(src).await.map_err(|e| {
        BuildError::IoError(std::io::Error::new(
            e.kind(),
            format!("failed to read directory {}: {e}", src.display()),
        ))
    })?;

    while let Some(entry) = entries.next_entry().await? {
        let entry_path = entry.path();
        let file_name = entry.file_name();
        let dest_path = dst.join(&file_name);

        let file_type = entry.file_type().await?;

        if file_type.is_dir() {
            Box::pin(copy_directory_recursive(&entry_path, &dest_path)).await?;
        } else if file_type.is_symlink() {
            let link_target = tokio::fs::read_link(&entry_path).await?;
            // Remove existing if present
            let _ = tokio::fs::remove_file(&dest_path).await;
            #[cfg(unix)]
            tokio::fs::symlink(&link_target, &dest_path)
                .await
                .map_err(|e| {
                    BuildError::IoError(std::io::Error::new(
                        e.kind(),
                        format!(
                            "failed to create symlink {} -> {}: {e}",
                            dest_path.display(),
                            link_target.display()
                        ),
                    ))
                })?;
        } else {
            tokio::fs::copy(&entry_path, &dest_path)
                .await
                .map_err(|e| {
                    BuildError::IoError(std::io::Error::new(
                        e.kind(),
                        format!(
                            "failed to copy {} -> {}: {e}",
                            entry_path.display(),
                            dest_path.display()
                        ),
                    ))
                })?;
        }
    }

    Ok(())
}

/// Rewrite absolute paths in a shell command to rootfs-relative paths.
///
/// Scans for absolute paths (starting with `/`) in the command string and
/// prefixes them with the rootfs directory. Skips paths that are already
/// rootfs-relative, well-known host paths, or inside common prefixes that
/// should remain as-is.
fn rewrite_command_paths(cmd: &str, rootfs_dir: &Path) -> String {
    let rootfs_str = rootfs_dir.display().to_string();

    // Paths to NOT rewrite (host system paths, already-resolved paths)
    let skip_prefixes = [
        &rootfs_str as &str,
        "/dev/",
        "/proc/",
        "/sys/",
        "/bin/",
        "/usr/bin/",
        "/usr/local/bin/",
        "/opt/homebrew/",
        "/tmp/", // macOS tmp
        "/var/",
        "/etc/",
        "//", // protocol-relative URLs
    ];

    let mut result = String::with_capacity(cmd.len() + 128);
    let chars: Vec<char> = cmd.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // Look for absolute paths: a '/' preceded by whitespace, '=', or start of string,
        // followed by a letter (not another /)
        let is_path_start = chars[i] == '/'
            && i + 1 < len
            && chars[i + 1].is_ascii_alphanumeric()
            && (i == 0 || matches!(chars[i - 1], ' ' | '=' | '\t' | '"' | '\''));

        if is_path_start {
            // Extract the full path (until whitespace, quote, or semicolon)
            let path_start = i;
            let mut path_end = i + 1;
            while path_end < len
                && !matches!(
                    chars[path_end],
                    ' ' | '\t' | '"' | '\'' | ';' | ')' | '|' | '&' | '\n'
                )
            {
                path_end += 1;
            }

            let path: String = chars[path_start..path_end].iter().collect();

            // Check if this path should be skipped
            let should_skip = skip_prefixes.iter().any(|prefix| path.starts_with(prefix));

            if should_skip {
                result.push_str(&path);
            } else {
                // Rewrite to rootfs-relative
                let stripped = path.strip_prefix('/').unwrap_or(&path);
                let resolved = format!("{rootfs_str}/{stripped}");
                result.push_str(&resolved);
            }

            i = path_end;
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_image_name() {
        assert_eq!(sanitize_image_name("alpine:3.18"), "alpine_3.18");
        assert_eq!(
            sanitize_image_name("docker.io/library/alpine:latest"),
            "docker.io_library_alpine_latest"
        );
        assert_eq!(
            sanitize_image_name("myapp@sha256:abc123"),
            "myapp_sha256_abc123"
        );
    }

    #[test]
    fn test_resolve_dest_path_absolute() {
        let rootfs = Path::new("/tmp/rootfs");
        let result = resolve_dest_path(rootfs, "/app", "/usr/local/bin");
        assert_eq!(result, PathBuf::from("/tmp/rootfs/usr/local/bin"));
    }

    #[test]
    fn test_resolve_dest_path_relative() {
        let rootfs = Path::new("/tmp/rootfs");
        let result = resolve_dest_path(rootfs, "/app", "src/");
        assert_eq!(result, PathBuf::from("/tmp/rootfs/app/src/"));
    }

    #[test]
    fn test_resolve_dest_path_root_workdir() {
        let rootfs = Path::new("/tmp/rootfs");
        let result = resolve_dest_path(rootfs, "/", "app");
        assert_eq!(result, PathBuf::from("/tmp/rootfs/app"));
    }

    #[test]
    fn test_shell_or_exec_to_vec_shell() {
        let cmd = ShellOrExec::Shell("echo hello".to_string());
        let result = shell_or_exec_to_vec(&cmd);
        assert_eq!(result, vec!["/bin/sh", "-c", "echo hello"]);
    }

    #[test]
    fn test_shell_or_exec_to_vec_exec() {
        let cmd = ShellOrExec::Exec(vec!["echo".to_string(), "hello".to_string()]);
        let result = shell_or_exec_to_vec(&cmd);
        assert_eq!(result, vec!["echo", "hello"]);
    }

    #[test]
    fn test_generate_build_seatbelt_profile() {
        let rootfs = Path::new("/tmp/rootfs");
        let tmp = Path::new("/tmp/build-tmp");
        let profile = generate_build_seatbelt_profile(rootfs, tmp);

        // Verify essential sections are present
        assert!(profile.contains("(version 1)"));
        assert!(profile.contains("(deny default)"));
        assert!(profile.contains("file-read*"));
        assert!(profile.contains("file-write*"));
        assert!(profile.contains("file-map-executable"));
        assert!(profile.contains("network-outbound"));
    }

    #[test]
    fn test_sandbox_image_config_defaults() {
        let config = SandboxImageConfig::default();
        assert!(config.env.is_empty());
        assert!(config.working_dir.is_empty());
        assert!(config.entrypoint.is_none());
        assert!(config.cmd.is_none());
        assert!(config.shell.is_none());
        assert!(config.healthcheck.is_none());
    }

    #[test]
    fn test_sandbox_image_config_serialization() {
        let mut config = SandboxImageConfig::default();
        config.env.push("PATH=/usr/bin".to_string());
        config.working_dir = "/app".to_string();
        config.entrypoint = Some(vec!["./server".to_string()]);
        config
            .labels
            .insert("version".to_string(), "1.0".to_string());
        config.shell = Some(vec![
            "/bin/bash".to_string(),
            "-o".to_string(),
            "pipefail".to_string(),
            "-c".to_string(),
        ]);
        config.healthcheck = Some(SandboxHealthcheck {
            command: vec![
                "CMD-SHELL".to_string(),
                "curl -f http://localhost/ || exit 1".to_string(),
            ],
            interval_secs: Some(30),
            timeout_secs: Some(10),
            start_period_secs: Some(5),
            retries: Some(3),
        });

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: SandboxImageConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.env, config.env);
        assert_eq!(deserialized.working_dir, config.working_dir);
        assert_eq!(deserialized.entrypoint, config.entrypoint);
        assert_eq!(deserialized.labels, config.labels);
        assert_eq!(
            deserialized.shell,
            Some(vec![
                "/bin/bash".to_string(),
                "-o".to_string(),
                "pipefail".to_string(),
                "-c".to_string()
            ])
        );
        assert!(deserialized.healthcheck.is_some());
        let hc = deserialized.healthcheck.unwrap();
        assert_eq!(hc.interval_secs, Some(30));
        assert_eq!(hc.retries, Some(3));
    }

    #[test]
    fn test_substitute_args() {
        let mut args = HashMap::new();
        args.insert("VERSION".to_string(), "1.0".to_string());
        args.insert("BASE".to_string(), "alpine".to_string());
        let env = HashMap::new();

        assert_eq!(substitute_args("$VERSION", &args, &env), "1.0");
        assert_eq!(substitute_args("${VERSION}", &args, &env), "1.0");
        assert_eq!(
            substitute_args("${BASE}:${VERSION}", &args, &env),
            "alpine:1.0"
        );
        assert_eq!(
            substitute_args("${UNSET:-fallback}", &args, &env),
            "fallback"
        );
        assert_eq!(substitute_args("no_vars_here", &args, &env), "no_vars_here");
    }

    #[test]
    fn test_is_extractable_archive() {
        assert!(is_extractable_archive(Path::new("file.tar")));
        assert!(is_extractable_archive(Path::new("file.tar.gz")));
        assert!(is_extractable_archive(Path::new("file.tgz")));
        assert!(is_extractable_archive(Path::new("file.tar.bz2")));
        assert!(is_extractable_archive(Path::new("file.tar.xz")));
        assert!(is_extractable_archive(Path::new("file.zip")));
        assert!(!is_extractable_archive(Path::new("file.txt")));
        assert!(!is_extractable_archive(Path::new("file.rs")));
    }

    #[test]
    fn test_resolve_user_name_numeric() {
        let rootfs = Path::new("/nonexistent");
        assert_eq!(resolve_user_name("1000", rootfs), "1000");
        assert_eq!(resolve_user_name("0", rootfs), "0");
    }

    #[test]
    fn test_resolve_user_name_with_group() {
        let rootfs = Path::new("/nonexistent");
        // Without passwd file, falls back to username portion
        assert_eq!(resolve_user_name("nobody:nogroup", rootfs), "nobody");
    }

    #[cfg(unix)]
    #[test]
    fn test_parse_chown() {
        assert_eq!(parse_chown("1000:1000"), (Some(1000), Some(1000)));
        assert_eq!(parse_chown("0:0"), (Some(0), Some(0)));
        assert_eq!(parse_chown("nobody:nogroup"), (None, None));
        assert_eq!(parse_chown("1000"), (Some(1000), None));
    }

    #[tokio::test]
    async fn test_copy_directory_recursive() {
        let tmp = tempfile::TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");

        // Create source structure
        tokio::fs::create_dir_all(src.join("subdir")).await.unwrap();
        tokio::fs::write(src.join("file.txt"), "hello")
            .await
            .unwrap();
        tokio::fs::write(src.join("subdir/nested.txt"), "world")
            .await
            .unwrap();

        // Copy
        copy_directory_recursive(&src, &dst).await.unwrap();

        // Verify
        assert!(dst.join("file.txt").exists());
        assert!(dst.join("subdir/nested.txt").exists());
        assert_eq!(
            tokio::fs::read_to_string(dst.join("file.txt"))
                .await
                .unwrap(),
            "hello"
        );
        assert_eq!(
            tokio::fs::read_to_string(dst.join("subdir/nested.txt"))
                .await
                .unwrap(),
            "world"
        );
    }

    #[tokio::test]
    async fn test_sandbox_builder_scratch_base() {
        let tmp = tempfile::TempDir::new().unwrap();
        let context_dir = tmp.path().join("context");
        let data_dir = tmp.path().join("data");
        tokio::fs::create_dir_all(&context_dir).await.unwrap();
        tokio::fs::create_dir_all(&data_dir).await.unwrap();

        // Create a simple file in the context
        tokio::fs::write(context_dir.join("hello.txt"), "hello world")
            .await
            .unwrap();

        let dockerfile = Dockerfile::parse(
            r#"
FROM scratch
COPY hello.txt /hello.txt
ENV GREETING=hello
WORKDIR /app
CMD ["cat", "/hello.txt"]
"#,
        )
        .unwrap();

        let builder = SandboxImageBuilder::new(context_dir, data_dir);
        let result = builder
            .build(&dockerfile, &["test:latest".to_string()])
            .await
            .unwrap();

        // Verify the rootfs has the copied file
        assert!(result.rootfs_dir.join("hello.txt").exists());

        // Verify config was written
        let config: SandboxImageConfig = serde_json::from_str(
            &tokio::fs::read_to_string(&result.config_path)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(config.working_dir, "/app");
        assert!(config.env.contains(&"GREETING=hello".to_string()));
        assert_eq!(
            config.cmd,
            Some(vec!["cat".to_string(), "/hello.txt".to_string()])
        );
    }

    #[tokio::test]
    async fn test_multi_stage_build() {
        let tmp = tempfile::TempDir::new().unwrap();
        let context_dir = tmp.path().join("context");
        let data_dir = tmp.path().join("data");
        tokio::fs::create_dir_all(&context_dir).await.unwrap();
        tokio::fs::create_dir_all(&data_dir).await.unwrap();

        // Create a file in the context
        tokio::fs::write(context_dir.join("app.txt"), "built artifact")
            .await
            .unwrap();

        let dockerfile = Dockerfile::parse(
            r#"
FROM scratch AS builder
COPY app.txt /build/app.txt

FROM scratch
COPY --from=builder /build/app.txt /app.txt
CMD ["cat", "/app.txt"]
"#,
        )
        .unwrap();

        let builder = SandboxImageBuilder::new(context_dir, data_dir);
        let result = builder
            .build(&dockerfile, &["multistage-test:latest".to_string()])
            .await
            .unwrap();

        // Verify the final stage has the file from the builder stage
        assert!(result.rootfs_dir.join("app.txt").exists());
        assert_eq!(
            tokio::fs::read_to_string(result.rootfs_dir.join("app.txt"))
                .await
                .unwrap(),
            "built artifact"
        );
    }

    #[tokio::test]
    async fn test_arg_substitution_in_build() {
        let tmp = tempfile::TempDir::new().unwrap();
        let context_dir = tmp.path().join("context");
        let data_dir = tmp.path().join("data");
        tokio::fs::create_dir_all(&context_dir).await.unwrap();
        tokio::fs::create_dir_all(&data_dir).await.unwrap();

        tokio::fs::write(context_dir.join("file.txt"), "content")
            .await
            .unwrap();

        let dockerfile = Dockerfile::parse(
            r"
FROM scratch
ARG MYDIR=target
WORKDIR /${MYDIR}
COPY file.txt .
",
        )
        .unwrap();

        let builder = SandboxImageBuilder::new(context_dir, data_dir);
        let result = builder
            .build(&dockerfile, &["arg-test:latest".to_string()])
            .await
            .unwrap();

        let config: SandboxImageConfig = serde_json::from_str(
            &tokio::fs::read_to_string(&result.config_path)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(config.working_dir, "/target");
    }

    #[tokio::test]
    async fn test_shell_instruction() {
        let tmp = tempfile::TempDir::new().unwrap();
        let context_dir = tmp.path().join("context");
        let data_dir = tmp.path().join("data");
        tokio::fs::create_dir_all(&context_dir).await.unwrap();
        tokio::fs::create_dir_all(&data_dir).await.unwrap();

        let dockerfile = Dockerfile::parse(
            r#"
FROM scratch
SHELL ["/bin/bash", "-o", "pipefail", "-c"]
LABEL shell_test=true
"#,
        )
        .unwrap();

        let builder = SandboxImageBuilder::new(context_dir, data_dir);
        let result = builder
            .build(&dockerfile, &["shell-test:latest".to_string()])
            .await
            .unwrap();

        let config: SandboxImageConfig = serde_json::from_str(
            &tokio::fs::read_to_string(&result.config_path)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            config.shell,
            Some(vec![
                "/bin/bash".to_string(),
                "-o".to_string(),
                "pipefail".to_string(),
                "-c".to_string()
            ])
        );
    }

    #[test]
    fn test_rewrite_command_paths_basic() {
        let rootfs = std::path::Path::new("/build/rootfs");
        assert_eq!(
            rewrite_command_paths("go build -o /out/binary ./cmd/main/", rootfs),
            "go build -o /build/rootfs/out/binary ./cmd/main/"
        );
    }

    #[test]
    fn test_rewrite_command_paths_skips_host() {
        let rootfs = std::path::Path::new("/build/rootfs");
        // /usr/bin/ and /bin/ should NOT be rewritten
        assert_eq!(
            rewrite_command_paths("/usr/bin/env go build", rootfs),
            "/usr/bin/env go build"
        );
    }

    #[test]
    fn test_rewrite_command_paths_env_assignment() {
        let rootfs = std::path::Path::new("/build/rootfs");
        assert_eq!(
            rewrite_command_paths("CGO_ENABLED=0 go build -o /out/app", rootfs),
            "CGO_ENABLED=0 go build -o /build/rootfs/out/app"
        );
    }

    #[test]
    fn test_rewrite_command_paths_no_change() {
        let rootfs = std::path::Path::new("/build/rootfs");
        assert_eq!(
            rewrite_command_paths("echo hello world", rootfs),
            "echo hello world"
        );
    }
}
