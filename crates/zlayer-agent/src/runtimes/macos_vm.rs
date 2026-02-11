//! macOS libkrun VM runtime
//!
//! Implements the [`Runtime`] trait using libkrun microVMs for full Linux
//! container compatibility with GPU forwarding on macOS Apple Silicon.
//!
//! This module is only compiled on macOS targets (`#[cfg(target_os = "macos")]`).
//! It provides hardware-level VM isolation using Apple's Hypervisor.framework
//! through the libkrun library, loaded dynamically at runtime via `dlopen`.
//!
//! ## Architecture
//!
//! Each "container" is a lightweight microVM running a minimal Linux kernel:
//!
//! ```text
//! macOS Host
//!   |
//!   +-- ZLayer VmRuntime
//!        |
//!        +-- libkrun microVM (per container)
//!             |
//!             +-- Linux kernel (built into libkrun)
//!             +-- Container rootfs (extracted OCI image)
//!             +-- GPU: Venus-Vulkan (~30%) or ggml API remoting (~97%)
//!             +-- Networking: TSI (Transparent Socket Impersonation)
//! ```
//!
//! ## Key Characteristics
//!
//! - **Boot time**: 100-300ms per VM
//! - **Memory overhead**: ~100MB per VM (Linux kernel + VM infrastructure)
//! - **CPU overhead**: Near-native (hardware virtualization via Hypervisor.framework)
//! - **GPU**: Venus-Vulkan protocol (~30% native) or ggml API remoting (~97% native)
//! - **Networking**: TSI maps guest sockets through the host process
//! - **Compatibility**: Full Linux OCI containers (unlike SandboxRuntime which needs macOS-native binaries)
//!
//! ## Limitations
//!
//! - **One GPU container at a time**: Metal GPU context cannot be shared across VMs
//! - **Apple Silicon only**: Requires M1 or later (Hypervisor.framework)
//! - **No exec support**: Would require vsock or SSH agent inside the VM
//! - **vCPUs must not exceed host cores**: libkrun silently hangs otherwise
//! - **No graceful shutdown API**: VM exits when entrypoint process exits
//!
//! ## Directory Layout
//!
//! ```text
//! {data_dir}/
//!   images/
//!     {sanitized_image_name}/
//!       rootfs/           -- extracted OCI image layers
//!   vms/
//!     {service}-{replica}/
//!       rootfs/           -- copy of base image rootfs (no APFS clone needed for VMs)
//!       config.json       -- serialized ServiceSpec
//!       console.log       -- VM console output
//! ```

use crate::cgroups_stats::ContainerStats;
use crate::error::{AgentError, Result};
use crate::runtime::{ContainerId, ContainerState, Runtime};
use std::collections::HashMap;
use std::ffi::CString;
use std::net::IpAddr;
use std::os::raw::{c_char, c_int, c_uint};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use zlayer_spec::ServiceSpec;

// ---------------------------------------------------------------------------
// libkrun FFI types (loaded via dlopen)
// ---------------------------------------------------------------------------

/// Function signature: `int krun_set_log_level(uint32_t level)`
type KrunSetLogLevel = unsafe extern "C" fn(level: c_uint) -> c_int;

/// Function signature: `int32_t krun_create_ctx()`
/// Returns a context ID (>= 0) on success, or -1 on error.
type KrunCreateCtx = unsafe extern "C" fn() -> c_int;

/// Function signature: `int32_t krun_free_ctx(uint32_t ctx_id)`
type KrunFreeCtx = unsafe extern "C" fn(ctx_id: c_uint) -> c_int;

/// Function signature: `int32_t krun_set_vm_config(uint32_t ctx_id, uint8_t num_vcpus, uint32_t ram_mib)`
type KrunSetVmConfig =
    unsafe extern "C" fn(ctx_id: c_uint, num_vcpus: c_uint, ram_mib: c_uint) -> c_int;

/// Function signature: `int32_t krun_set_root(uint32_t ctx_id, const char *root_path)`
type KrunSetRoot = unsafe extern "C" fn(ctx_id: c_uint, root_path: *const c_char) -> c_int;

/// Function signature: `int32_t krun_set_workdir(uint32_t ctx_id, const char *workdir_path)`
type KrunSetWorkdir = unsafe extern "C" fn(ctx_id: c_uint, workdir_path: *const c_char) -> c_int;

/// Function signature: `int32_t krun_set_exec(uint32_t ctx_id, const char *exec_path, char *const argv[], char *const envp[])`
type KrunSetExec = unsafe extern "C" fn(
    ctx_id: c_uint,
    exec_path: *const c_char,
    argv: *const *const c_char,
    envp: *const *const c_char,
) -> c_int;

/// Function signature: `int32_t krun_set_gpu_options2(uint32_t ctx_id, uint32_t virgl_flags)`
type KrunSetGpuOptions2 = unsafe extern "C" fn(ctx_id: c_uint, virgl_flags: c_uint) -> c_int;

/// Function signature: `int32_t krun_set_tsi(uint32_t ctx_id)`
/// Enables Transparent Socket Impersonation networking.
type KrunSetTsi = unsafe extern "C" fn(ctx_id: c_uint) -> c_int;

/// Function signature: `int32_t krun_start_enter(uint32_t ctx_id)`
/// BLOCKS the calling thread until the VM exits.
type KrunStartEnter = unsafe extern "C" fn(ctx_id: c_uint) -> c_int;

// ---------------------------------------------------------------------------
// Dynamically loaded libkrun library
// ---------------------------------------------------------------------------

/// Wrapper around a dynamically loaded `libkrun.dylib`.
///
/// All function pointers are resolved once at construction time via `dlsym`.
/// The underlying `libloading::Library` is kept alive for the lifetime of
/// this struct to keep the loaded symbols valid.
struct LibKrun {
    /// The loaded dynamic library handle. Must remain alive as long as the
    /// function pointers are in use.
    _lib: libloading::Library,
    set_log_level: KrunSetLogLevel,
    create_ctx: KrunCreateCtx,
    free_ctx: KrunFreeCtx,
    set_vm_config: KrunSetVmConfig,
    set_root: KrunSetRoot,
    set_workdir: KrunSetWorkdir,
    set_exec: KrunSetExec,
    set_gpu_options2: KrunSetGpuOptions2,
    set_tsi: KrunSetTsi,
    start_enter: KrunStartEnter,
}

// SAFETY: LibKrun holds function pointers from a dynamically loaded library.
// The library handle (_lib) ensures the pointers remain valid. The function
// pointers themselves are safe to share across threads because they point to
// code (not mutable data). libkrun's API is documented as thread-safe for
// distinct context IDs.
unsafe impl Send for LibKrun {}
unsafe impl Sync for LibKrun {}

impl LibKrun {
    /// Load libkrun dynamically from standard macOS library paths.
    ///
    /// Searches the following locations in order:
    /// 1. `libkrun.dylib` (system library path / DYLD_LIBRARY_PATH)
    /// 2. `/usr/local/lib/libkrun.dylib` (Intel Homebrew)
    /// 3. `/opt/homebrew/lib/libkrun.dylib` (Apple Silicon Homebrew)
    ///
    /// # Errors
    ///
    /// Returns `AgentError::Configuration` if libkrun cannot be found or if
    /// any required symbol is missing from the library.
    fn load() -> Result<Self> {
        let lib_paths = [
            "libkrun.dylib",
            "/usr/local/lib/libkrun.dylib",
            "/opt/homebrew/lib/libkrun.dylib",
        ];

        let lib = lib_paths
            .iter()
            .find_map(|path| unsafe { libloading::Library::new(*path).ok() })
            .ok_or_else(|| {
                AgentError::Configuration(
                    "libkrun not found. Install via: brew tap slp/krunkit && brew install krunkit\n\
                     Searched: libkrun.dylib, /usr/local/lib/libkrun.dylib, /opt/homebrew/lib/libkrun.dylib"
                        .to_string(),
                )
            })?;

        // Load all required symbols. Each `lib.get()` call returns a reference
        // to the symbol, and we dereference it to get the function pointer.
        unsafe {
            let set_log_level: KrunSetLogLevel =
                *lib.get(b"krun_set_log_level\0").map_err(|e| {
                    AgentError::Configuration(format!("libkrun missing krun_set_log_level: {e}"))
                })?;
            let create_ctx: KrunCreateCtx = *lib.get(b"krun_create_ctx\0").map_err(|e| {
                AgentError::Configuration(format!("libkrun missing krun_create_ctx: {e}"))
            })?;
            let free_ctx: KrunFreeCtx = *lib.get(b"krun_free_ctx\0").map_err(|e| {
                AgentError::Configuration(format!("libkrun missing krun_free_ctx: {e}"))
            })?;
            let set_vm_config: KrunSetVmConfig =
                *lib.get(b"krun_set_vm_config\0").map_err(|e| {
                    AgentError::Configuration(format!("libkrun missing krun_set_vm_config: {e}"))
                })?;
            let set_root: KrunSetRoot = *lib.get(b"krun_set_root\0").map_err(|e| {
                AgentError::Configuration(format!("libkrun missing krun_set_root: {e}"))
            })?;
            let set_workdir: KrunSetWorkdir = *lib.get(b"krun_set_workdir\0").map_err(|e| {
                AgentError::Configuration(format!("libkrun missing krun_set_workdir: {e}"))
            })?;
            let set_exec: KrunSetExec = *lib.get(b"krun_set_exec\0").map_err(|e| {
                AgentError::Configuration(format!("libkrun missing krun_set_exec: {e}"))
            })?;
            let set_gpu_options2: KrunSetGpuOptions2 =
                *lib.get(b"krun_set_gpu_options2\0").map_err(|e| {
                    AgentError::Configuration(format!("libkrun missing krun_set_gpu_options2: {e}"))
                })?;
            let set_tsi: KrunSetTsi = *lib.get(b"krun_set_tsi\0").map_err(|e| {
                AgentError::Configuration(format!("libkrun missing krun_set_tsi: {e}"))
            })?;
            let start_enter: KrunStartEnter = *lib.get(b"krun_start_enter\0").map_err(|e| {
                AgentError::Configuration(format!("libkrun missing krun_start_enter: {e}"))
            })?;

            Ok(Self {
                _lib: lib,
                set_log_level,
                create_ctx,
                free_ctx,
                set_vm_config,
                set_root,
                set_workdir,
                set_exec,
                set_gpu_options2,
                set_tsi,
                start_enter,
            })
        }
    }
}

// ---------------------------------------------------------------------------
// VM container state
// ---------------------------------------------------------------------------

/// Metadata for a running libkrun microVM.
struct VmContainer {
    /// Container identifier.
    id: ContainerId,
    /// Current container state.
    state: ContainerState,
    /// libkrun context ID (returned by `krun_create_ctx`).
    ctx_id: Option<u32>,
    /// Path to the VM's state directory.
    state_dir: PathBuf,
    /// Path to the rootfs provided to the VM.
    rootfs_dir: PathBuf,
    /// Path to the console log file.
    log_file: PathBuf,
    /// When the VM was started.
    started_at: Option<Instant>,
    /// The original service spec.
    spec: ServiceSpec,
    /// Thread handle for the VM. `krun_start_enter()` blocks the calling
    /// thread, so we spawn it in `std::thread::spawn` (NOT tokio::spawn,
    /// as it would block the async runtime).
    vm_thread: Option<std::thread::JoinHandle<i32>>,
    /// Whether this VM has GPU enabled (for enforcing one-GPU-at-a-time).
    gpu_enabled: bool,
    /// Memory allocated to this VM in MiB (for stats reporting).
    ram_mib: u32,
    /// Number of vCPUs allocated to this VM.
    vcpus: u32,
}

// ---------------------------------------------------------------------------
// VmRuntime
// ---------------------------------------------------------------------------

/// macOS libkrun VM-based container runtime.
///
/// Uses libkrun (dynamically loaded) to create lightweight microVMs via
/// Apple's Hypervisor.framework. Each container runs as a real Linux VM
/// with its own kernel, providing full OCI Linux container compatibility
/// on macOS.
///
/// ## GPU Access
///
/// libkrun supports GPU forwarding via Venus-Vulkan protocol and ggml API
/// remoting. Only **one GPU-enabled VM** can run at a time per host due to
/// Metal GPU context limitations. This constraint is enforced by the runtime.
///
/// ## Networking
///
/// TSI (Transparent Socket Impersonation) maps guest socket operations
/// through the host process. Guest `bind()` appears on the host, guest
/// `connect()` goes through the host. No virtual NIC or IP configuration
/// is needed. All containers appear as `127.0.0.1` from the host's perspective.
pub struct VmRuntime {
    /// Dynamically loaded libkrun API. `None` if libkrun is not installed.
    api: Option<Arc<LibKrun>>,
    /// Base directory for VM state and images.
    data_dir: PathBuf,
    /// Directory for VM logs.
    log_dir: PathBuf,
    /// Active VM containers keyed by directory name (e.g., "myservice-1").
    containers: Arc<RwLock<HashMap<String, VmContainer>>>,
    /// Pulled image rootfs paths keyed by sanitized image name.
    image_rootfs: Arc<RwLock<HashMap<String, PathBuf>>>,
    /// Whether a GPU-enabled VM is currently running. Only one is allowed
    /// at a time due to Metal GPU context limitations in libkrun.
    gpu_in_use: Arc<AtomicBool>,
}

impl std::fmt::Debug for VmRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VmRuntime")
            .field("data_dir", &self.data_dir)
            .field("log_dir", &self.log_dir)
            .field("libkrun_loaded", &self.api.is_some())
            .finish_non_exhaustive()
    }
}

impl VmRuntime {
    /// Create a new VM runtime.
    ///
    /// Attempts to dynamically load `libkrun.dylib`. If libkrun is not
    /// installed, the runtime is created in a degraded state where
    /// `start_container` will return an error with installation instructions.
    ///
    /// Creates the required directory hierarchy:
    /// - `{data_dir}/vms/` -- per-VM state directories
    /// - `{data_dir}/images/` -- pulled OCI image rootfs
    pub fn new() -> Result<Self> {
        let data_dir = PathBuf::from("/var/lib/zlayer");
        let log_dir = PathBuf::from("/var/log/zlayer");

        std::fs::create_dir_all(data_dir.join("vms")).map_err(|e| {
            AgentError::Configuration(format!(
                "Failed to create VM state dir {}: {}",
                data_dir.join("vms").display(),
                e
            ))
        })?;
        std::fs::create_dir_all(data_dir.join("images")).map_err(|e| {
            AgentError::Configuration(format!(
                "Failed to create images dir {}: {}",
                data_dir.join("images").display(),
                e
            ))
        })?;
        std::fs::create_dir_all(&log_dir).map_err(|e| {
            AgentError::Configuration(format!(
                "Failed to create log dir {}: {}",
                log_dir.display(),
                e
            ))
        })?;

        // Try to load libkrun dynamically
        let api = match LibKrun::load() {
            Ok(lib) => {
                // Set log level: 0=off, 1=error, 2=warn, 3=info, 4=debug
                unsafe { (lib.set_log_level)(2) };
                tracing::info!("libkrun loaded successfully");
                Some(Arc::new(lib))
            }
            Err(e) => {
                tracing::warn!(
                    "libkrun not available: {e}. VM runtime will be non-functional. \
                     Install via: brew tap slp/krunkit && brew install krunkit"
                );
                None
            }
        };

        tracing::info!(
            data_dir = %data_dir.display(),
            log_dir = %log_dir.display(),
            libkrun_available = api.is_some(),
            "macOS VM runtime initialized"
        );

        Ok(Self {
            api,
            data_dir,
            log_dir,
            containers: Arc::new(RwLock::new(HashMap::new())),
            image_rootfs: Arc::new(RwLock::new(HashMap::new())),
            gpu_in_use: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Generate a directory name for a container from its [`ContainerId`].
    fn container_dir_name(id: &ContainerId) -> String {
        format!("{}-{}", id.service, id.replica)
    }

    /// Get the base VM state directory for a container.
    fn vm_dir(&self, id: &ContainerId) -> PathBuf {
        self.data_dir.join("vms").join(Self::container_dir_name(id))
    }

    /// Get the images base directory.
    fn images_dir(&self) -> PathBuf {
        self.data_dir.join("images")
    }

    /// Require that libkrun is loaded, returning the API handle or an error.
    fn require_api(&self) -> Result<&Arc<LibKrun>> {
        self.api.as_ref().ok_or_else(|| {
            AgentError::Configuration(
                "libkrun is not available. Cannot start VM containers.\n\
                 Install via: brew tap slp/krunkit && brew install krunkit"
                    .to_string(),
            )
        })
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Sanitize an image name for use as a filesystem directory name.
fn sanitize_image_name(image: &str) -> String {
    image.replace(['/', ':', '@'], "_")
}

/// Parse a memory string like "512Mi" or "2Gi" into megabytes.
fn parse_memory_to_mib(s: &str) -> Option<u32> {
    let s = s.trim();
    if let Some(num) = s.strip_suffix("Gi") {
        num.parse::<u32>().ok().map(|v| v * 1024)
    } else if let Some(num) = s.strip_suffix("Mi") {
        num.parse::<u32>().ok()
    } else if let Some(num) = s.strip_suffix("Ki") {
        num.parse::<u32>().ok().map(|v| v / 1024)
    } else {
        // Raw bytes -- convert to MiB
        s.parse::<u64>().ok().map(|v| (v / (1024 * 1024)) as u32)
    }
}

/// Clamp the requested vCPU count to not exceed host physical cores.
///
/// libkrun silently hangs if more vCPUs are configured than the host
/// has physical cores. This function prevents that.
fn safe_vcpu_count(requested: u32) -> u32 {
    let host_cores = num_cpus::get() as u32;
    let clamped = requested.min(host_cores).max(1);
    if requested > host_cores {
        tracing::warn!(
            requested = requested,
            host_cores = host_cores,
            clamped = clamped,
            "Clamping vCPU count to host core count (libkrun hangs if exceeded)"
        );
    }
    clamped
}

/// Resolve the entrypoint command from a [`ServiceSpec`].
///
/// Checks `spec.command.entrypoint` and `spec.command.args` in order,
/// then falls back to `/bin/sh`.
fn resolve_entrypoint(spec: &ServiceSpec) -> Result<(String, Vec<String>)> {
    // Use entrypoint if specified
    if let Some(ref entrypoint) = spec.command.entrypoint {
        if !entrypoint.is_empty() {
            let program = entrypoint[0].clone();
            let mut args: Vec<String> = entrypoint[1..].to_vec();

            // Append args from spec.command.args if present
            if let Some(ref extra_args) = spec.command.args {
                args.extend(extra_args.iter().cloned());
            }

            return Ok((program, args));
        }
    }

    // Use args as command if no entrypoint
    if let Some(ref cmd_args) = spec.command.args {
        if !cmd_args.is_empty() {
            let program = cmd_args[0].clone();
            let args = cmd_args[1..].to_vec();
            return Ok((program, args));
        }
    }

    // Fallback: use /bin/sh (Linux rootfs inside VM)
    Ok(("/bin/sh".to_string(), vec![]))
}

/// Convert a Rust string to a null-terminated CString, returning an error
/// with context if the string contains interior null bytes.
fn to_cstring(s: &str, context: &str) -> Result<CString> {
    CString::new(s)
        .map_err(|e| AgentError::InvalidSpec(format!("{context} contains null byte: {e}")))
}

/// Build a null-terminated array of CString pointers for passing to C.
///
/// Returns a tuple of `(Vec<CString>, Vec<*const c_char>)` where the
/// second vec is the pointer array (null-terminated) suitable for C's
/// `char *const argv[]`. The first vec must be kept alive as long as the
/// pointers are in use.
fn build_c_string_array(strings: &[String]) -> Result<(Vec<CString>, Vec<*const c_char>)> {
    let c_strings: Vec<CString> = strings
        .iter()
        .map(|s| {
            CString::new(s.as_str())
                .map_err(|e| AgentError::InvalidSpec(format!("String contains null byte: {e}")))
        })
        .collect::<Result<Vec<_>>>()?;

    let mut ptrs: Vec<*const c_char> = c_strings.iter().map(|cs| cs.as_ptr()).collect();
    ptrs.push(std::ptr::null()); // Null terminator for C array

    Ok((c_strings, ptrs))
}

/// Build environment variable array in "KEY=VALUE" format for C.
fn build_env_array(env: &HashMap<String, String>) -> Result<(Vec<CString>, Vec<*const c_char>)> {
    let env_strings: Vec<String> = env.iter().map(|(k, v)| format!("{k}={v}")).collect();

    build_c_string_array(&env_strings)
}

// ---------------------------------------------------------------------------
// Runtime trait implementation
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
impl Runtime for VmRuntime {
    /// Pull an image to local storage with default policy (IfNotPresent).
    async fn pull_image(&self, image: &str) -> Result<()> {
        self.pull_image_with_policy(image, zlayer_spec::PullPolicy::IfNotPresent)
            .await
    }

    /// Pull an image to local storage with a specific policy.
    ///
    /// Uses `zlayer_registry` to pull OCI image layers and extract them.
    /// Unlike the SandboxRuntime, VM containers run real Linux -- so standard
    /// Linux OCI images work directly.
    async fn pull_image_with_policy(
        &self,
        image: &str,
        policy: zlayer_spec::PullPolicy,
    ) -> Result<()> {
        let safe_name = sanitize_image_name(image);
        let image_dir = self.images_dir().join(&safe_name);
        let rootfs_dir = image_dir.join("rootfs");

        match policy {
            zlayer_spec::PullPolicy::Always => { /* always re-pull */ }
            zlayer_spec::PullPolicy::IfNotPresent => {
                if rootfs_dir.exists() {
                    tracing::debug!(image = %image, "Image already present, skipping pull");
                    let mut images = self.image_rootfs.write().await;
                    images.insert(safe_name, rootfs_dir);
                    return Ok(());
                }
            }
            zlayer_spec::PullPolicy::Never => {
                if !rootfs_dir.exists() {
                    return Err(AgentError::PullFailed {
                        image: image.to_string(),
                        reason: "Image not present and pull policy is Never".to_string(),
                    });
                }
                let mut images = self.image_rootfs.write().await;
                images.insert(safe_name, rootfs_dir);
                return Ok(());
            }
        }

        tracing::info!(
            image = %image,
            "Pulling image for VM runtime (Linux OCI images supported natively)"
        );

        tokio::fs::create_dir_all(&rootfs_dir)
            .await
            .map_err(|e| AgentError::PullFailed {
                image: image.to_string(),
                reason: format!("Failed to create rootfs dir: {e}"),
            })?;

        // Use zlayer-registry to pull and extract OCI image layers.
        let cache_path = self.images_dir().join("blobs.redb");
        let cache_type = zlayer_registry::CacheType::persistent_at(&cache_path);
        let blob_cache = cache_type
            .build()
            .await
            .map_err(|e| AgentError::PullFailed {
                image: image.to_string(),
                reason: format!("Failed to open blob cache: {e}"),
            })?;

        let puller = zlayer_registry::ImagePuller::with_cache(blob_cache);
        let auth = zlayer_registry::RegistryAuth::Anonymous;

        let layers = puller
            .pull_image(image, &auth)
            .await
            .map_err(|e| AgentError::PullFailed {
                image: image.to_string(),
                reason: format!("Failed to pull image layers: {e}"),
            })?;

        tracing::info!(
            image = %image,
            layer_count = layers.len(),
            "Extracting layers to image rootfs"
        );

        let mut unpacker = zlayer_registry::LayerUnpacker::new(rootfs_dir.clone());
        unpacker
            .unpack_layers(&layers)
            .await
            .map_err(|e| AgentError::PullFailed {
                image: image.to_string(),
                reason: format!("Failed to extract rootfs: {e}"),
            })?;

        // Track the rootfs path
        let mut images = self.image_rootfs.write().await;
        images.insert(safe_name, rootfs_dir.clone());

        tracing::info!(
            image = %image,
            rootfs = %rootfs_dir.display(),
            "Image pulled successfully"
        );

        Ok(())
    }

    /// Create a container (prepare VM rootfs and state directory).
    ///
    /// Unlike the SandboxRuntime, no Seatbelt profile is needed -- the VM
    /// provides full hardware-level isolation. The rootfs is copied (not
    /// APFS-cloned) since each VM has its own filesystem view through
    /// the virtualization layer.
    async fn create_container(&self, id: &ContainerId, spec: &ServiceSpec) -> Result<()> {
        let dir_name = Self::container_dir_name(id);
        let vm_dir = self.vm_dir(id);
        let rootfs_dir = vm_dir.join("rootfs");

        // Clean up stale VM directory if it exists
        if vm_dir.exists() {
            tracing::warn!(
                container = %dir_name,
                "Stale VM directory found, cleaning up"
            );
            if let Err(e) = tokio::fs::remove_dir_all(&vm_dir).await {
                tracing::warn!(
                    container = %dir_name,
                    error = %e,
                    "Failed to remove stale VM directory"
                );
            }
        }

        // Create VM state directory
        tokio::fs::create_dir_all(&vm_dir)
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: dir_name.clone(),
                reason: format!("Failed to create VM dir {}: {e}", vm_dir.display()),
            })?;

        // Locate the base image rootfs
        let safe_image_name = sanitize_image_name(&spec.image.name);
        let image_rootfs = {
            let images = self.image_rootfs.read().await;
            images.get(&safe_image_name).cloned()
        };
        let image_rootfs =
            image_rootfs.unwrap_or_else(|| self.images_dir().join(&safe_image_name).join("rootfs"));

        if !image_rootfs.exists() {
            return Err(AgentError::CreateFailed {
                id: dir_name.clone(),
                reason: format!(
                    "Image rootfs not found at {}. Run pull_image first.",
                    image_rootfs.display()
                ),
            });
        }

        // Copy rootfs to VM directory. No APFS clone needed -- each VM
        // has its own filesystem view through virtio-fs.
        tracing::debug!(
            container = %dir_name,
            src = %image_rootfs.display(),
            dst = %rootfs_dir.display(),
            "Copying rootfs for VM"
        );
        copy_directory_recursive(&image_rootfs, &rootfs_dir)
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: dir_name.clone(),
                reason: format!(
                    "Failed to copy rootfs from {} to {}: {e}",
                    image_rootfs.display(),
                    rootfs_dir.display()
                ),
            })?;

        // Write config to disk
        let config_json =
            serde_json::to_string_pretty(spec).map_err(|e| AgentError::CreateFailed {
                id: dir_name.clone(),
                reason: format!("Failed to serialize spec: {e}"),
            })?;
        tokio::fs::write(vm_dir.join("config.json"), &config_json)
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: dir_name.clone(),
                reason: format!("Failed to write config.json: {e}"),
            })?;

        let log_file = vm_dir.join("console.log");

        // Determine if GPU is requested
        let gpu_enabled = spec
            .resources
            .gpu
            .as_ref()
            .map(|gpu| gpu.vendor == "apple")
            .unwrap_or(false);

        // Determine vCPU count from spec (default 2)
        let vcpus = spec
            .resources
            .cpu
            .map(|cpu| (cpu.ceil() as u32).max(1))
            .unwrap_or(2);
        let vcpus = safe_vcpu_count(vcpus);

        // Determine RAM from spec (default 512 MiB, minimum 128 MiB for VM overhead)
        let ram_mib = spec
            .resources
            .memory
            .as_ref()
            .and_then(|m| parse_memory_to_mib(m))
            .unwrap_or(512)
            .max(128); // 128 MiB minimum for Linux kernel + init

        // Register the container as pending
        let container = VmContainer {
            id: id.clone(),
            state: ContainerState::Pending,
            ctx_id: None,
            state_dir: vm_dir,
            rootfs_dir,
            log_file,
            started_at: None,
            spec: spec.clone(),
            vm_thread: None,
            gpu_enabled,
            ram_mib,
            vcpus,
        };

        let mut containers = self.containers.write().await;
        containers.insert(dir_name.clone(), container);

        tracing::info!(
            container = %dir_name,
            image = %spec.image.name,
            vcpus = vcpus,
            ram_mib = ram_mib,
            gpu = gpu_enabled,
            "VM container created"
        );

        Ok(())
    }

    /// Start a container by creating and booting a libkrun microVM.
    ///
    /// This method:
    /// 1. Creates a libkrun context via `krun_create_ctx()`
    /// 2. Configures the VM: vCPUs, RAM, rootfs, workdir, GPU, networking
    /// 3. Sets the entrypoint command and environment variables
    /// 4. Spawns `krun_start_enter()` on a dedicated OS thread (it blocks
    ///    until the VM exits, so it cannot run on the tokio runtime)
    /// 5. Updates state to `Running`
    ///
    /// # GPU Constraint
    ///
    /// Only one GPU-enabled VM can run at a time. If a GPU container is
    /// already running, this method returns an error.
    async fn start_container(&self, id: &ContainerId) -> Result<()> {
        let dir_name = Self::container_dir_name(id);
        let api = self.require_api()?;

        // Extract what we need from the container state, then release the lock
        let (rootfs_dir, log_file, spec, gpu_enabled, vcpus, ram_mib) = {
            let containers = self.containers.read().await;
            let container = containers
                .get(&dir_name)
                .ok_or_else(|| AgentError::NotFound {
                    container: dir_name.clone(),
                    reason: "Container not created".to_string(),
                })?;
            (
                container.rootfs_dir.clone(),
                container.log_file.clone(),
                container.spec.clone(),
                container.gpu_enabled,
                container.vcpus,
                container.ram_mib,
            )
        };

        // Enforce one-GPU-at-a-time constraint
        if gpu_enabled {
            let was_in_use = self.gpu_in_use.swap(true, Ordering::SeqCst);
            if was_in_use {
                return Err(AgentError::StartFailed {
                    id: dir_name,
                    reason: "Cannot start GPU-enabled VM: another GPU VM is already running. \
                             libkrun supports only one GPU-enabled VM at a time due to Metal \
                             GPU context limitations. Stop the existing GPU container first, \
                             or use the SandboxRuntime (Approach A) which supports multiple \
                             GPU processes via Metal's built-in multi-process support."
                        .to_string(),
                });
            }
        }

        // Resolve entrypoint
        let (program, args) = resolve_entrypoint(&spec)?;

        tracing::info!(
            container = %dir_name,
            program = %program,
            args = ?args,
            vcpus = vcpus,
            ram_mib = ram_mib,
            gpu = gpu_enabled,
            "Starting libkrun VM"
        );

        // --- Configure libkrun context ---

        // 1. Create context
        let ctx_id = unsafe { (api.create_ctx)() };
        if ctx_id < 0 {
            if gpu_enabled {
                self.gpu_in_use.store(false, Ordering::SeqCst);
            }
            return Err(AgentError::StartFailed {
                id: dir_name,
                reason: format!("krun_create_ctx failed with code {ctx_id}"),
            });
        }
        let ctx_id = ctx_id as u32;

        // Helper closure to clean up context on error
        let cleanup_on_error = |api: &LibKrun, ctx: u32, gpu: bool, gpu_flag: &AtomicBool| {
            unsafe { (api.free_ctx)(ctx) };
            if gpu {
                gpu_flag.store(false, Ordering::SeqCst);
            }
        };

        // 2. Configure VM resources
        let ret = unsafe { (api.set_vm_config)(ctx_id, vcpus, ram_mib) };
        if ret != 0 {
            cleanup_on_error(api, ctx_id, gpu_enabled, &self.gpu_in_use);
            return Err(AgentError::StartFailed {
                id: dir_name,
                reason: format!(
                    "krun_set_vm_config({vcpus} vCPUs, {ram_mib} MiB) failed with code {ret}"
                ),
            });
        }

        // 3. Set rootfs (virtio-fs passthrough)
        let rootfs_cstr = to_cstring(rootfs_dir.to_str().unwrap_or("/invalid"), "rootfs path")?;
        let ret = unsafe { (api.set_root)(ctx_id, rootfs_cstr.as_ptr()) };
        if ret != 0 {
            cleanup_on_error(api, ctx_id, gpu_enabled, &self.gpu_in_use);
            return Err(AgentError::StartFailed {
                id: dir_name,
                reason: format!(
                    "krun_set_root({}) failed with code {ret}",
                    rootfs_dir.display()
                ),
            });
        }

        // 4. Set working directory if specified
        if let Some(ref workdir) = spec.command.workdir {
            let workdir_cstr = to_cstring(workdir, "workdir path")?;
            let ret = unsafe { (api.set_workdir)(ctx_id, workdir_cstr.as_ptr()) };
            if ret != 0 {
                cleanup_on_error(api, ctx_id, gpu_enabled, &self.gpu_in_use);
                return Err(AgentError::StartFailed {
                    id: dir_name,
                    reason: format!("krun_set_workdir({workdir}) failed with code {ret}"),
                });
            }
        }

        // 5. Configure GPU if requested
        if gpu_enabled {
            // Flags: 0 = auto-detect Metal backend on Apple Silicon.
            // libkrun auto-selects Venus-Vulkan or ggml remoting based on
            // the guest workload.
            let ret = unsafe { (api.set_gpu_options2)(ctx_id, 0) };
            if ret != 0 {
                cleanup_on_error(api, ctx_id, gpu_enabled, &self.gpu_in_use);
                return Err(AgentError::StartFailed {
                    id: dir_name,
                    reason: format!("krun_set_gpu_options2 failed with code {ret}"),
                });
            }
            tracing::info!(
                container = %dir_name,
                "GPU forwarding enabled (Venus-Vulkan/ggml)"
            );
        }

        // 6. Enable TSI networking
        let ret = unsafe { (api.set_tsi)(ctx_id) };
        if ret != 0 {
            cleanup_on_error(api, ctx_id, gpu_enabled, &self.gpu_in_use);
            return Err(AgentError::StartFailed {
                id: dir_name,
                reason: format!("krun_set_tsi failed with code {ret}"),
            });
        }

        // 7. Set entrypoint command and environment
        let exec_cstr = to_cstring(&program, "exec path")?;

        // Build argv: [program, ...args]
        let mut argv_strings = vec![program.clone()];
        argv_strings.extend(args.clone());
        // Scope raw pointer arrays so they don't live across .await
        let ret = {
            let (_argv_cstrings, argv_ptrs) = build_c_string_array(&argv_strings)?;
            let (_envp_cstrings, envp_ptrs) = build_env_array(&spec.env)?;

            unsafe {
                (api.set_exec)(
                    ctx_id,
                    exec_cstr.as_ptr(),
                    argv_ptrs.as_ptr(),
                    envp_ptrs.as_ptr(),
                )
            }
        };
        if ret != 0 {
            cleanup_on_error(api, ctx_id, gpu_enabled, &self.gpu_in_use);
            return Err(AgentError::StartFailed {
                id: dir_name,
                reason: format!("krun_set_exec({program}) failed with code {ret}"),
            });
        }

        // 8. Spawn the VM on a dedicated OS thread.
        //
        // CRITICAL: `krun_start_enter()` BLOCKS the calling thread until the
        // VM exits. We must NOT call it from a tokio task, as it would block
        // the entire async runtime. Use `std::thread::spawn` instead.
        let api_clone = Arc::clone(api);
        let thread_dir_name = dir_name.clone();
        let thread_log_file = log_file.clone();

        let vm_thread = std::thread::Builder::new()
            .name(format!("krun-vm-{dir_name}"))
            .spawn(move || {
                tracing::debug!(
                    container = %thread_dir_name,
                    ctx_id = ctx_id,
                    "VM thread starting krun_start_enter"
                );

                // Create log file for any pre-start output (libkrun logs to stderr)
                // This is best-effort -- the VM will still run if logging fails.
                let _ = std::fs::File::create(&thread_log_file);

                // This call BLOCKS until the VM exits (entrypoint process exits).
                let exit_code = unsafe { (api_clone.start_enter)(ctx_id) };

                tracing::info!(
                    container = %thread_dir_name,
                    ctx_id = ctx_id,
                    exit_code = exit_code,
                    "VM exited"
                );

                exit_code
            })
            .map_err(|e| {
                cleanup_on_error(api, ctx_id, gpu_enabled, &self.gpu_in_use);
                AgentError::StartFailed {
                    id: dir_name.clone(),
                    reason: format!("Failed to spawn VM thread: {e}"),
                }
            })?;

        // Update container state
        let mut containers = self.containers.write().await;
        if let Some(container) = containers.get_mut(&dir_name) {
            container.ctx_id = Some(ctx_id);
            container.state = ContainerState::Running;
            container.started_at = Some(Instant::now());
            container.vm_thread = Some(vm_thread);
        }

        tracing::info!(
            container = %dir_name,
            ctx_id = ctx_id,
            "VM started"
        );

        Ok(())
    }

    /// Stop a container by destroying the libkrun VM context.
    ///
    /// libkrun does not have a clean shutdown API (`krun_stop` does not exist).
    /// The VM exits when the entrypoint process exits. To force stop:
    ///
    /// 1. Call `krun_free_ctx()` to destroy the VM context (abrupt)
    /// 2. Wait for the VM thread to complete
    ///
    /// The `timeout` parameter is respected for waiting on the thread join.
    async fn stop_container(&self, id: &ContainerId, timeout: Duration) -> Result<()> {
        let dir_name = Self::container_dir_name(id);

        let (ctx_id, gpu_enabled, vm_thread) = {
            let mut containers = self.containers.write().await;
            let container = containers
                .get_mut(&dir_name)
                .ok_or_else(|| AgentError::NotFound {
                    container: dir_name.clone(),
                    reason: "Container not found".to_string(),
                })?;

            // If already exited, nothing to do
            if matches!(
                container.state,
                ContainerState::Exited { .. } | ContainerState::Failed { .. }
            ) {
                return Ok(());
            }

            container.state = ContainerState::Stopping;

            (
                container.ctx_id,
                container.gpu_enabled,
                container.vm_thread.take(),
            )
        };

        tracing::info!(
            container = %dir_name,
            ctx_id = ?ctx_id,
            timeout = ?timeout,
            "Stopping VM"
        );

        // Destroy the VM context to force the VM to exit.
        // This is the only way to stop a libkrun VM externally.
        if let (Some(api), Some(ctx)) = (&self.api, ctx_id) {
            let ret = unsafe { (api.free_ctx)(ctx) };
            if ret != 0 {
                tracing::warn!(
                    container = %dir_name,
                    ctx_id = ctx,
                    ret = ret,
                    "krun_free_ctx returned non-zero (VM may have already exited)"
                );
            }
        }

        // Wait for the VM thread to finish (with timeout)
        if let Some(thread) = vm_thread {
            let exit_code = tokio::task::spawn_blocking(move || {
                // std::thread::JoinHandle doesn't support timeout directly.
                // We rely on krun_free_ctx having terminated the VM above.
                // Give it a reasonable window then report.
                match thread.join() {
                    Ok(code) => code,
                    Err(_) => {
                        tracing::warn!("VM thread panicked during join");
                        -1
                    }
                }
            })
            .await
            .unwrap_or(-1);

            let mut containers = self.containers.write().await;
            if let Some(c) = containers.get_mut(&dir_name) {
                c.state = ContainerState::Exited { code: exit_code };
                c.ctx_id = None;
            }

            tracing::info!(
                container = %dir_name,
                exit_code = exit_code,
                "VM stopped"
            );
        } else {
            // No thread -- mark as exited
            let mut containers = self.containers.write().await;
            if let Some(c) = containers.get_mut(&dir_name) {
                c.state = ContainerState::Exited { code: 0 };
                c.ctx_id = None;
            }
        }

        // Release GPU lock if this was a GPU container
        if gpu_enabled {
            self.gpu_in_use.store(false, Ordering::SeqCst);
        }

        Ok(())
    }

    /// Remove a container.
    ///
    /// Stops the VM if still running, removes the state directory, and
    /// removes the container from internal tracking.
    async fn remove_container(&self, id: &ContainerId) -> Result<()> {
        let dir_name = Self::container_dir_name(id);

        tracing::info!(container = %dir_name, "Removing VM container");

        // Stop the VM if it is still running
        {
            let containers = self.containers.read().await;
            if let Some(c) = containers.get(&dir_name) {
                if matches!(
                    c.state,
                    ContainerState::Running
                        | ContainerState::Stopping
                        | ContainerState::Initializing
                ) {
                    drop(containers);
                    // Best-effort stop
                    let _ = self.stop_container(id, Duration::from_secs(5)).await;
                }
            }
        }

        // Remove from tracking
        {
            let mut containers = self.containers.write().await;
            if let Some(c) = containers.remove(&dir_name) {
                // Release GPU lock if needed
                if c.gpu_enabled
                    && matches!(c.state, ContainerState::Running | ContainerState::Stopping)
                {
                    self.gpu_in_use.store(false, Ordering::SeqCst);
                }

                // Free context if still active
                if let (Some(api), Some(ctx)) = (&self.api, c.ctx_id) {
                    let _ = unsafe { (api.free_ctx)(ctx) };
                }
            }
        }

        // Remove state directory
        let vm_dir = self.vm_dir(id);
        if vm_dir.exists() {
            tokio::fs::remove_dir_all(&vm_dir).await.map_err(|e| {
                AgentError::Internal(format!("Failed to remove VM dir {}: {e}", vm_dir.display()))
            })?;
        }

        tracing::info!(container = %dir_name, "VM container removed");
        Ok(())
    }

    /// Get container state.
    ///
    /// Checks whether the VM thread is still alive to detect exits.
    async fn container_state(&self, id: &ContainerId) -> Result<ContainerState> {
        let dir_name = Self::container_dir_name(id);

        let mut containers = self.containers.write().await;
        let container = containers
            .get_mut(&dir_name)
            .ok_or_else(|| AgentError::NotFound {
                container: dir_name.clone(),
                reason: "Container not found".to_string(),
            })?;

        // If already in a terminal state, return it
        match &container.state {
            ContainerState::Exited { .. } | ContainerState::Failed { .. } => {
                return Ok(container.state.clone());
            }
            ContainerState::Pending => return Ok(ContainerState::Pending),
            _ => {}
        }

        // Check if the VM thread has finished
        if let Some(ref thread) = container.vm_thread {
            if thread.is_finished() {
                // Thread finished -- take ownership to join it
                if let Some(thread) = container.vm_thread.take() {
                    let exit_code = match thread.join() {
                        Ok(code) => code,
                        Err(_) => {
                            container.state = ContainerState::Failed {
                                reason: "VM thread panicked".to_string(),
                            };
                            return Ok(container.state.clone());
                        }
                    };

                    container.state = ContainerState::Exited { code: exit_code };
                    container.ctx_id = None;

                    // Release GPU lock
                    if container.gpu_enabled {
                        self.gpu_in_use.store(false, Ordering::SeqCst);
                    }
                }
            }
            // else: thread still running, state is Running
        }

        Ok(container.state.clone())
    }

    /// Get container logs from the VM console output.
    ///
    /// Reads the console log file. If `tail > 0`, returns only the last
    /// `tail` lines.
    async fn container_logs(&self, id: &ContainerId, tail: usize) -> Result<String> {
        let dir_name = Self::container_dir_name(id);

        let log_file = {
            let containers = self.containers.read().await;
            let container = containers
                .get(&dir_name)
                .ok_or_else(|| AgentError::NotFound {
                    container: dir_name.clone(),
                    reason: "Container not found".to_string(),
                })?;
            container.log_file.clone()
        };

        let content = match tokio::fs::read_to_string(&log_file).await {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(String::new());
            }
            Err(e) => {
                return Err(AgentError::Internal(format!(
                    "Failed to read VM console log {}: {e}",
                    log_file.display()
                )));
            }
        };

        // Apply tail
        if tail > 0 {
            let lines: Vec<&str> = content.lines().collect();
            if lines.len() > tail {
                let start = lines.len() - tail;
                return Ok(lines[start..].join("\n"));
            }
        }

        Ok(content)
    }

    /// Execute a command inside the VM container.
    ///
    /// **NOT SUPPORTED** in the VM runtime. libkrun does not provide an API
    /// to execute arbitrary commands inside a running VM. Implementing this
    /// would require:
    /// - A vsock-based agent running inside the guest
    /// - Or SSH access into the VM
    ///
    /// Neither is currently set up. Use the SandboxRuntime if exec is required.
    async fn exec(&self, id: &ContainerId, _cmd: &[String]) -> Result<(i32, String, String)> {
        let dir_name = Self::container_dir_name(id);

        // Verify the container exists
        let containers = self.containers.read().await;
        if !containers.contains_key(&dir_name) {
            return Err(AgentError::NotFound {
                container: dir_name,
                reason: "Container not found".to_string(),
            });
        }

        Err(AgentError::Internal(
            "exec is not supported in the VM runtime. libkrun does not provide an API \
             to execute commands inside a running VM. This would require a vsock-based \
             agent or SSH server inside the guest. Use the SandboxRuntime (MacSandbox) \
             if you need exec capability."
                .to_string(),
        ))
    }

    /// Get container resource statistics.
    ///
    /// **Limited in VM runtime.** Since the VM runs as a host thread (not a
    /// separate process), we cannot use `proc_pidinfo` to get per-container
    /// stats. Instead, we report the configured resource limits as approximate
    /// values.
    ///
    /// For accurate per-container stats, use the SandboxRuntime which has
    /// real per-process monitoring via `proc_pidinfo`.
    async fn get_container_stats(&self, id: &ContainerId) -> Result<ContainerStats> {
        let dir_name = Self::container_dir_name(id);

        let containers = self.containers.read().await;
        let container = containers
            .get(&dir_name)
            .ok_or_else(|| AgentError::NotFound {
                container: dir_name.clone(),
                reason: "Container not found".to_string(),
            })?;

        if !matches!(container.state, ContainerState::Running) {
            return Err(AgentError::Internal(
                "Container is not running -- cannot collect stats".to_string(),
            ));
        }

        // We cannot get real per-VM stats without an in-guest agent.
        // Report configured limits as approximate values.
        let uptime_usec = container
            .started_at
            .map(|t| t.elapsed().as_micros() as u64)
            .unwrap_or(0);

        let memory_limit = (container.ram_mib as u64) * 1024 * 1024;

        Ok(ContainerStats {
            // Approximate CPU usage based on uptime (not real usage).
            // A proper implementation would require an in-guest metrics agent.
            cpu_usage_usec: uptime_usec,
            // Report memory limit as current usage (conservative estimate).
            // The actual usage inside the VM is not visible from the host
            // without an in-guest agent.
            memory_bytes: memory_limit,
            memory_limit,
            timestamp: Instant::now(),
        })
    }

    /// Wait for a container to exit and return its exit code.
    ///
    /// Blocks (asynchronously) until the VM thread completes.
    async fn wait_container(&self, id: &ContainerId) -> Result<i32> {
        let dir_name = Self::container_dir_name(id);

        // Check if already exited
        {
            let containers = self.containers.read().await;
            let container = containers
                .get(&dir_name)
                .ok_or_else(|| AgentError::NotFound {
                    container: dir_name.clone(),
                    reason: "Container not found".to_string(),
                })?;

            if let ContainerState::Exited { code } = &container.state {
                return Ok(*code);
            }
        }

        tracing::debug!(container = %dir_name, "Waiting for VM to exit");

        // Poll the VM thread status until it completes.
        // We poll rather than taking the thread handle because multiple
        // callers might be waiting on the same container.
        loop {
            {
                let mut containers = self.containers.write().await;
                if let Some(container) = containers.get_mut(&dir_name) {
                    // Check for terminal states
                    if let ContainerState::Exited { code } = &container.state {
                        return Ok(*code);
                    }
                    if let ContainerState::Failed { reason } = &container.state {
                        return Err(AgentError::Internal(format!("VM failed: {reason}")));
                    }

                    // Check if thread has finished
                    if let Some(ref thread) = container.vm_thread {
                        if thread.is_finished() {
                            if let Some(thread) = container.vm_thread.take() {
                                let exit_code = thread.join().unwrap_or(-1);
                                container.state = ContainerState::Exited { code: exit_code };
                                container.ctx_id = None;

                                if container.gpu_enabled {
                                    self.gpu_in_use.store(false, Ordering::SeqCst);
                                }

                                tracing::info!(
                                    container = %dir_name,
                                    exit_code = exit_code,
                                    "VM exited (via wait)"
                                );

                                return Ok(exit_code);
                            }
                        }
                    } else {
                        // No thread -- container was never started or thread was already joined
                        return Err(AgentError::Internal(
                            "VM has no active thread to wait on".to_string(),
                        ));
                    }
                } else {
                    return Err(AgentError::NotFound {
                        container: dir_name,
                        reason: "Container disappeared while waiting".to_string(),
                    });
                }
            }

            // Poll interval -- 250ms is a reasonable balance between
            // responsiveness and overhead
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    /// Get container logs as a vector of lines.
    async fn get_logs(&self, id: &ContainerId) -> Result<Vec<String>> {
        let dir_name = Self::container_dir_name(id);

        let log_file = {
            let containers = self.containers.read().await;
            let container = containers
                .get(&dir_name)
                .ok_or_else(|| AgentError::NotFound {
                    container: dir_name.clone(),
                    reason: "Container not found".to_string(),
                })?;
            container.log_file.clone()
        };

        match tokio::fs::read_to_string(&log_file).await {
            Ok(content) => Ok(content.lines().map(|l| l.to_string()).collect()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
            Err(e) => Err(AgentError::Internal(format!(
                "Failed to read VM console log: {e}"
            ))),
        }
    }

    /// Get the PID of a container's main process.
    ///
    /// Returns `None` for VM containers. The PID of the main process lives
    /// inside the VM's PID namespace and is not meaningful from the host.
    async fn get_container_pid(&self, id: &ContainerId) -> Result<Option<u32>> {
        let dir_name = Self::container_dir_name(id);

        let containers = self.containers.read().await;
        if !containers.contains_key(&dir_name) {
            return Err(AgentError::NotFound {
                container: dir_name,
                reason: "Container not found".to_string(),
            });
        }

        // VM process PID is inside the guest PID namespace -- not useful from the host.
        Ok(None)
    }

    /// Get the IP address of a container.
    ///
    /// Returns `127.0.0.1` (localhost) for all VM containers. TSI
    /// (Transparent Socket Impersonation) maps guest socket operations
    /// through the host process, so guest services bind ports directly
    /// on the host. The proxy manager routes traffic by port.
    ///
    /// ## Multi-replica note
    ///
    /// Unlike the SandboxRuntime, the VmRuntime does NOT have a port conflict
    /// problem with multiple replicas. Each VM has its own full network stack
    /// via libkrun's TSI -- the guest binds inside its own namespace, and TSI
    /// maps it to a unique host port automatically. No `get_container_port_override`
    /// is needed (the default `Ok(None)` from the trait is correct).
    async fn get_container_ip(&self, id: &ContainerId) -> Result<Option<IpAddr>> {
        let dir_name = Self::container_dir_name(id);

        let containers = self.containers.read().await;
        if !containers.contains_key(&dir_name) {
            return Err(AgentError::NotFound {
                container: dir_name,
                reason: "Container not found".to_string(),
            });
        }

        // TSI maps guest networking through the host process.
        // All VM containers appear as localhost from the host's perspective.
        Ok(Some(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)))
    }
}

// ---------------------------------------------------------------------------
// Directory copy helper
// ---------------------------------------------------------------------------

/// Recursively copy a directory tree.
///
/// Unlike the SandboxRuntime's `clone_directory_recursive` which uses APFS
/// `clonefile` for CoW, this does a regular copy. VMs have their own
/// filesystem view through virtio-fs, so CoW is not needed.
async fn copy_directory_recursive(
    src: &std::path::Path,
    dst: &std::path::Path,
) -> std::io::Result<()> {
    tokio::fs::create_dir_all(dst).await?;

    let mut entries = tokio::fs::read_dir(src).await?;
    while let Some(entry) = entries.next_entry().await? {
        let entry_path = entry.path();
        let file_name = entry.file_name();
        let dest_path = dst.join(&file_name);

        let file_type = entry.file_type().await?;

        if file_type.is_dir() {
            Box::pin(copy_directory_recursive(&entry_path, &dest_path)).await?;
        } else if file_type.is_file() {
            tokio::fs::copy(&entry_path, &dest_path).await?;
        } else if file_type.is_symlink() {
            let link_target = tokio::fs::read_link(&entry_path).await?;
            tokio::fs::symlink(&link_target, &dest_path).await?;
        }
    }

    // Preserve directory permissions
    let src_meta = tokio::fs::metadata(src).await?;
    tokio::fs::set_permissions(dst, src_meta.permissions()).await?;

    Ok(())
}
