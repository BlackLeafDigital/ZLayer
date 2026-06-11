//! macOS Seatbelt sandbox runtime
//!
//! Implements the [`Runtime`] trait using macOS process isolation:
//! - `fork()` + `sandbox_init()` + `exec()` for process creation
//! - Seatbelt `.sb` profiles for mandatory access control (deny-default whitelist)
//! - APFS `clonefile()` for copy-on-write filesystem isolation
//! - Direct Metal/MPS GPU access at 100% native performance
//!
//! This module is only compiled on macOS targets (`#[cfg(target_os = "macos")]`).
//! It provides lightweight process-level isolation without requiring Docker or
//! a Linux container runtime.
//!
//! ## Architecture
//!
//! Each "container" is a native macOS process running under a generated Seatbelt
//! profile. The profile restricts filesystem, network, IPC, and device access
//! based on the [`ServiceSpec`]. The rootfs is cloned from a pulled OCI image
//! using APFS copy-on-write (nearly instantaneous, zero additional disk space
//! until files are modified).
//!
//! ## Directory Layout
//!
//! ```text
//! {data_dir}/
//!   images/
//!     {sanitized_image_name}/
//!       rootfs/           -- extracted OCI image layers
//!   containers/
//!     {service}-{replica}/
//!       rootfs/           -- APFS clone of base image rootfs
//!       config.json       -- serialized ServiceSpec
//!       sandbox.sb        -- generated Seatbelt profile
//!       stdout.log        -- captured stdout
//!       stderr.log        -- captured stderr
//!       pid               -- PID file
//!       tmp/              -- container temp directory
//! ```

use crate::cgroups_stats::ContainerStats;
use crate::error::{AgentError, Result};
use crate::runtime::{ContainerId, ContainerState, ImageInfo, PruneResult, Runtime};
use crate::MacSandboxConfig;
use std::collections::{HashMap, HashSet};
use std::fmt::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use zlayer_observability::logs::{LogEntry, LogSource, LogStream};
use zlayer_registry::BlobCacheBackend;
use zlayer_spec::{RegistryAuth, ServiceSpec};

// ---------------------------------------------------------------------------
// Seatbelt profile types and generation
// ---------------------------------------------------------------------------

/// GPU access level for the sandbox profile.
///
/// Controls which `IOKit` user client classes, Mach services, and framework
/// paths are allowed in the generated Seatbelt profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GpuAccess {
    /// No GPU access -- deny all `IOKit` and GPU Mach services.
    None,
    /// Full Metal compute -- shader compilation + `IOKit` GPU access.
    /// Required for custom Metal shaders, `PyTorch` MPS with JIT compilation,
    /// and any workload that calls `MTLCreateSystemDefaultDevice()`.
    MetalCompute,
    /// MPS only -- pre-compiled kernels, no `MTLCompilerService` needed.
    /// Suitable for inference-only workloads using Apple's pre-built MPS kernels.
    /// Smaller attack surface than full Metal compute.
    MpsOnly,
}

/// Network access level for the sandbox profile.
#[derive(Debug, Clone)]
pub enum NetworkAccess {
    /// No network access at all.
    None,
    /// Only specific localhost ports (for inter-service communication).
    LocalhostOnly {
        bind_ports: Vec<u16>,
        connect_ports: Vec<u16>,
    },
    /// Full network access (outbound + inbound + bind).
    Full,
}

/// Complete sandbox configuration used to generate a Seatbelt profile.
#[derive(Debug, Clone)]
pub struct SandboxConfig {
    /// Path to the container's cloned rootfs.
    pub rootfs_dir: PathBuf,
    /// Path to the container's workspace/state directory.
    pub workspace_dir: PathBuf,
    /// GPU access level.
    pub gpu_access: GpuAccess,
    /// Network access configuration.
    pub network_access: NetworkAccess,
    /// Directories the process can write to (volume mounts).
    pub writable_dirs: Vec<PathBuf>,
    /// Additional read-only directories.
    pub readonly_dirs: Vec<PathBuf>,
    /// Maximum open file descriptors.
    pub max_files: u64,
    /// CPU time limit in seconds (`RLIMIT_CPU`).
    pub cpu_time_limit: Option<u64>,
    /// Memory limit in bytes (for watchdog, not kernel-enforced on macOS).
    pub memory_limit: Option<u64>,
}

/// Full Metal compute profile section.
///
/// Allows `IOKit` GPU access, Mach shader compilation services,
/// and all filesystem paths needed for Metal.framework.
///
/// `IOKit` user client class names were derived from:
/// - `ioreg -l -w0` on macOS 26 / Apple M5 (`AGXAcceleratorG17G`, `AGXDeviceUserClient`)
/// - Apple's own system sandbox profiles:
///   - `/System/Library/Sandbox/Profiles/com.apple.intelligenceplatformd.sb`
///   - `/System/Library/Sandbox/Profiles/safety-inference-extension-macos.sb`
///   - `/System/Library/Sandbox/Profiles/com.apple.intelligenceplatform.IntelligencePlatformComputeService.sb`
///
/// Key insight: On Apple Silicon (M1+), the actual `IOUserClient` class opened by
/// `MTLCreateSystemDefaultDevice()` is `AGXDeviceUserClient` -- NOT `AGXAccelerator`
/// (which is the IOService/kernel driver class name, not a user client class).
/// `AGXSharedUserClient` is needed for multi-process GPU sharing.
const METAL_COMPUTE_PROFILE_SECTION: &str = "\
; --- GPU: Full Metal Compute ---

; IOKit user clients for GPU hardware access
; Apple Silicon (M1/M2/M3/M4/M5): AGXDeviceUserClient is the actual user client
; class opened by Metal. AGXSharedUserClient handles multi-process GPU sharing.
; The IOAccel* classes are IOKit compatibility shims (still needed).
(allow iokit-open
  (iokit-user-client-class \"AGXDeviceUserClient\")
  (iokit-user-client-class \"AGXSharedUserClient\")
  (iokit-user-client-class \"IOSurfaceRootUserClient\")
  (iokit-user-client-class \"IOSurfaceAcceleratorClient\")
  (iokit-user-client-class \"IOAccelDevice\")
  (iokit-user-client-class \"IOAccelDevice2\")
  (iokit-user-client-class \"IOAccelContext\")
  (iokit-user-client-class \"IOAccelContext2\")
  (iokit-user-client-class \"IOAccelSharedUserClient\")
  (iokit-user-client-class \"IOAccelSharedUserClient2\")
  (iokit-user-client-class \"IOAccelSubmitter2\")
  (iokit-user-client-class \"RootDomainUserClient\"))

; IOKit service-level access (macOS 26+ fine-grained syntax)
; AGXAcceleratorG* prefix matches all Apple Silicon GPU generations.
(allow iokit-open-service
  (iokit-user-client-class \"IOSurfaceRoot\")
  (iokit-registry-entry-class-prefix \"AGXAcceleratorG\"))

; IOKit user-client-level access (macOS 26+ fine-grained syntax)
(allow iokit-open-user-client
  (iokit-user-client-class \"AGXDeviceUserClient\")
  (iokit-user-client-class \"AGXSharedUserClient\")
  (iokit-user-client-class \"IOSurfaceRootUserClient\")
  (iokit-user-client-class \"IOSurfaceAcceleratorClient\"))

; GPU IOKit properties (comprehensive set from Apple's safety-inference profile)
(allow iokit-get-properties
  (iokit-property \"AGCInfo\")
  (iokit-property \"AGXCliqueTracingDefaults\")
  (iokit-property \"AGXInternalPerfCounterResourcesPath\")
  (iokit-property \"AGXLimitersDirName\")
  (iokit-property \"AGXParameterBufferMaxSize\")
  (iokit-property \"AGXParameterBufferMaxSizeEverMemless\")
  (iokit-property \"AGXParameterBufferMaxSizeNeverMemless\")
  (iokit-property \"AGXTraceCodeVersion\")
  (iokit-property \"CFBundleIdentifier\")
  (iokit-property \"CFBundleIdentifierKernel\")
  (iokit-property \"chip-id\")
  (iokit-property \"CommandSubmissionEnabled\")
  (iokit-property \"CompactVRAM\")
  (iokit-property \"EnableBlitLib\")
  (iokit-property \"gpu-core-count\")
  (iokit-property \"GPUConfigurationVariable\")
  (iokit-property \"GPUDCCDisplayable\")
  (iokit-property \"GPUDebugNullClientMask\")
  (iokit-property \"GpuDebugPolicy\")
  (iokit-property \"GPURawCounterBundleName\")
  (iokit-property \"GPURawCounterPluginClassName\")
  (iokit-property \"IOClass\")
  (iokit-property \"IOClassNameOverride\")
  (iokit-property \"IOGeneralInterest\")
  (iokit-property \"IOGLBundleName\")
  (iokit-property \"IOGLESBundleName\")
  (iokit-property \"IOGLESDefaultUseMetal\")
  (iokit-property \"IOGLESMetalBundleName\")
  (iokit-property \"IOMatchCategory\")
  (iokit-property \"IOMatchedAtBoot\")
  (iokit-property \"IONameMatch\")
  (iokit-property \"IONameMatched\")
  (iokit-property \"IOPCIMatch\")
  (iokit-property \"IOPersonalityPublisher\")
  (iokit-property \"IOPowerManagement\")
  (iokit-property \"IOProbeScore\")
  (iokit-property \"IOProviderClass\")
  (iokit-property \"IORegistryEntryPropertyKeys\")
  (iokit-property \"IOReportLegend\")
  (iokit-property \"IOReportLegendPublic\")
  (iokit-property \"IOSourceVersion\")
  (iokit-property \"KDebugVersion\")
  (iokit-property \"MetalCoalesce\")
  (iokit-property \"MetalPluginClassName\")
  (iokit-property \"MetalPluginName\")
  (iokit-property \"MetalStatisticsName\")
  (iokit-property \"MetalStatisticsScriptName\")
  (iokit-property \"model\")
  (iokit-property \"PerformanceStatistics\")
  (iokit-property \"Removable\")
  (iokit-property \"SafeEjectRequested\")
  (iokit-property \"SchedulerState\")
  (iokit-property \"SCMBuildTime\")
  (iokit-property \"SCMVersionNumber\")
  (iokit-property \"soc-generation\")
  (iokit-property \"SurfaceList\")
  (iokit-property \"vendor-id\")
  (iokit-property \"device-id\")
  (iokit-property \"class-code\"))

; Mach services for Metal shader compilation and GPU memory
(allow mach-lookup
  (global-name \"com.apple.MTLCompilerService\")
  (global-name \"com.apple.CARenderServer\")
  (global-name \"com.apple.PowerManagement.control\")
  (global-name \"com.apple.gpu.process\")
  (global-name \"com.apple.gpumemd.source\")
  (global-name \"com.apple.cvmsServ\"))

; XPC services for shader compilation (Apple Silicon)
(allow mach-lookup
  (xpc-service-name \"com.apple.MTLCompilerService\")
  (xpc-service-name-prefix \"com.apple.AGXCompilerService\"))

; User preferences for Metal/OpenGL
(allow user-preference-read
  (preference-domain \"com.apple.opengl\")
  (preference-domain \"com.apple.Metal\")
  (preference-domain \"com.nvidia.OpenGL\"))

; GPU driver bundles and libraries
(allow file-read*
  (subpath \"/Library/GPUBundles\")
  (subpath \"/System/Library/Frameworks/Metal.framework\")
  (subpath \"/System/Library/Frameworks/MetalPerformanceShaders.framework\")
  (subpath \"/System/Library/Frameworks/MetalPerformanceShadersGraph.framework\")
  (subpath \"/System/Library/PrivateFrameworks/GPUCompiler.framework\"))

";

/// MPS-only profile section (subset of Metal compute).
///
/// MPS mode provides a smaller attack surface than full Metal compute by
/// restricting `IOKit` access to a minimal set and omitting `AGXCompilerService`
/// XPC services. However, `MTLCompilerService` is still required because
/// `MPSGraph` on macOS 26+ uses JIT compilation internally for kernel fusion.
///
/// Uses the same corrected `IOKit` user client classes as the full Metal profile
/// (`AGXDeviceUserClient` instead of the incorrect `AGXAccelerator`).
const MPS_ONLY_PROFILE_SECTION: &str = "\
; --- GPU: MPS Only (pre-compiled kernels, no shader compilation) ---

; IOKit user clients for GPU hardware access (minimal set)
; AGXDeviceUserClient is required -- MTLCreateSystemDefaultDevice() opens this class.
(allow iokit-open
  (iokit-user-client-class \"AGXDeviceUserClient\")
  (iokit-user-client-class \"AGXSharedUserClient\")
  (iokit-user-client-class \"IOSurfaceRootUserClient\")
  (iokit-user-client-class \"IOAccelDevice2\")
  (iokit-user-client-class \"IOAccelContext2\")
  (iokit-user-client-class \"IOAccelSharedUserClient2\")
  (iokit-user-client-class \"RootDomainUserClient\"))

; IOKit service-level access (macOS 26+ fine-grained syntax)
(allow iokit-open-service
  (iokit-user-client-class \"IOSurfaceRoot\")
  (iokit-registry-entry-class-prefix \"AGXAcceleratorG\"))

; IOKit user-client-level access (macOS 26+ fine-grained syntax)
(allow iokit-open-user-client
  (iokit-user-client-class \"AGXDeviceUserClient\")
  (iokit-user-client-class \"IOSurfaceRootUserClient\"))

; GPU IOKit properties (minimal set for MPS)
(allow iokit-get-properties
  (iokit-property \"MetalPluginClassName\")
  (iokit-property \"MetalPluginName\")
  (iokit-property \"IOClass\")
  (iokit-property \"IOGLESDefaultUseMetal\")
  (iokit-property \"IORegistryEntryPropertyKeys\")
  (iokit-property \"IOSourceVersion\")
  (iokit-property \"GPUConfigurationVariable\")
  (iokit-property \"GPURawCounterBundleName\")
  (iokit-property \"gpu-core-count\")
  (iokit-property \"model\")
  (iokit-property \"vendor-id\")
  (iokit-property \"device-id\")
  (iokit-property \"soc-generation\"))

; Mach services for MPS
; MTLCompilerService is required because MPSGraph on macOS 26+ uses JIT
; compilation internally for kernel fusion, even for pre-compiled MPS kernels.
; gpumemd.source is needed for GPU memory management.
(allow mach-lookup
  (global-name \"com.apple.MTLCompilerService\")
  (global-name \"com.apple.PowerManagement.control\")
  (global-name \"com.apple.gpumemd.source\"))

; XPC service for MTLCompilerService (required by MPSGraph JIT)
(allow mach-lookup
  (xpc-service-name \"com.apple.MTLCompilerService\"))

; User preferences
(allow user-preference-read
  (preference-domain \"com.apple.Metal\"))

; MPS framework access
(allow file-read*
  (subpath \"/Library/GPUBundles\")
  (subpath \"/System/Library/Frameworks/Metal.framework\")
  (subpath \"/System/Library/Frameworks/MetalPerformanceShaders.framework\")
  (subpath \"/System/Library/Frameworks/MetalPerformanceShadersGraph.framework\"))

";

/// Generate a complete Seatbelt profile from a [`SandboxConfig`].
///
/// The profile follows a deny-default whitelist model: everything is denied
/// unless explicitly allowed. The profile is structured in sections:
///
/// 1. Base process rules (always needed for any process to run)
/// 2. System library access (dyld, libSystem, frameworks)
/// 3. Container rootfs access (read + write)
/// 4. Volume mount access (writable dirs)
/// 5. GPU rules (if `gpu_access != None`)
/// 6. Network rules (based on `network_access`)
/// 7. Logging and /dev/null access
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn generate_sandbox_profile(config: &SandboxConfig) -> String {
    let mut profile = String::with_capacity(4096);

    // Header
    profile.push_str("(version 1)\n");
    profile.push_str("(deny default)\n");
    profile.push('\n');

    // ===== Section 1: Base process rules =====
    profile.push_str("; --- Base process rules ---\n");
    profile.push_str("(allow process-exec)\n");
    profile.push_str("(allow process-fork)\n");
    profile.push_str("(allow signal (target same-sandbox))\n");
    profile.push_str("(allow process-info* (target self))\n");
    profile.push_str("(allow process-info-pidinfo)\n");
    profile.push_str("(allow process-info-rusage)\n");
    profile.push('\n');

    // ===== Section 2: System library and framework access =====
    profile.push_str("; --- System libraries (required for any process to run) ---\n");
    profile.push_str("(allow file-read*\n");
    profile.push_str("  (subpath \"/usr/lib\")\n");
    profile.push_str("  (subpath \"/System/Library/Frameworks\")\n");
    profile.push_str("  (subpath \"/System/Library/PrivateFrameworks\")\n");
    profile.push_str("  (subpath \"/System/Library/Extensions\")\n");
    profile.push_str("  (subpath \"/System/Library/ColorSync\")\n");
    profile.push_str("  (literal \"/\")\n");
    profile.push_str("  (literal \"/dev/random\")\n");
    profile.push_str("  (literal \"/dev/urandom\"))\n");
    profile.push('\n');
    profile.push_str("; --- Executable mapping (required for dyld) ---\n");
    profile.push_str("(allow file-map-executable\n");
    profile.push_str("  (subpath \"/usr/lib\")\n");
    profile.push_str("  (subpath \"/System/Library/Frameworks\")\n");
    profile.push_str("  (subpath \"/System/Library/PrivateFrameworks\")\n");
    profile.push_str("  (subpath \"/System/Library/Extensions\"))\n");
    profile.push('\n');
    profile.push_str("; --- System info (hw detection, etc.) ---\n");
    profile.push_str("(allow sysctl-read)\n");
    profile.push_str("(allow system-info)\n");
    profile.push('\n');
    profile.push_str("; --- Mach basics ---\n");
    profile.push_str("(allow mach-lookup\n");
    profile.push_str("  (global-name \"com.apple.system.opendirectoryd.libinfo\"))\n");
    profile.push('\n');

    // ===== Section 3: Container rootfs access =====
    profile.push_str("; --- Container rootfs ---\n");
    let _ = writeln!(
        profile,
        "(allow file-read* file-write* (subpath \"{}\"))",
        config.rootfs_dir.display()
    );
    let _ = writeln!(
        profile,
        "(allow file-map-executable (subpath \"{}\"))",
        config.rootfs_dir.display()
    );
    profile.push('\n');

    // Workspace directory (logs, config, etc.)
    profile.push_str("; --- Workspace directory ---\n");
    let _ = writeln!(
        profile,
        "(allow file-read* file-write* (subpath \"{}\"))",
        config.workspace_dir.display()
    );
    profile.push('\n');

    // ===== Section 4: Volume mounts =====
    if !config.writable_dirs.is_empty() {
        profile.push_str("; --- Volume mounts (writable) ---\n");
        for dir in &config.writable_dirs {
            let _ = writeln!(
                profile,
                "(allow file-read* file-write* (subpath \"{}\"))",
                dir.display()
            );
        }
        profile.push('\n');
    }

    if !config.readonly_dirs.is_empty() {
        profile.push_str("; --- Volume mounts (read-only) ---\n");
        for dir in &config.readonly_dirs {
            let _ = writeln!(
                profile,
                "(allow file-read* (subpath \"{}\"))",
                dir.display()
            );
        }
        profile.push('\n');
    }

    // ===== Section 5: GPU access =====
    match config.gpu_access {
        GpuAccess::MetalCompute => {
            profile.push_str(METAL_COMPUTE_PROFILE_SECTION);
        }
        GpuAccess::MpsOnly => {
            profile.push_str(MPS_ONLY_PROFILE_SECTION);
        }
        GpuAccess::None => {}
    }

    // ===== Section 6: Network access =====
    match &config.network_access {
        NetworkAccess::None => {
            profile.push_str("; --- Network: DENIED ---\n\n");
        }
        NetworkAccess::LocalhostOnly {
            bind_ports,
            connect_ports,
        } => {
            profile.push_str("; --- Network: localhost only ---\n");
            for port in bind_ports {
                let _ = writeln!(
                    profile,
                    "(allow network-bind (local ip \"localhost:{port}\"))",
                );
            }
            for port in connect_ports {
                let _ = writeln!(
                    profile,
                    "(allow network-outbound (remote ip \"localhost:{port}\"))",
                );
            }
            // Allow inbound on bound ports
            if !bind_ports.is_empty() {
                profile.push_str("(allow network-inbound (local ip \"localhost:*\"))\n");
            }
            profile.push('\n');
        }
        NetworkAccess::Full => {
            profile.push_str("; --- Network: full access ---\n");
            profile.push_str("(allow network-outbound)\n");
            profile.push_str("(allow network-inbound)\n");
            profile.push_str("(allow network-bind)\n");
            profile.push_str("(allow system-socket)\n");
            profile.push('\n');
        }
    }

    // ===== Section 7: Logging, /dev/null, pseudo-tty =====
    profile.push_str("; --- I/O essentials ---\n");
    profile.push_str("(allow file-write-data\n");
    profile.push_str("  (require-all (literal \"/dev/null\") (vnode-type CHARACTER-DEVICE)))\n");
    profile.push_str("(allow file-read-data\n");
    profile.push_str("  (require-all (literal \"/dev/null\") (vnode-type CHARACTER-DEVICE)))\n");
    profile.push_str("(allow pseudo-tty)\n");
    profile.push_str("(allow file-read* file-write* file-ioctl (literal \"/dev/ptmx\"))\n");
    profile.push('\n');

    // IPC basics
    profile.push_str("; --- IPC ---\n");
    profile.push_str("(allow ipc-posix-sem)\n");
    profile.push_str("(allow ipc-posix-shm)\n");
    profile.push('\n');

    profile
}

// ---------------------------------------------------------------------------
// Seatbelt FFI
// ---------------------------------------------------------------------------

/// FFI declarations for macOS Seatbelt sandbox.
mod seatbelt_ffi {
    use std::os::raw::c_char;

    #[link(name = "System", kind = "dylib")]
    extern "C" {
        /// Apply a sandbox profile to the current process.
        ///
        /// - `profile`: SBPL string (Scheme-based sandbox profile)
        /// - `flags`: 0 for raw SBPL string, 0x0001 for named profile
        /// - `errorbuf`: receives error message on failure (free with `sandbox_free_error`)
        ///
        /// Returns 0 on success, -1 on failure.
        /// WARNING: Once applied, the sandbox CANNOT be removed or loosened.
        pub fn sandbox_init(profile: *const c_char, flags: u64, errorbuf: *mut *mut c_char) -> i32;

        /// Free an error buffer allocated by `sandbox_init`.
        pub fn sandbox_free_error(errorbuf: *mut c_char);
    }
}

/// Apply a Seatbelt profile to the current process.
///
/// This is called in the child process after `fork()` and before `exec()`.
/// Once applied, the sandbox cannot be removed or loosened.
///
/// # Errors
///
/// Returns an error if the profile string contains a null byte or if
/// `sandbox_init()` fails to apply the profile.
#[allow(unsafe_code)]
fn apply_seatbelt_profile(sbpl: &str) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::ptr;

    let profile_cstr =
        CString::new(sbpl).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

    let mut error_buf: *mut std::os::raw::c_char = ptr::null_mut();

    let result = unsafe {
        seatbelt_ffi::sandbox_init(
            profile_cstr.as_ptr(),
            0, // 0 = raw SBPL string
            &raw mut error_buf,
        )
    };

    if result != 0 {
        let error_msg = if error_buf.is_null() {
            format!("sandbox_init returned error code {result}")
        } else {
            let msg = unsafe {
                std::ffi::CStr::from_ptr(error_buf)
                    .to_string_lossy()
                    .into_owned()
            };
            unsafe { seatbelt_ffi::sandbox_free_error(error_buf) };
            msg
        };
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("Failed to initialize sandbox: {error_msg}"),
        ));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// APFS clonefile FFI
// ---------------------------------------------------------------------------

extern "C" {
    /// Clone a file using APFS copy-on-write semantics.
    ///
    /// From `<sys/clonefile.h>`. Creates a new file at `dst` that shares
    /// storage blocks with `src`. Nearly instantaneous. Both files must
    /// be on the same APFS volume.
    fn clonefile(
        src: *const libc::c_char,
        dst: *const libc::c_char,
        flags: libc::c_int,
    ) -> libc::c_int;
}

/// Clone a single file using APFS `CoW`.
///
/// Returns `Ok(true)` if clonefile succeeded, `Ok(false)` if clonefile
/// is not supported (non-APFS volume) and the caller should fall back
/// to a regular copy.
///
/// # Errors
///
/// Returns an error if the source or destination paths contain invalid
/// characters or if `clonefile()` fails for a reason other than unsupported
/// filesystem or cross-device.
#[allow(unsafe_code)]
fn clone_file_apfs(src: &Path, dst: &Path) -> std::io::Result<bool> {
    use std::ffi::CString;

    let c_src = CString::new(src.to_str().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid src path")
    })?)
    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

    let c_dst = CString::new(dst.to_str().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid dst path")
    })?)
    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

    let ret = unsafe { clonefile(c_src.as_ptr(), c_dst.as_ptr(), 0) };

    if ret == 0 {
        Ok(true) // Clone succeeded
    } else {
        let err = std::io::Error::last_os_error();
        // ENOTSUP means the filesystem doesn't support clonefile (not APFS)
        // EXDEV means src and dst are on different volumes
        // EEXIST means the destination already exists (concurrent writer)
        if err.raw_os_error() == Some(libc::ENOTSUP)
            || err.raw_os_error() == Some(libc::EXDEV)
            || err.raw_os_error() == Some(libc::EEXIST)
        {
            Ok(false) // Fallback needed
        } else {
            Err(err)
        }
    }
}

/// Recursively clone a directory tree using APFS `clonefile` for files.
///
/// `clonefile` operates at the file level, not directory level, so we must
/// walk the directory tree, create directories in the destination, and
/// clonefile each regular file.
///
/// If APFS clonefile is not available (non-APFS volume), falls back to
/// regular file copy.
async fn clone_directory_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    // Create destination directory
    tokio::fs::create_dir_all(dst).await?;

    let mut entries = tokio::fs::read_dir(src).await?;
    while let Some(entry) = entries.next_entry().await? {
        let entry_path = entry.path();
        let file_name = entry.file_name();
        let dest_path = dst.join(&file_name);

        let file_type = entry.file_type().await?;

        if file_type.is_dir() {
            // Recurse into subdirectories
            Box::pin(clone_directory_recursive(&entry_path, &dest_path)).await?;
        } else if file_type.is_file() {
            // Try APFS clone first, fall back to copy
            let src_clone = entry_path.clone();
            let dst_clone = dest_path.clone();

            let cloned =
                tokio::task::spawn_blocking(move || clone_file_apfs(&src_clone, &dst_clone))
                    .await
                    .map_err(std::io::Error::other)??;

            if !cloned {
                // Fallback: regular copy
                tokio::fs::copy(&entry_path, &dest_path).await?;
            }
        } else if file_type.is_symlink() {
            // Recreate symlinks
            let link_target = tokio::fs::read_link(&entry_path).await?;
            tokio::fs::symlink(&link_target, &dest_path).await?;
        }
    }

    // Preserve directory permissions
    let src_meta = tokio::fs::metadata(src).await?;
    tokio::fs::set_permissions(dst, src_meta.permissions()).await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Process stats FFI
// ---------------------------------------------------------------------------

/// Get the resident set size (RSS) of a process using `proc_pidinfo`.
///
/// On macOS, we use the `PROC_PIDTASKINFO` flavor which works for child
/// processes without special entitlements.
///
/// # Errors
///
/// Returns an error if `proc_pidinfo` fails (e.g. the process does not exist).
#[allow(unsafe_code)]
#[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
fn get_process_rss(pid: u32) -> std::io::Result<u64> {
    #[repr(C)]
    #[allow(non_snake_case)]
    #[allow(clippy::struct_field_names)]
    struct ProcTaskInfo {
        pti_virtual_size: u64,
        pti_resident_size: u64,
        pti_total_user: u64,
        pti_total_system: u64,
        pti_threads_user: u64,
        pti_threads_system: u64,
        pti_policy: i32,
        pti_faults: i32,
        pti_pageins: i32,
        pti_cow_faults: i32,
        pti_messages_sent: i32,
        pti_messages_received: i32,
        pti_syscalls_mach: i32,
        pti_syscalls_unix: i32,
        pti_csw: i32,
        pti_threadnum: i32,
        pti_numrunning: i32,
        pti_priority: i32,
    }

    extern "C" {
        fn proc_pidinfo(
            pid: libc::c_int,
            flavor: libc::c_int,
            arg: u64,
            buffer: *mut libc::c_void,
            buffersize: libc::c_int,
        ) -> libc::c_int;
    }

    const PROC_PIDTASKINFO: libc::c_int = 4;

    let mut info: ProcTaskInfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<ProcTaskInfo>() as libc::c_int;

    let ret = unsafe {
        proc_pidinfo(
            pid as libc::c_int,
            PROC_PIDTASKINFO,
            0,
            (&raw mut info).cast::<libc::c_void>(),
            size,
        )
    };

    if ret <= 0 {
        return Err(std::io::Error::last_os_error());
    }

    Ok(info.pti_resident_size)
}

/// Get CPU time (user + system) and memory RSS for a process.
///
/// CPU time is returned in microseconds. Memory is returned in bytes.
///
/// # Errors
///
/// Returns an error if `proc_pidinfo` fails (e.g. the process does not exist).
#[allow(unsafe_code)]
#[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
fn get_process_stats(pid: u32) -> Result<(u64, u64)> {
    #[repr(C)]
    #[allow(non_snake_case)]
    #[allow(clippy::struct_field_names)]
    struct ProcTaskInfo {
        pti_virtual_size: u64,
        pti_resident_size: u64,
        pti_total_user: u64,
        pti_total_system: u64,
        pti_threads_user: u64,
        pti_threads_system: u64,
        pti_policy: i32,
        pti_faults: i32,
        pti_pageins: i32,
        pti_cow_faults: i32,
        pti_messages_sent: i32,
        pti_messages_received: i32,
        pti_syscalls_mach: i32,
        pti_syscalls_unix: i32,
        pti_csw: i32,
        pti_threadnum: i32,
        pti_numrunning: i32,
        pti_priority: i32,
    }

    extern "C" {
        fn proc_pidinfo(
            pid: libc::c_int,
            flavor: libc::c_int,
            arg: u64,
            buffer: *mut libc::c_void,
            buffersize: libc::c_int,
        ) -> libc::c_int;
    }

    const PROC_PIDTASKINFO: libc::c_int = 4;

    let mut info: ProcTaskInfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<ProcTaskInfo>() as libc::c_int;

    let ret = unsafe {
        proc_pidinfo(
            pid as libc::c_int,
            PROC_PIDTASKINFO,
            0,
            (&raw mut info).cast::<libc::c_void>(),
            size,
        )
    };

    if ret <= 0 {
        return Err(AgentError::Internal(format!(
            "proc_pidinfo failed for pid {pid}: {}",
            std::io::Error::last_os_error()
        )));
    }

    // pti_total_user and pti_total_system are in Mach absolute time units (nanoseconds on Apple Silicon).
    // Convert to microseconds.
    let cpu_usec = (info.pti_total_user + info.pti_total_system) / 1000;
    let rss = info.pti_resident_size;

    Ok((cpu_usec, rss))
}

// ---------------------------------------------------------------------------
// Resource limits
// ---------------------------------------------------------------------------

/// Set resource limits for the sandboxed process.
///
/// Called in the child process after `fork()` via `pre_exec`.
///
/// # Errors
///
/// Returns an error if `setrlimit` fails.
#[allow(unsafe_code)]
fn set_resource_limits(max_files: u64, cpu_seconds: Option<u64>) -> std::io::Result<()> {
    // Limit open file descriptors
    let file_limit = libc::rlimit {
        rlim_cur: max_files,
        rlim_max: max_files,
    };
    if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &raw const file_limit) } != 0 {
        return Err(std::io::Error::last_os_error());
    }

    // Limit CPU time if specified
    if let Some(seconds) = cpu_seconds {
        let cpu_limit = libc::rlimit {
            rlim_cur: seconds,
            rlim_max: seconds,
        };
        if unsafe { libc::setrlimit(libc::RLIMIT_CPU, &raw const cpu_limit) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Memory watchdog
// ---------------------------------------------------------------------------

/// Memory watchdog that monitors a process's RSS and kills it if exceeded.
///
/// This is necessary because macOS does NOT enforce `RLIMIT_RSS` or `RLIMIT_AS`.
/// The watchdog polls every 2 seconds using `proc_pidinfo` (Mach API).
#[allow(unsafe_code)]
#[allow(clippy::cast_possible_wrap)]
async fn memory_watchdog(pid: u32, limit_bytes: u64) {
    let check_interval = Duration::from_secs(2);

    loop {
        tokio::time::sleep(check_interval).await;

        // Check if process is still alive
        let alive = unsafe { libc::kill(pid as i32, 0) } == 0;
        if !alive {
            tracing::debug!(pid = pid, "Memory watchdog: process exited");
            return;
        }

        // Get current RSS
        match get_process_rss(pid) {
            Ok(rss_bytes) => {
                if rss_bytes > limit_bytes {
                    tracing::warn!(
                        pid = pid,
                        rss_mb = rss_bytes / (1024 * 1024),
                        limit_mb = limit_bytes / (1024 * 1024),
                        "Memory limit exceeded, sending SIGKILL"
                    );
                    unsafe {
                        libc::kill(pid as i32, libc::SIGKILL);
                    }
                    return;
                }
            }
            Err(e) => {
                tracing::debug!(pid = pid, error = %e, "Failed to read process RSS");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build [`NetworkAccess`] from a [`ServiceSpec`]'s endpoints and network configuration.
fn build_network_access(spec: &ServiceSpec) -> NetworkAccess {
    let mut bind_ports = Vec::new();
    let mut connect_ports = Vec::new();

    // Collect ports from endpoints
    for endpoint in &spec.endpoints {
        bind_ports.push(endpoint.target_port());
    }

    // If no endpoints and no special network config, default to full access
    // (most services need outbound connectivity for dependencies)
    if bind_ports.is_empty() {
        return NetworkAccess::Full;
    }

    // Add common connect ports (DNS, HTTP, HTTPS) for outbound
    connect_ports.extend_from_slice(&[53, 80, 443]);

    // Add all bind ports as connect ports too (for health checks)
    connect_ports.extend_from_slice(&bind_ports);

    NetworkAccess::LocalhostOnly {
        bind_ports,
        connect_ports,
    }
}

/// Build writable directory list from [`ServiceSpec`] storage configuration.
fn build_writable_dirs(spec: &ServiceSpec, container_dir: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![
        container_dir.join("tmp"), // Always allow a tmp directory
    ];

    for storage in &spec.storage {
        let target = match storage {
            zlayer_spec::StorageSpec::Named { target, .. }
            | zlayer_spec::StorageSpec::Anonymous { target, .. }
            | zlayer_spec::StorageSpec::Bind { target, .. }
            | zlayer_spec::StorageSpec::Tmpfs { target, .. }
            | zlayer_spec::StorageSpec::S3 { target, .. } => target,
        };
        dirs.push(PathBuf::from(target));
    }

    dirs
}

/// Parse a memory string like "512Mi" or "2Gi" into bytes.
#[must_use]
fn parse_memory_string(s: &str) -> Option<u64> {
    let s = s.trim();
    if let Some(num) = s.strip_suffix("Gi") {
        num.parse::<u64>().ok().map(|v| v * 1024 * 1024 * 1024)
    } else if let Some(num) = s.strip_suffix("Mi") {
        num.parse::<u64>().ok().map(|v| v * 1024 * 1024)
    } else if let Some(num) = s.strip_suffix("Ki") {
        num.parse::<u64>().ok().map(|v| v * 1024)
    } else {
        s.parse::<u64>().ok()
    }
}

/// Sanitize an image name for use as a filesystem directory name.
#[must_use]
fn sanitize_image_name(image: &str) -> String {
    image.replace(['/', ':', '@'], "_")
}

/// Resolve the entrypoint command from a [`ServiceSpec`].
///
/// Checks `spec.command.entrypoint` and `spec.command.args` in order,
/// then falls back to searching for a shell in the rootfs.
///
/// # Errors
///
/// Returns an error if no entrypoint is specified and no shell is found.
fn resolve_entrypoint(spec: &ServiceSpec, rootfs: &Path) -> Result<(String, Vec<String>)> {
    // Resolve a program path for the macOS sandbox runtime.
    //
    // If the program is an absolute path (e.g. "/usr/local/bin/app"):
    //   1. If it exists on the host -> use the host path (macOS platform binaries
    //      must be exec'd from their original path to pass code-signing checks).
    //   2. Else if it exists inside rootfs -> use the rootfs-resolved path so
    //      `Command::new()` can find it.
    //   3. Otherwise return as-is and let exec() produce a clear error.
    let resolve_program = |prog: &str| -> String {
        if prog.starts_with('/') {
            // Prefer the host binary (code-signing / platform binary compat)
            if std::path::Path::new(prog).exists() {
                return prog.to_string();
            }
            // Fall back to rootfs copy
            let rootfs_path = rootfs.join(prog.trim_start_matches('/'));
            if rootfs_path.exists() {
                return rootfs_path.to_string_lossy().into_owned();
            }
        }
        prog.to_string()
    };

    // Use entrypoint if specified
    if let Some(ref entrypoint) = spec.command.entrypoint {
        if !entrypoint.is_empty() {
            let program = resolve_program(&entrypoint[0]);
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
            let program = resolve_program(&cmd_args[0]);
            let args = cmd_args[1..].to_vec();
            return Ok((program, args));
        }
    }

    // Fallback: try to find a shell - prefer host path for code-signing compat,
    // then check rootfs
    for shell in &["/bin/sh", "/bin/bash", "/usr/bin/sh"] {
        if std::path::Path::new(shell).exists() {
            return Ok(((*shell).to_string(), vec![]));
        }
        if rootfs.join(shell.trim_start_matches('/')).exists() {
            let resolved = rootfs.join(shell.trim_start_matches('/'));
            return Ok((resolved.to_string_lossy().into_owned(), vec![]));
        }
    }

    Err(AgentError::InvalidSpec(
        "No command specified and no shell found in rootfs".to_string(),
    ))
}

/// Parameters for spawning a sandboxed process.
struct SandboxSpawnParams {
    program: String,
    args: Vec<String>,
    sbpl_profile: String,
    rootfs_dir: PathBuf,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
    spec: ServiceSpec,
    sandbox_config: SandboxConfig,
    assigned_port: u16,
    auth_env: Option<(String, String, String)>, // (api_url, token, socket_path)
}

/// Spawn a sandboxed process using `fork()` + `sandbox_init()` + `exec()`.
///
/// The sequence:
/// 1. Open log files for stdout/stderr capture.
/// 2. Build the environment, including `PORT` and `ZLAYER_PORT` set to the
///    dynamically assigned port.
/// 3. Create a `Command` with `pre_exec` that applies the Seatbelt profile
///    and resource limits.
/// 4. Spawn the child process.
/// 5. Return the child PID.
///
/// # Errors
///
/// Returns an error if log files cannot be created or the process fails to spawn.
#[allow(unsafe_code)]
fn spawn_sandboxed_process(params: &SandboxSpawnParams) -> Result<u32> {
    use std::os::unix::process::CommandExt;

    let profile = params.sbpl_profile.clone();
    let max_files = params.sandbox_config.max_files;
    let cpu_time_limit = params.sandbox_config.cpu_time_limit;

    // Build environment variables from the spec
    let mut env_vars: Vec<(String, String)> = params
        .spec
        .env
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    // Inject the assigned port as PORT and ZLAYER_PORT.
    //
    // PORT is the de-facto standard env var for telling a web server which
    // port to listen on (Express, Flask, Actix, Axum, Rails, etc.).
    // ZLAYER_PORT is a ZLayer-specific alias for frameworks that use PORT
    // for something else.
    //
    // Only set PORT if the user's spec didn't already define it (don't
    // override explicit user configuration).
    let port_str = params.assigned_port.to_string();
    if !env_vars.iter().any(|(k, _)| k == "PORT") {
        env_vars.push(("PORT".to_string(), port_str.clone()));
    }
    env_vars.push(("ZLAYER_PORT".to_string(), port_str));

    // Inject ZLayer API credentials for container-to-host communication
    if let Some((ref api_url, ref token, ref socket)) = params.auth_env {
        env_vars.push(("ZLAYER_API_URL".to_string(), api_url.clone()));
        env_vars.push(("ZLAYER_TOKEN".to_string(), token.clone()));
        env_vars.push(("ZLAYER_SOCKET".to_string(), socket.clone()));
    }

    // Open log files for stdout/stderr redirection
    let stdout_file =
        std::fs::File::create(&params.stdout_path).map_err(|e| AgentError::CreateFailed {
            id: "sandbox-process".to_string(),
            reason: format!("Failed to create stdout log: {e}"),
        })?;
    let stderr_file =
        std::fs::File::create(&params.stderr_path).map_err(|e| AgentError::CreateFailed {
            id: "sandbox-process".to_string(),
            reason: format!("Failed to create stderr log: {e}"),
        })?;

    // Spawn the child process with pre_exec hook for sandbox application.
    // SAFETY: pre_exec runs after fork() in the child process. We only call
    // async-signal-safe-compatible operations (our FFI calls and setrlimit).
    let child = unsafe {
        std::process::Command::new(&params.program)
            .args(&params.args)
            .current_dir(&params.rootfs_dir)
            .stdout(stdout_file)
            .stderr(stderr_file)
            .env_clear()
            .envs(env_vars)
            .pre_exec(move || {
                // Apply Seatbelt sandbox profile (irrevocable)
                apply_seatbelt_profile(&profile)?;

                // Apply resource limits
                set_resource_limits(max_files, cpu_time_limit)?;

                Ok(())
            })
            .spawn()
    }
    .map_err(|e| AgentError::StartFailed {
        id: "sandbox-process".to_string(),
        reason: format!("Failed to spawn sandboxed process: {e}"),
    })?;

    Ok(child.id())
}

// ---------------------------------------------------------------------------
// Port allocation
// ---------------------------------------------------------------------------

/// Reserve a free TCP port on localhost by binding to port 0.
///
/// Returns `(port, listener)`. The caller **must** hold the returned
/// `TcpListener` until the child process has started and is ready to bind
/// the same port.  Dropping the listener releases the port to the OS,
/// creating a brief window for the child to bind before anything else can
/// claim it.
///
/// The flow:
///   1. Parent calls `reserve_port()` -> gets `(port, guard_listener)`
///   2. Parent spawns the child with `PORT={port}` in its environment
///   3. Parent drops `guard_listener` immediately after `spawn()` returns
///   4. Child's framework reads `PORT`, calls `bind("0.0.0.0:{port}")`
///
/// The race window (step 3->4) is on the order of microseconds (process
/// startup before the child enters `bind()`).  On a developer laptop
/// (the target for macOS sandbox), port theft in this window is not a
/// realistic concern.  For server-class isolation use the `VmRuntime`,
/// where each VM has its own network stack.
///
/// # Errors
///
/// Returns an error if binding to port 0 fails.
fn reserve_port() -> std::io::Result<(u16, std::net::TcpListener)> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    Ok((port, listener))
}

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

/// Metadata for a sandboxed container process.
#[derive(Debug)]
struct SandboxContainer {
    /// Process ID of the sandboxed child (0 if not yet started).
    pid: u32,
    /// Current container state.
    state: ContainerState,
    /// Path to the container's state directory.
    state_dir: PathBuf,
    /// Path to the cloned rootfs.
    rootfs_dir: PathBuf,
    /// Path to the stdout log file.
    stdout_path: PathBuf,
    /// Path to the stderr log file.
    stderr_path: PathBuf,
    /// When the process was started.
    started_at: Option<Instant>,
    /// The original service spec (needed for `start_container`).
    spec: ServiceSpec,
    /// Generated sandbox configuration.
    sandbox_config: SandboxConfig,
    /// Memory limit in bytes (for watchdog).
    memory_limit: Option<u64>,
    /// Handle to memory watchdog task.
    watchdog_handle: Option<tokio::task::JoinHandle<()>>,
    /// Dynamic port assigned to this container for host-network port isolation.
    ///
    /// On macOS, all sandboxed processes share the host network stack. To support
    /// multiple replicas of the same service, each replica is assigned a unique
    /// port. This port is passed to the process via the `PORT` environment variable
    /// (a convention respected by most web frameworks). The proxy uses this port
    /// instead of the spec-declared port when constructing backend addresses.
    assigned_port: u16,
    /// Guard listener that holds the assigned port until the child process starts.
    ///
    /// Dropped in `start_container()` right after spawning the child process,
    /// freeing the port for the child to bind. Holding this prevents other
    /// processes from claiming the port between `create_container()` and
    /// `start_container()`.
    port_guard: Option<std::net::TcpListener>,
}

// ---------------------------------------------------------------------------
// SandboxRuntime
// ---------------------------------------------------------------------------

/// Sandbox-based container runtime for macOS.
///
/// Uses Apple's Seatbelt (`sandbox_init`) to run each container as a native
/// macOS process with mandatory access control. The rootfs is APFS-cloned
/// from pulled OCI images for copy-on-write isolation.
///
/// Monotonically increasing counter used to make staging directory names
/// unique across concurrent calls within the same process. Combined with
/// PID and nanosecond timestamp to prevent collisions.
static STAGING_COUNTER: AtomicU64 = AtomicU64::new(0);

/// GPU access (Metal/MPS) runs at 100% native performance -- no
/// virtualization overhead. Each container gets a generated `.sb` profile
/// that precisely whitelists the `IOKit` classes, Mach services, and filesystem
/// paths required for its workload.
pub struct SandboxRuntime {
    /// Runtime configuration.
    config: MacSandboxConfig,
    /// Active containers keyed by directory name (e.g., "myservice-1").
    containers: Arc<RwLock<HashMap<String, SandboxContainer>>>,
    /// Pulled image rootfs paths keyed by sanitized image name.
    image_rootfs: Arc<RwLock<HashMap<String, PathBuf>>>,
    /// Auth context for container-to-host API authentication.
    auth_context: Option<crate::runtime::ContainerAuthContext>,
}

impl std::fmt::Debug for SandboxRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxRuntime")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl SandboxRuntime {
    /// Create a new sandbox runtime with the given configuration.
    ///
    /// Creates the required directory hierarchy under `config.data_dir`:
    /// - `containers/` -- per-container state
    /// - `images/` -- pulled OCI image rootfs
    ///
    /// # Errors
    ///
    /// Returns an error if the required directories cannot be created.
    pub fn new(
        config: MacSandboxConfig,
        auth_context: Option<crate::runtime::ContainerAuthContext>,
    ) -> Result<Self> {
        std::fs::create_dir_all(&config.data_dir).map_err(|e| {
            AgentError::Configuration(format!(
                "Failed to create data dir {}: {e}",
                config.data_dir.display(),
            ))
        })?;
        std::fs::create_dir_all(&config.log_dir).map_err(|e| {
            AgentError::Configuration(format!(
                "Failed to create log dir {}: {e}",
                config.log_dir.display(),
            ))
        })?;
        std::fs::create_dir_all(config.data_dir.join("containers")).map_err(|e| {
            AgentError::Configuration(format!("Failed to create containers dir: {e}"))
        })?;
        std::fs::create_dir_all(config.data_dir.join("images"))
            .map_err(|e| AgentError::Configuration(format!("Failed to create images dir: {e}")))?;
        tracing::info!(
            data_dir = %config.data_dir.display(),
            log_dir = %config.log_dir.display(),
            gpu_access = config.gpu_access,
            "macOS sandbox runtime initialized"
        );

        Ok(Self {
            config,
            containers: Arc::new(RwLock::new(HashMap::new())),
            image_rootfs: Arc::new(RwLock::new(HashMap::new())),
            auth_context,
        })
    }

    /// Get the runtime configuration.
    #[must_use]
    pub fn config(&self) -> &MacSandboxConfig {
        &self.config
    }

    /// Generate a directory name for a container from its [`ContainerId`].
    fn container_dir_name(id: &ContainerId) -> String {
        format!("{}-{}", id.service, id.replica)
    }

    /// Get the base container state directory for a container.
    fn container_dir(&self, id: &ContainerId) -> PathBuf {
        self.config
            .data_dir
            .join("containers")
            .join(Self::container_dir_name(id))
    }

    /// Get the images base directory.
    fn images_dir(&self) -> PathBuf {
        self.config.data_dir.join("images")
    }

    /// Register a local directory as a pre-built image rootfs.
    ///
    /// This allows using local directories (e.g., host system binaries) as
    /// image sources without pulling from a registry. The directory is
    /// copied/cloned to the standard image location and tracked for use
    /// by `create_container`.
    ///
    /// Used by E2E tests to provide macOS-native binaries, and can be used
    /// by the builder to register locally-built images.
    ///
    /// # Errors
    ///
    /// Returns an error if the image directory cannot be created or the
    /// rootfs cannot be cloned.
    ///
    /// # Panics
    ///
    /// Panics if the system clock is before the Unix epoch.
    #[allow(clippy::too_many_lines)]
    pub async fn register_local_rootfs(&self, image: &str, source_dir: &Path) -> Result<()> {
        let safe_name = sanitize_image_name(image);
        let image_dir = self.images_dir().join(&safe_name);
        let rootfs_dir = image_dir.join("rootfs");

        // Fast path: already on disk
        if rootfs_dir.exists() {
            let mut images = self.image_rootfs.write().await;
            images.insert(safe_name, rootfs_dir);
            return Ok(());
        }

        // Ensure parent dir exists
        tokio::fs::create_dir_all(&image_dir)
            .await
            .map_err(|e| AgentError::PullFailed {
                image: image.to_string(),
                reason: format!("Failed to create image dir: {e}"),
            })?;

        // Clone to a unique staging directory to avoid races when multiple
        // runtime instances register the same image concurrently.
        let staging_name = format!(
            ".rootfs-staging-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            STAGING_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let staging_dir = image_dir.join(&staging_name);

        clone_directory_recursive(source_dir, &staging_dir)
            .await
            .map_err(|e| {
                let _ = std::fs::remove_dir_all(&staging_dir);
                AgentError::PullFailed {
                    image: image.to_string(),
                    reason: format!(
                        "Failed to clone local rootfs from {}: {e}",
                        source_dir.display(),
                    ),
                }
            })?;

        // Atomic rename -- only one caller wins the race
        if tokio::fs::rename(&staging_dir, &rootfs_dir).await.is_err() {
            // Race loser: clean up staging, use winner's rootfs
            let _ = tokio::fs::remove_dir_all(&staging_dir).await;
            if !rootfs_dir.exists() {
                return Err(AgentError::PullFailed {
                    image: image.to_string(),
                    reason: "Failed to finalize rootfs and no other clone succeeded".into(),
                });
            }
        }

        let mut images = self.image_rootfs.write().await;
        images.insert(safe_name, rootfs_dir.clone());

        tracing::info!(
            image = %image,
            source = %source_dir.display(),
            rootfs = %rootfs_dir.display(),
            "Registered local rootfs as image"
        );

        Ok(())
    }

    /// Collect the set of sanitized image directory names that are still
    /// referenced by a container.
    ///
    /// The union of two sources is returned, using the identical derivation as
    /// `create_container` (`sanitize_image_name(&spec.image.name.to_string())`):
    /// 1. Every in-memory container in the `containers` map.
    /// 2. Every on-disk `containers/{dir}/config.json` (containers are not
    ///    restored into memory on daemon restart, so the on-disk specs are the
    ///    authoritative in-use set after a restart).
    ///
    /// Best-effort for source (2): a missing or unparseable `config.json` is
    /// logged at `warn` and skipped (it is treated as referencing nothing). A
    /// missing `containers/` directory contributes an empty set with no warning.
    async fn referenced_image_dirs(&self) -> HashSet<String> {
        let mut referenced = HashSet::new();

        // Source 1: in-memory containers.
        {
            let containers = self.containers.read().await;
            for container in containers.values() {
                referenced.insert(sanitize_image_name(&container.spec.image.name.to_string()));
            }
        }

        // Source 2: on-disk container config.json files.
        let containers_dir = self.config.data_dir.join("containers");
        match tokio::fs::read_dir(&containers_dir).await {
            Ok(mut entries) => {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    let path = entry.path();
                    if !path.is_dir() {
                        continue;
                    }
                    let dir_name = entry.file_name();
                    let config_path = path.join("config.json");
                    let bytes = match tokio::fs::read(&config_path).await {
                        Ok(bytes) => bytes,
                        Err(e) => {
                            tracing::warn!(
                                container = %dir_name.to_string_lossy(),
                                error = %e,
                                "prune: failed to read container config.json; \
                                 not counting it as referencing any image"
                            );
                            continue;
                        }
                    };
                    match serde_json::from_slice::<ServiceSpec>(&bytes) {
                        Ok(spec) => {
                            referenced.insert(sanitize_image_name(&spec.image.name.to_string()));
                        }
                        Err(e) => {
                            tracing::warn!(
                                container = %dir_name.to_string_lossy(),
                                error = %e,
                                "prune: failed to parse container config.json; \
                                 not counting it as referencing any image"
                            );
                        }
                    }
                }
            }
            Err(_) => {
                // Missing containers dir => nothing referenced from disk.
            }
        }

        referenced
    }

    /// Sum the LOGICAL byte size of every file under `path` (recursively).
    ///
    /// This is the sum of file lengths, NOT the number of physical blocks freed.
    /// Image rootfs directories are APFS copy-on-write clones, so multiple
    /// containers/images may share the same underlying blocks; the actual disk
    /// space reclaimed by deleting `path` can therefore be less than the value
    /// returned here. It is used only to populate `PruneResult::space_reclaimed`
    /// as a best-effort estimate.
    ///
    /// Best-effort: any `read_dir`/`metadata` error is silently skipped.
    async fn dir_size_bytes(path: &Path) -> u64 {
        let mut total: u64 = 0;
        let mut stack: Vec<PathBuf> = vec![path.to_path_buf()];

        while let Some(dir) = stack.pop() {
            let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
                continue;
            };
            while let Ok(Some(entry)) = entries.next_entry().await {
                let Ok(file_type) = entry.file_type().await else {
                    continue;
                };
                if file_type.is_dir() {
                    stack.push(entry.path());
                } else if file_type.is_file() {
                    if let Ok(meta) = entry.metadata().await {
                        total = total.saturating_add(meta.len());
                    }
                }
            }
        }

        total
    }
}

// ---------------------------------------------------------------------------
// Runtime trait implementation
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
impl Runtime for SandboxRuntime {
    /// Pull an image to local storage with default policy (`IfNotPresent`).
    async fn pull_image(&self, image: &str) -> Result<()> {
        self.pull_image_with_policy(
            image,
            zlayer_spec::PullPolicy::IfNotPresent,
            None,
            zlayer_spec::SourcePolicy::default(),
        )
        .await
    }

    /// Pull an image to local storage with a specific policy.
    ///
    /// Uses `zlayer_registry` to pull OCI image layers and extract them to
    /// `{data_dir}/images/{sanitized_name}/rootfs/`. On macOS, OCI images
    /// from registries contain Linux binaries -- the sandbox runtime expects
    /// macOS-native binaries or cross-platform scripts.
    ///
    /// The `_auth` parameter is accepted for trait conformance (§3.10) but
    /// currently ignored: credentials flow through the existing
    /// `AuthResolver` hostname lookup. Callers that need inline auth should
    /// use the Docker runtime.
    #[allow(clippy::too_many_lines)]
    async fn pull_image_with_policy(
        &self,
        image: &str,
        policy: zlayer_spec::PullPolicy,
        _auth: Option<&RegistryAuth>,
        _source: zlayer_spec::SourcePolicy,
    ) -> Result<()> {
        let safe_name = sanitize_image_name(image);
        let image_dir = self.images_dir().join(&safe_name);
        let rootfs_dir = image_dir.join("rootfs");

        match policy {
            zlayer_spec::PullPolicy::Always | zlayer_spec::PullPolicy::Newer => {
                /* always re-pull; drift detection happens at the service layer */
            }
            zlayer_spec::PullPolicy::IfNotPresent => {
                if rootfs_dir.exists() {
                    tracing::debug!(image = %image, "Image already present, skipping pull");
                    // Backfill the ref file for pre-existing images so a later
                    // `list_images` shows the real reference. Best-effort and only
                    // when missing, so this stays a cheap no-op on the hot path.
                    let ref_path = image_dir.join("ref");
                    if matches!(ref_path.try_exists(), Ok(false)) {
                        if let Err(e) = tokio::fs::write(&ref_path, image.as_bytes()).await {
                            tracing::debug!(
                                image = %image,
                                error = %e,
                                "sandbox: failed to backfill image ref file"
                            );
                        }
                    }
                    // Ensure it is tracked
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
            "Pulling image for macOS sandbox runtime \
             (note: sandbox runtime expects macOS-native images)"
        );

        tokio::fs::create_dir_all(&rootfs_dir)
            .await
            .map_err(|e| AgentError::PullFailed {
                image: image.to_string(),
                reason: format!("Failed to create rootfs dir: {e}"),
            })?;

        // Record the ORIGINAL image reference alongside the sanitized rootfs so a
        // later `list_images` can display the real reference instead of the
        // sanitized directory name. Best-effort: a write failure never fails the
        // pull (the image is still usable, just shows its dir name).
        let ref_path = image_dir.join("ref");
        if let Err(e) = tokio::fs::write(&ref_path, image.as_bytes()).await {
            tracing::debug!(
                image = %image,
                error = %e,
                "sandbox: failed to write image ref file; list_images will fall back to dir name"
            );
        }

        // Use zlayer-registry to pull and extract OCI image layers.
        // Build a blob cache in the images directory for layer deduplication.
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
        // Honor ~/.docker/config.json (AuthConfig default = DockerConfig) so
        // `zlayer login` creds / Docker Hub auth apply instead of anonymous.
        let auth =
            zlayer_core::AuthResolver::new(zlayer_core::AuthConfig::default()).resolve(image);

        let layers = puller
            .pull_image(image, &auth)
            .await
            .map_err(|e| AgentError::PullFailed {
                image: image.to_string(),
                reason: format!("Failed to pull image layers: {e}"),
            })?;

        // Persist the OCI image CONFIG blob into the same `blobs.redb` while we
        // still have the network. `pull_image` caches the manifest + layers but
        // NOT the config blob, and the config's `os` field is what the
        // composite's LOCAL-ONLY dispatch inspection
        // (`fetch_image_os_in_cache_only`) reads to route an image correctly on a
        // later `create_container` with NO network. Caching it here is what lets
        // a macOS-native bundle pulled through the sandbox resolve `os=darwin`
        // locally (so it never gets mis-routed to the Linux VM) even under a
        // Docker Hub rate-limit. Non-fatal: a config-blob miss only costs the
        // local OS hint.
        if let Err(e) = puller.pull_image_config(image, &auth).await {
            tracing::debug!(
                image = %image,
                error = %e,
                "sandbox: failed to cache OCI config blob for local OS inspection; \
                 dispatch will rely on its fallthrough",
            );
        }

        tracing::info!(
            image = %image,
            layer_count = layers.len(),
            "Extracting layers to image rootfs"
        );

        // Extract layers to rootfs
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

    /// Prune unused image rootfs directories and dangling blob-cache entries.
    ///
    /// An image directory under `{data_dir}/images/` is removed when no
    /// container references it (see [`Self::referenced_image_dirs`]). The
    /// `blobs.redb` dedup cache is preserved as a directory entry and never
    /// itself pruned as an image; after the rootfs sweep a secondary best-effort
    /// pass first removes the cached manifest entries (manifest, digest, and
    /// original-ref keys) belonging to each pruned image — read from that
    /// image's `ref` file before its directory was deleted — so the layer blobs
    /// those manifests pinned become unreferenced, then garbage-collects every
    /// blob no longer referenced by any remaining cached manifest. Images
    /// pulled before the `ref` file existed cannot have their manifests
    /// identified, so their manifest entries are left in place (their blobs
    /// simply aren't reclaimed this pass).
    ///
    /// `space_reclaimed` is a best-effort LOGICAL byte estimate (see
    /// [`Self::dir_size_bytes`]); APFS copy-on-write sharing means the actual
    /// disk freed may be smaller.
    ///
    /// # Errors
    ///
    /// Returns [`AgentError::Internal`] if the images directory exists but
    /// cannot be enumerated. Per-image removal failures and the blob-cache pass
    /// are best-effort: they are logged and never fail the call. A missing
    /// images directory yields an empty [`PruneResult`].
    async fn prune_images(&self) -> Result<PruneResult> {
        let referenced = self.referenced_image_dirs().await;

        let images_dir = self.images_dir();
        let mut entries = match tokio::fs::read_dir(&images_dir).await {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(PruneResult::default());
            }
            Err(e) => {
                return Err(AgentError::Internal(format!(
                    "failed to read images dir {}: {e}",
                    images_dir.display()
                )));
            }
        };

        let mut deleted: Vec<String> = Vec::new();
        let mut space_reclaimed: u64 = 0;
        // Original image references of pruned dirs, read from each `{dir}/ref`
        // before deletion, used to evict their manifest cache entries below.
        let mut pruned_refs: Vec<String> = Vec::new();

        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();

            // Preserve the dedup blob cache and any non-directory entries.
            if name == "blobs.redb" {
                continue;
            }
            match entry.file_type().await {
                Ok(ft) if ft.is_dir() => {}
                _ => continue,
            }

            // Skip images still referenced by a container.
            if referenced.contains(&name) {
                continue;
            }

            let path = entry.path();
            let size = Self::dir_size_bytes(&path).await;

            // Capture the ORIGINAL image reference (written by
            // `pull_image_with_policy`) before the directory is removed, so the
            // secondary cache pass can drop this image's manifest entries and
            // let its layer blobs become reclaimable. Best-effort: a missing or
            // unreadable `ref` file (images pulled before that file existed)
            // just means this image's manifests aren't GC'd this pass.
            let ref_path = path.join("ref");
            match tokio::fs::read_to_string(&ref_path).await {
                Ok(contents) => {
                    let r = contents.trim();
                    if r.is_empty() {
                        tracing::debug!(
                            image = %name,
                            "prune: empty ref file; manifest entries not reclaimable"
                        );
                    } else {
                        pruned_refs.push(r.to_string());
                    }
                }
                Err(e) => {
                    tracing::debug!(
                        image = %name,
                        error = %e,
                        "prune: no readable ref file; manifest entries not reclaimable"
                    );
                }
            }

            if let Err(e) = tokio::fs::remove_dir_all(&path).await {
                tracing::warn!(
                    image = %name,
                    error = %e,
                    "prune: failed to remove unused image rootfs; skipping"
                );
                continue;
            }

            tracing::info!(
                image = %name,
                bytes = size,
                "prune: removed unused image rootfs"
            );
            deleted.push(name);
            space_reclaimed = space_reclaimed.saturating_add(size);
        }

        // Drop pruned images from the in-memory rootfs tracking map.
        if !deleted.is_empty() {
            let mut images = self.image_rootfs.write().await;
            for name in &deleted {
                images.remove(name);
            }
        }

        // Secondary best-effort pass: garbage-collect dangling blobs from the
        // dedup cache. Never fails the prune call.
        let cache_path = images_dir.join("blobs.redb");
        if matches!(cache_path.try_exists(), Ok(true)) {
            match zlayer_registry::CacheType::persistent_at(&cache_path)
                .build()
                .await
            {
                Ok(cache) => {
                    // Evict the manifest entries of every pruned image first so
                    // the blobs they pinned become unreferenced and the GC below
                    // can reclaim them. All three keys canonicalize the ref
                    // internally, so the original reference maps to exactly the
                    // keys the pull wrote. Delete failures are non-fatal.
                    for r in &pruned_refs {
                        for key in [
                            zlayer_registry::manifest_cache_key(r),
                            zlayer_registry::manifest_digest_cache_key(r),
                            zlayer_registry::manifest_orig_cache_key(r),
                        ] {
                            if let Err(e) = cache.delete(&key).await {
                                tracing::warn!(
                                    image = %r,
                                    key = %key,
                                    error = %e,
                                    "prune: failed to evict manifest cache entry; continuing"
                                );
                            }
                        }
                    }

                    match zlayer_registry::prune_dangling_blobs(cache.as_ref().as_ref()).await {
                        Ok((blob_deleted, blob_bytes)) => {
                            deleted.extend(blob_deleted);
                            space_reclaimed = space_reclaimed.saturating_add(blob_bytes);
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                "prune: failed to garbage-collect dangling blobs; skipping"
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "prune: failed to open blob cache for blob GC; skipping"
                    );
                }
            }
        }

        Ok(PruneResult {
            deleted,
            space_reclaimed,
        })
    }

    /// List the images stored under `{data_dir}/images/`.
    ///
    /// Each image is one subdirectory named after the sanitized reference
    /// (see [`sanitize_image_name`]); the `blobs.redb` dedup cache and any
    /// non-directory entries are skipped. For each image:
    /// - `reference` is the trimmed contents of `{dir}/ref` (the original
    ///   reference recorded by `pull_image_with_policy`) when that file is
    ///   present and non-empty, otherwise the sanitized directory name (older
    ///   images pulled before the `ref` file existed).
    /// - `size_bytes` is the LOGICAL byte sum of the extracted `rootfs/` tree
    ///   (see [`Self::dir_size_bytes`]); metadata files like `ref` are excluded
    ///   so the value reflects the image payload. APFS copy-on-write sharing
    ///   means the true on-disk footprint may be smaller, and an image
    ///   directory missing its `rootfs/` reports `Some(0)`.
    /// - `digest` is the registry digest recorded under the manifest-digest
    ///   cache key for the resolved reference, when the `blobs.redb` cache opens
    ///   and the entry is present. Any cache failure or miss leaves it `None`.
    ///
    /// The blob cache is opened once before the loop (best-effort); if it
    /// cannot be opened, every entry's `digest` is `None`. Entries are returned
    /// in directory-iteration order (the youki backend does not sort either).
    ///
    /// # Errors
    ///
    /// Returns [`AgentError::Internal`] if the images directory exists but
    /// cannot be enumerated. A missing images directory yields an empty list.
    async fn list_images(&self) -> Result<Vec<ImageInfo>> {
        let images_dir = self.images_dir();
        let mut entries = match tokio::fs::read_dir(&images_dir).await {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Vec::new());
            }
            Err(e) => {
                return Err(AgentError::Internal(format!(
                    "failed to read images dir {}: {e}",
                    images_dir.display()
                )));
            }
        };

        // Open the dedup cache once before the loop. Best-effort: when it is
        // absent or fails to open, every image's digest is left as `None`.
        let cache_path = images_dir.join("blobs.redb");
        let cache: Option<Arc<Box<dyn BlobCacheBackend>>> =
            if matches!(cache_path.try_exists(), Ok(true)) {
                match zlayer_registry::CacheType::persistent_at(&cache_path)
                    .build()
                    .await
                {
                    Ok(cache) => Some(cache),
                    Err(e) => {
                        tracing::debug!(
                            error = %e,
                            "list_images: failed to open blob cache; digests will be None"
                        );
                        None
                    }
                }
            } else {
                None
            };

        let mut images = Vec::new();

        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();

            // Skip the dedup blob cache and any non-directory entries.
            if name == "blobs.redb" {
                continue;
            }
            match entry.file_type().await {
                Ok(ft) if ft.is_dir() => {}
                _ => continue,
            }

            let dir = entry.path();

            // Prefer the ORIGINAL image reference recorded at pull time; fall
            // back to the sanitized directory name for images pulled before the
            // `ref` file existed.
            let reference = match tokio::fs::read_to_string(dir.join("ref")).await {
                Ok(contents) => {
                    let r = contents.trim();
                    if r.is_empty() {
                        name.clone()
                    } else {
                        r.to_string()
                    }
                }
                Err(_) => name.clone(),
            };

            // LOGICAL byte sum of the extracted `rootfs/` tree only; metadata
            // files like `ref` are deliberately excluded so the reported size
            // matches the image payload (APFS CoW caveat documented on
            // `dir_size_bytes`). A directory without a `rootfs/` reports
            // `Some(0)` (read_dir error on the missing path is skipped).
            let size_bytes = Some(Self::dir_size_bytes(&dir.join("rootfs")).await);

            // Best-effort registry digest from the manifest-digest cache key.
            // The key canonicalizes the reference internally, matching what the
            // pull wrote. Any failure or miss leaves the digest `None`.
            let digest = match cache.as_ref() {
                Some(cache) => cache
                    .get(&zlayer_registry::manifest_digest_cache_key(&reference))
                    .await
                    .ok()
                    .flatten()
                    .and_then(|bytes| String::from_utf8(bytes).ok()),
                None => None,
            };

            images.push(ImageInfo {
                reference,
                digest,
                size_bytes,
            });
        }

        Ok(images)
    }

    /// Remove an image's rootfs directory and its dangling manifest cache
    /// entries.
    ///
    /// The image directory is `{data_dir}/images/{sanitize_image_name(image)}`.
    /// Before deleting it, the image's `ref` file is read so the cache-cleanup
    /// pass can evict the manifest entries for both the supplied `image`
    /// argument and the recorded original reference.
    ///
    /// When the image is still referenced by a container (see
    /// [`Self::referenced_image_dirs`]) and `force` is `false`, the removal is
    /// refused. The directory is then removed with `remove_dir_all`, the image
    /// is dropped from the in-memory rootfs tracking map, and a best-effort
    /// cache pass deletes the three manifest keys (manifest, digest, and
    /// original-ref) for each candidate reference. Layer blobs are deliberately
    /// NOT deleted here: a blob can be shared by several images, so deleting it
    /// directly could corrupt an unrelated image. Orphaned blobs left behind by
    /// the evicted manifests are reclaimed by the next [`Self::prune_images`].
    ///
    /// # Errors
    ///
    /// - [`AgentError::NotFound`] if no image directory exists for `image`.
    /// - [`AgentError::InvalidSpec`] if the image is in use by a container and
    ///   `force` is `false`.
    /// - [`AgentError::Internal`] if the image directory exists but cannot be
    ///   removed.
    ///
    /// The blob-cache cleanup is best-effort: failures are logged and never
    /// fail the call.
    async fn remove_image(&self, image: &str, force: bool) -> Result<()> {
        let safe = sanitize_image_name(image);
        let dir = self.images_dir().join(&safe);

        if !matches!(dir.try_exists(), Ok(true)) {
            return Err(AgentError::NotFound {
                container: image.to_string(),
                reason: format!("image '{image}' not found"),
            });
        }

        // Refuse to remove an image still referenced by a container unless the
        // caller forces it. An in-use conflict is a client-state issue, so map
        // it to InvalidSpec (400) rather than Internal (500).
        if !force && self.referenced_image_dirs().await.contains(&safe) {
            return Err(AgentError::InvalidSpec(format!(
                "image '{image}' is in use by a container; pass --force to remove it"
            )));
        }

        // Capture the ORIGINAL image reference (written by
        // `pull_image_with_policy`) before deleting the directory, so the cache
        // pass can evict this image's manifest entries under the recorded ref as
        // well as the supplied argument.
        let ref_file = match tokio::fs::read_to_string(dir.join("ref")).await {
            Ok(contents) => {
                let r = contents.trim();
                if r.is_empty() {
                    None
                } else {
                    Some(r.to_string())
                }
            }
            Err(_) => None,
        };

        tokio::fs::remove_dir_all(&dir).await.map_err(|e| {
            AgentError::Internal(format!("failed to remove image dir {}: {e}", dir.display()))
        })?;

        // Drop the image from the in-memory rootfs tracking map.
        {
            let mut images = self.image_rootfs.write().await;
            images.remove(&safe);
        }

        // Best-effort cache cleanup: evict the manifest entries that pinned this
        // image's layer blobs. We deliberately do NOT delete the layer blobs
        // directly — a blob can be shared across multiple images, so removing it
        // here could corrupt an unrelated image. Once the manifests are evicted,
        // the now-orphaned blobs are reclaimed by the next `prune_images` GC.
        let cache_path = self.images_dir().join("blobs.redb");
        if matches!(cache_path.try_exists(), Ok(true)) {
            match zlayer_registry::CacheType::persistent_at(&cache_path)
                .build()
                .await
            {
                Ok(cache) => {
                    // Dedupe {image arg, ref-file content}: when the ref file
                    // equals the argument we only evict once.
                    let mut candidates: Vec<String> = vec![image.to_string()];
                    if let Some(r) = ref_file {
                        if !candidates.contains(&r) {
                            candidates.push(r);
                        }
                    }
                    for r in &candidates {
                        // All three keys canonicalize the ref internally, so the
                        // reference maps to exactly the keys the pull wrote.
                        for key in [
                            zlayer_registry::manifest_cache_key(r),
                            zlayer_registry::manifest_digest_cache_key(r),
                            zlayer_registry::manifest_orig_cache_key(r),
                        ] {
                            if let Err(e) = cache.delete(&key).await {
                                tracing::warn!(
                                    image = %r,
                                    key = %key,
                                    error = %e,
                                    "remove_image: failed to evict manifest cache entry; continuing"
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "remove_image: failed to open blob cache for manifest eviction; skipping"
                    );
                }
            }
        }

        tracing::info!(image = %image, "Removed image (sandbox)");
        Ok(())
    }

    /// Create a container.
    ///
    /// 1. APFS-clones the base image rootfs to a per-container directory.
    /// 2. Generates a Seatbelt profile based on the [`ServiceSpec`] (GPU, network, filesystem).
    /// 3. Writes the profile to `{container_dir}/sandbox.sb`.
    /// 4. Writes container metadata to `{container_dir}/config.json`.
    /// 5. Stores the container as [`ContainerState::Pending`].
    #[allow(clippy::too_many_lines)]
    async fn create_container(&self, id: &ContainerId, spec: &ServiceSpec) -> Result<()> {
        let dir_name = Self::container_dir_name(id);
        let container_dir = self.container_dir(id);
        let rootfs_dir = container_dir.join("rootfs");

        // Clean up stale container directory if it exists
        if container_dir.exists() {
            tracing::warn!(
                container = %dir_name,
                "Stale container directory found, cleaning up"
            );
            if let Err(e) = tokio::fs::remove_dir_all(&container_dir).await {
                tracing::warn!(
                    container = %dir_name,
                    error = %e,
                    "Failed to remove stale container directory"
                );
            }
        }

        // Create container state directory
        tokio::fs::create_dir_all(&container_dir)
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: dir_name.clone(),
                reason: format!(
                    "Failed to create container dir {}: {e}",
                    container_dir.display(),
                ),
            })?;

        // Create tmp directory within the container
        tokio::fs::create_dir_all(container_dir.join("tmp"))
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: dir_name.clone(),
                reason: format!("Failed to create container tmp dir: {e}"),
            })?;

        // Locate the base image rootfs
        let image_name_str = spec.image.name.to_string();
        let safe_image_name = sanitize_image_name(&image_name_str);
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

        // APFS-clone the base image rootfs to this container's rootfs
        tracing::debug!(
            container = %dir_name,
            src = %image_rootfs.display(),
            dst = %rootfs_dir.display(),
            "Cloning rootfs (APFS CoW)"
        );
        clone_directory_recursive(&image_rootfs, &rootfs_dir)
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: dir_name.clone(),
                reason: format!(
                    "Failed to clone rootfs from {} to {}: {e}",
                    image_rootfs.display(),
                    rootfs_dir.display(),
                ),
            })?;

        // Determine GPU access level from spec
        let gpu_access = if self.config.gpu_access {
            if let Some(ref gpu) = spec.resources.gpu {
                if gpu.vendor == "apple" {
                    match gpu.mode.as_deref() {
                        Some("mps") => GpuAccess::MpsOnly,
                        _ => GpuAccess::MetalCompute,
                    }
                } else {
                    GpuAccess::None
                }
            } else {
                GpuAccess::None
            }
        } else {
            GpuAccess::None
        };

        // Reserve a unique port for this container BEFORE building the sandbox
        // profile, so the Seatbelt profile can allow binding on the assigned port.
        //
        // All macOS sandbox containers share the host network. If we let
        // multiple replicas of the same service bind the same spec-declared
        // port, the second one gets EADDRINUSE. We solve this by assigning
        // each container a dynamically allocated port (via OS port-0 binding)
        // and passing it through the PORT environment variable (respected by
        // most web frameworks: Express, Actix, Axum, Flask, etc.).
        //
        // The guard listener holds the port until start_container() spawns
        // the child process, preventing other processes from stealing it.
        let (assigned_port, port_guard) = reserve_port().map_err(|e| AgentError::CreateFailed {
            id: dir_name.clone(),
            reason: format!("Failed to reserve a dynamic port for sandbox container: {e}",),
        })?;

        tracing::info!(
            container = %dir_name,
            assigned_port = assigned_port,
            "Reserved dynamic port for sandbox container"
        );

        // Determine network access from spec endpoints, including the assigned port
        let mut network_access = build_network_access(spec);

        // Ensure the Seatbelt profile allows binding on the dynamically assigned port.
        // Without this, the sandbox would deny the child's bind() call.
        match &mut network_access {
            NetworkAccess::LocalhostOnly {
                ref mut bind_ports,
                ref mut connect_ports,
            } => {
                if !bind_ports.contains(&assigned_port) {
                    bind_ports.push(assigned_port);
                }
                if !connect_ports.contains(&assigned_port) {
                    connect_ports.push(assigned_port);
                }
            }
            NetworkAccess::None => {
                // If network was fully denied but we need a port, upgrade to localhost-only
                network_access = NetworkAccess::LocalhostOnly {
                    bind_ports: vec![assigned_port],
                    connect_ports: vec![assigned_port, 53, 80, 443],
                };
            }
            NetworkAccess::Full => {
                // Full access already allows all ports
            }
        }

        // Determine writable directories from spec volumes
        let writable_dirs = build_writable_dirs(spec, &container_dir);

        // Parse memory limit
        let memory_limit = spec
            .resources
            .memory
            .as_ref()
            .and_then(|m| parse_memory_string(m));

        let mut sandbox_config = SandboxConfig {
            rootfs_dir: rootfs_dir.clone(),
            workspace_dir: container_dir.clone(),
            gpu_access,
            network_access,
            writable_dirs,
            readonly_dirs: vec![],
            max_files: 4096,
            cpu_time_limit: None,
            memory_limit,
        };

        // Allow sandbox write access to the API socket so the container can
        // communicate back to the host ZLayer API over the Unix domain socket.
        if let Some(ref auth_ctx) = self.auth_context {
            sandbox_config
                .writable_dirs
                .push(PathBuf::from(&auth_ctx.socket_path));
        }

        // Generate Seatbelt profile
        let profile = generate_sandbox_profile(&sandbox_config);

        // Write profile to disk
        let profile_path = container_dir.join("sandbox.sb");
        tokio::fs::write(&profile_path, &profile)
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: dir_name.clone(),
                reason: format!("Failed to write Seatbelt profile: {e}"),
            })?;

        // Write config to disk (for use by start_container)
        let config_json =
            serde_json::to_string_pretty(spec).map_err(|e| AgentError::CreateFailed {
                id: dir_name.clone(),
                reason: format!("Failed to serialize spec: {e}"),
            })?;
        tokio::fs::write(container_dir.join("config.json"), &config_json)
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: dir_name.clone(),
                reason: format!("Failed to write config.json: {e}"),
            })?;

        let stdout_path = container_dir.join("stdout.log");
        let stderr_path = container_dir.join("stderr.log");

        // Register the container as pending
        let container = SandboxContainer {
            pid: 0,
            state: ContainerState::Pending,
            state_dir: container_dir,
            rootfs_dir,
            stdout_path,
            stderr_path,
            started_at: None,
            spec: spec.clone(),
            sandbox_config,
            memory_limit,
            watchdog_handle: None,
            assigned_port,
            port_guard: Some(port_guard),
        };

        let mut containers = self.containers.write().await;
        containers.insert(dir_name.clone(), container);

        tracing::info!(
            container = %dir_name,
            image = %spec.image.name,
            port = assigned_port,
            "Container created (sandbox)"
        );

        Ok(())
    }

    /// Start a container.
    ///
    /// Reads the saved spec and Seatbelt profile, resolves the entrypoint,
    /// and forks a child process with `sandbox_init()` applied via `pre_exec`.
    /// Stdout/stderr are redirected to log files. If a memory limit is
    /// configured, a watchdog task is spawned to enforce it.
    ///
    /// The container's dynamically assigned port is injected as `PORT` and
    /// `ZLAYER_PORT` environment variables. The port guard listener (which
    /// was holding the port since `create_container()`) is dropped immediately
    /// after the child process spawns, freeing the port for the child to bind.
    #[allow(clippy::too_many_lines)]
    async fn start_container(&self, id: &ContainerId) -> Result<()> {
        let dir_name = Self::container_dir_name(id);

        // Extract what we need from the container state, then release the lock
        // so spawn_sandboxed_process (which is blocking) does not hold it.
        let (
            rootfs_dir,
            stdout_path,
            stderr_path,
            spec,
            sandbox_config,
            memory_limit,
            assigned_port,
        ) = {
            let containers = self.containers.read().await;
            let container = containers
                .get(&dir_name)
                .ok_or_else(|| AgentError::NotFound {
                    container: dir_name.clone(),
                    reason: "Container not created".to_string(),
                })?;
            (
                container.rootfs_dir.clone(),
                container.stdout_path.clone(),
                container.stderr_path.clone(),
                container.spec.clone(),
                container.sandbox_config.clone(),
                container.memory_limit,
                container.assigned_port,
            )
        };

        // Read the generated Seatbelt profile from disk
        let profile_path = self.container_dir(id).join("sandbox.sb");
        let profile = tokio::fs::read_to_string(&profile_path)
            .await
            .map_err(|e| AgentError::StartFailed {
                id: dir_name.clone(),
                reason: format!("Failed to read Seatbelt profile: {e}"),
            })?;

        // Resolve the command to execute
        let (program, args) = resolve_entrypoint(&spec, &rootfs_dir)?;

        tracing::info!(
            container = %dir_name,
            program = %program,
            args = ?args,
            port = assigned_port,
            "Starting sandboxed process"
        );

        // Drop the port guard right before spawning.
        //
        // The guard has been holding the port since create_container() to prevent
        // other processes from claiming it. We must release it so the child can
        // bind the same port. The window between drop and child bind() is
        // microseconds (process startup time).
        {
            let mut containers = self.containers.write().await;
            if let Some(container) = containers.get_mut(&dir_name) {
                // Drop the guard listener to free the port for the child
                container.port_guard.take();
            }
        }

        // Mint a per-container JWT so the sandboxed process can authenticate
        // back to the host ZLayer API.
        let auth_env = self.auth_context.as_ref().map(|ctx| {
            let token = crate::auth::mint_container_token(
                &ctx.jwt_secret,
                &id.service,
                &format!("{}-{}", id.service, id.replica),
                std::time::Duration::from_secs(86400 * 365),
            )
            .unwrap_or_default();
            (ctx.api_url.clone(), token, ctx.socket_path.clone())
        });

        // Spawn the sandboxed process in a blocking task so that the fork+exec
        // does not block the tokio reactor (which would prevent timers and other
        // futures from making progress on a current_thread runtime).
        let dir_name_for_err = dir_name.clone();
        let child_pid = tokio::task::spawn_blocking(move || {
            spawn_sandboxed_process(&SandboxSpawnParams {
                program,
                args,
                sbpl_profile: profile,
                rootfs_dir,
                stdout_path,
                stderr_path,
                spec,
                sandbox_config,
                assigned_port,
                auth_env,
            })
        })
        .await
        .map_err(|e| AgentError::StartFailed {
            id: dir_name_for_err,
            reason: format!("spawn task join error: {e}"),
        })??;

        // Write PID file
        let pid_path = self.container_dir(id).join("pid");
        tokio::fs::write(&pid_path, child_pid.to_string())
            .await
            .map_err(|e| AgentError::StartFailed {
                id: dir_name.clone(),
                reason: format!("Failed to write PID file: {e}"),
            })?;

        // Update container state and optionally start memory watchdog
        let mut containers = self.containers.write().await;
        if let Some(container) = containers.get_mut(&dir_name) {
            container.pid = child_pid;
            container.state = ContainerState::Running;
            container.started_at = Some(Instant::now());

            // Start memory watchdog if a memory limit is configured
            if let Some(limit) = memory_limit {
                let pid = child_pid;
                let handle = tokio::spawn(async move {
                    memory_watchdog(pid, limit).await;
                });
                container.watchdog_handle = Some(handle);
            }
        }

        tracing::info!(
            container = %dir_name,
            pid = child_pid,
            "Sandboxed process started"
        );

        Ok(())
    }

    /// Stop a container.
    ///
    /// Sends `SIGTERM` to the process and waits up to `timeout` for graceful
    /// shutdown. If the process is still alive after the timeout, sends `SIGKILL`.
    #[allow(unsafe_code)]
    #[allow(clippy::too_many_lines)]
    #[allow(clippy::cast_possible_wrap)]
    async fn stop_container(&self, id: &ContainerId, timeout: Duration) -> Result<()> {
        let dir_name = Self::container_dir_name(id);

        // Get the PID and update state to Stopping
        let pid = {
            let mut containers = self.containers.write().await;
            let container = containers
                .get_mut(&dir_name)
                .ok_or_else(|| AgentError::NotFound {
                    container: dir_name.clone(),
                    reason: "Container not found".to_string(),
                })?;

            if container.pid == 0 {
                container.state = ContainerState::Exited { code: 0 };
                return Ok(());
            }

            container.state = ContainerState::Stopping;
            container.pid
        };

        tracing::info!(
            container = %dir_name,
            pid = pid,
            timeout = ?timeout,
            "Stopping sandboxed process"
        );

        // Send SIGTERM
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }

        // Wait for graceful shutdown with timeout
        let deadline = Instant::now() + timeout;
        loop {
            if Instant::now() >= deadline {
                break;
            }

            // Check if process has exited (non-blocking waitpid)
            let mut status: libc::c_int = 0;
            let result = unsafe { libc::waitpid(pid as i32, &raw mut status, libc::WNOHANG) };

            if result > 0 {
                // Process exited
                let exit_code = if libc::WIFEXITED(status) {
                    libc::WEXITSTATUS(status)
                } else {
                    -1
                };

                let mut containers = self.containers.write().await;
                if let Some(c) = containers.get_mut(&dir_name) {
                    c.state = ContainerState::Exited { code: exit_code };
                    if let Some(h) = c.watchdog_handle.take() {
                        h.abort();
                    }
                }
                tracing::info!(
                    container = %dir_name,
                    exit_code = exit_code,
                    "Container stopped gracefully"
                );
                return Ok(());
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // Timeout reached -- send SIGKILL
        tracing::warn!(
            container = %dir_name,
            pid = pid,
            "SIGTERM timeout, sending SIGKILL"
        );
        unsafe {
            libc::kill(pid as i32, libc::SIGKILL);
        }

        // Wait for SIGKILL to take effect (non-blocking poll with timeout,
        // because the child may have already been reaped by container_state())
        let pid_for_wait = pid;
        let exit_code = tokio::task::spawn_blocking(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            loop {
                let mut status: libc::c_int = 0;
                let result =
                    unsafe { libc::waitpid(pid_for_wait as i32, &raw mut status, libc::WNOHANG) };
                if result > 0 || result == -1 {
                    break; // reaped or already gone
                }
                if std::time::Instant::now() >= deadline {
                    break; // give up -- process already reaped elsewhere
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            -9i32
        })
        .await
        .unwrap_or(-9);

        let mut containers = self.containers.write().await;
        if let Some(c) = containers.get_mut(&dir_name) {
            c.state = ContainerState::Exited { code: exit_code };
            if let Some(h) = c.watchdog_handle.take() {
                h.abort();
            }
        }

        tracing::info!(container = %dir_name, "Container killed (SIGKILL)");
        Ok(())
    }

    /// Remove a container.
    ///
    /// Kills the process if still running, aborts the watchdog, removes the
    /// container directory (rootfs clone, profile, logs), and removes it
    /// from internal tracking.
    #[allow(unsafe_code)]
    #[allow(clippy::cast_possible_wrap)]
    async fn remove_container(&self, id: &ContainerId) -> Result<()> {
        let dir_name = Self::container_dir_name(id);

        tracing::info!(container = %dir_name, "Removing container");

        // Remove from tracking, killing the process if needed
        {
            let mut containers = self.containers.write().await;
            if let Some(mut c) = containers.remove(&dir_name) {
                // Abort watchdog if running
                if let Some(h) = c.watchdog_handle.take() {
                    h.abort();
                }

                // Kill process if still running
                if c.pid > 0
                    && matches!(c.state, ContainerState::Running | ContainerState::Stopping)
                {
                    unsafe {
                        libc::kill(c.pid as i32, libc::SIGKILL);
                    }
                    // Reap the zombie (non-blocking poll, child may already be reaped)
                    let pid = c.pid;
                    let _ = tokio::task::spawn_blocking(move || {
                        let deadline =
                            std::time::Instant::now() + std::time::Duration::from_secs(3);
                        loop {
                            let mut status: libc::c_int = 0;
                            let result = unsafe {
                                libc::waitpid(pid as i32, &raw mut status, libc::WNOHANG)
                            };
                            if result > 0 || result == -1 {
                                break;
                            }
                            if std::time::Instant::now() >= deadline {
                                break;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(50));
                        }
                    })
                    .await;
                }
            }
        }

        // Remove container directory (rootfs, logs, config, profile)
        let container_dir = self.container_dir(id);
        if container_dir.exists() {
            tokio::fs::remove_dir_all(&container_dir)
                .await
                .map_err(|e| {
                    AgentError::Internal(format!(
                        "Failed to remove container dir {}: {e}",
                        container_dir.display(),
                    ))
                })?;
        }

        tracing::info!(container = %dir_name, "Container removed");
        Ok(())
    }

    /// Get container state.
    ///
    /// If the container is in a running state, checks whether the process
    /// is still alive via `waitpid(WNOHANG)` and updates the state accordingly.
    #[allow(unsafe_code)]
    #[allow(clippy::cast_possible_wrap)]
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

        // Check if process is still alive
        if container.pid > 0 {
            let mut status: libc::c_int = 0;
            let result =
                unsafe { libc::waitpid(container.pid as i32, &raw mut status, libc::WNOHANG) };

            match result.cmp(&0) {
                std::cmp::Ordering::Greater => {
                    // Process has exited
                    let exit_code = if libc::WIFEXITED(status) {
                        libc::WEXITSTATUS(status)
                    } else if libc::WIFSIGNALED(status) {
                        -(libc::WTERMSIG(status))
                    } else {
                        -1
                    };
                    container.state = ContainerState::Exited { code: exit_code };
                    if let Some(h) = container.watchdog_handle.take() {
                        h.abort();
                    }
                }
                std::cmp::Ordering::Equal => {
                    // Process still running
                    container.state = ContainerState::Running;
                }
                std::cmp::Ordering::Less => {
                    // Error -- process disappeared
                    container.state = ContainerState::Failed {
                        reason: "Process disappeared".to_string(),
                    };
                }
            }
        }

        Ok(container.state.clone())
    }

    /// Get container logs as structured [`LogEntry`] values.
    ///
    /// If `tail > 0`, returns only the last `tail` entries.
    async fn container_logs(&self, id: &ContainerId, tail: usize) -> Result<Vec<LogEntry>> {
        let dir_name = Self::container_dir_name(id);

        let (stdout_path, stderr_path) = {
            let containers = self.containers.read().await;
            let container = containers
                .get(&dir_name)
                .ok_or_else(|| AgentError::NotFound {
                    container: dir_name.clone(),
                    reason: "Container not found".to_string(),
                })?;
            (container.stdout_path.clone(), container.stderr_path.clone())
        };

        let now = chrono::Utc::now();
        let source = LogSource::Container(id.to_string());
        let mut entries = Vec::new();

        if let Ok(stdout) = tokio::fs::read_to_string(&stdout_path).await {
            for line in stdout.lines() {
                entries.push(LogEntry {
                    timestamp: now,
                    stream: LogStream::Stdout,
                    message: line.to_string(),
                    source: source.clone(),
                    service: None,
                    deployment: None,
                });
            }
        }

        if let Ok(stderr) = tokio::fs::read_to_string(&stderr_path).await {
            for line in stderr.lines() {
                entries.push(LogEntry {
                    timestamp: now,
                    stream: LogStream::Stderr,
                    message: line.to_string(),
                    source: source.clone(),
                    service: None,
                    deployment: None,
                });
            }
        }

        // Apply tail limit
        if tail > 0 && entries.len() > tail {
            let start = entries.len() - tail;
            entries = entries.split_off(start);
        }

        Ok(entries)
    }

    /// Execute a command inside a container's sandbox.
    ///
    /// Spawns a new process with the same Seatbelt profile as the container,
    /// running in the container's rootfs directory. Captures stdout/stderr
    /// and returns `(exit_code, stdout, stderr)`.
    async fn exec(&self, id: &ContainerId, cmd: &[String]) -> Result<(i32, String, String)> {
        let dir_name = Self::container_dir_name(id);

        if cmd.is_empty() {
            return Err(AgentError::InvalidSpec(
                "exec command cannot be empty".to_string(),
            ));
        }

        let (rootfs, profile_path) = {
            let containers = self.containers.read().await;
            let container = containers
                .get(&dir_name)
                .ok_or_else(|| AgentError::NotFound {
                    container: dir_name.clone(),
                    reason: "Container not found".to_string(),
                })?;
            (
                container.rootfs_dir.clone(),
                container.state_dir.join("sandbox.sb"),
            )
        };

        // Read the Seatbelt profile (same sandbox as the main process)
        let profile = tokio::fs::read_to_string(&profile_path)
            .await
            .map_err(|e| AgentError::Internal(format!("Failed to read Seatbelt profile: {e}")))?;

        tracing::debug!(
            container = %dir_name,
            cmd = ?cmd,
            "Executing command in sandbox"
        );

        // Use sandbox-exec to run the command in the container's rootfs.
        // We pass the profile inline via `-p` to avoid needing a file path
        // that the sandbox itself can read.
        let profile_clone = profile.clone();
        let rootfs_clone = rootfs.clone();
        let cmd_clone = cmd.to_vec();

        let output = tokio::task::spawn_blocking(move || {
            std::process::Command::new("/usr/bin/sandbox-exec")
                .arg("-p")
                .arg(&profile_clone)
                .arg("--")
                .arg(&cmd_clone[0])
                .args(&cmd_clone[1..])
                .current_dir(&rootfs_clone)
                .output()
        })
        .await
        .map_err(|e| AgentError::Internal(format!("exec task join error: {e}")))?
        .map_err(|e| AgentError::Internal(format!("Failed to exec: {e}")))?;

        let exit_code = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        tracing::debug!(
            container = %dir_name,
            exit_code = exit_code,
            stdout_len = stdout.len(),
            stderr_len = stderr.len(),
            "exec completed"
        );

        Ok((exit_code, stdout, stderr))
    }

    /// Get container resource statistics.
    ///
    /// Uses macOS `proc_pidinfo` with `PROC_PIDTASKINFO` to read CPU time
    /// (user + system) and resident set size for the sandboxed process.
    async fn get_container_stats(&self, id: &ContainerId) -> Result<ContainerStats> {
        let dir_name = Self::container_dir_name(id);

        let (pid, memory_limit) = {
            let containers = self.containers.read().await;
            let container = containers
                .get(&dir_name)
                .ok_or_else(|| AgentError::NotFound {
                    container: dir_name.clone(),
                    reason: "Container not found".to_string(),
                })?;

            if container.pid == 0 {
                return Err(AgentError::Internal(
                    "Container not started -- no PID available for stats".to_string(),
                ));
            }

            (container.pid, container.memory_limit.unwrap_or(0))
        };

        // Get process stats via proc_pidinfo (blocking FFI call)
        let pid_for_stats = pid;
        let (cpu_usec, memory_bytes) =
            tokio::task::spawn_blocking(move || get_process_stats(pid_for_stats))
                .await
                .map_err(|e| AgentError::Internal(format!("stats task join error: {e}")))??;

        Ok(ContainerStats {
            cpu_usage_usec: cpu_usec,
            memory_bytes,
            memory_limit,
            timestamp: Instant::now(),
        })
    }

    /// Wait for a container to exit and return its exit code.
    ///
    /// Uses `spawn_blocking` with `waitpid` (blocking) to avoid tying up
    /// the async runtime. Updates the container state on completion.
    #[allow(unsafe_code)]
    #[allow(clippy::cast_possible_wrap)]
    async fn wait_container(&self, id: &ContainerId) -> Result<i32> {
        let dir_name = Self::container_dir_name(id);
        let pid = {
            let containers = self.containers.read().await;
            let container = containers
                .get(&dir_name)
                .ok_or_else(|| AgentError::NotFound {
                    container: dir_name.clone(),
                    reason: "Container not found".to_string(),
                })?;

            // If already exited, return immediately
            if let ContainerState::Exited { code } = &container.state {
                return Ok(*code);
            }

            if container.pid == 0 {
                return Err(AgentError::Internal(
                    "Container not started -- no PID to wait on".to_string(),
                ));
            }

            container.pid
        };

        tracing::debug!(container = %dir_name, pid = pid, "Waiting for container to exit");

        // Block on waitpid in a spawned blocking task
        let exit_code = tokio::task::spawn_blocking(move || {
            let mut status: libc::c_int = 0;
            let result = unsafe { libc::waitpid(pid as i32, &raw mut status, 0) };
            if result < 0 {
                return -1;
            }
            if libc::WIFEXITED(status) {
                libc::WEXITSTATUS(status)
            } else if libc::WIFSIGNALED(status) {
                -(libc::WTERMSIG(status))
            } else {
                -1
            }
        })
        .await
        .map_err(|e| AgentError::Internal(format!("wait task join error: {e}")))?;

        // Update state
        let mut containers = self.containers.write().await;
        if let Some(c) = containers.get_mut(&dir_name) {
            c.state = ContainerState::Exited { code: exit_code };
            if let Some(h) = c.watchdog_handle.take() {
                h.abort();
            }
        }

        tracing::info!(
            container = %dir_name,
            exit_code = exit_code,
            "Container exited"
        );

        Ok(exit_code)
    }

    /// Get container logs as structured [`LogEntry`] values.
    async fn get_logs(&self, id: &ContainerId) -> Result<Vec<LogEntry>> {
        let dir_name = Self::container_dir_name(id);

        let (stdout_path, stderr_path) = {
            let containers = self.containers.read().await;
            let container = containers
                .get(&dir_name)
                .ok_or_else(|| AgentError::NotFound {
                    container: dir_name.clone(),
                    reason: "Container not found".to_string(),
                })?;
            (container.stdout_path.clone(), container.stderr_path.clone())
        };

        let now = chrono::Utc::now();
        let source = LogSource::Container(id.to_string());
        let mut entries = Vec::new();

        // Read stdout
        if let Ok(content) = tokio::fs::read_to_string(&stdout_path).await {
            for line in content.lines() {
                entries.push(LogEntry {
                    timestamp: now,
                    stream: LogStream::Stdout,
                    message: line.to_string(),
                    source: source.clone(),
                    service: None,
                    deployment: None,
                });
            }
        }

        // Read stderr
        if let Ok(content) = tokio::fs::read_to_string(&stderr_path).await {
            for line in content.lines() {
                entries.push(LogEntry {
                    timestamp: now,
                    stream: LogStream::Stderr,
                    message: line.to_string(),
                    source: source.clone(),
                    service: None,
                    deployment: None,
                });
            }
        }

        Ok(entries)
    }

    /// Get the PID of a container's main process.
    ///
    /// Returns `Some(pid)` if the container has been started, `None` if
    /// it is still in `Pending` state (pid == 0).
    async fn get_container_pid(&self, id: &ContainerId) -> Result<Option<u32>> {
        let dir_name = Self::container_dir_name(id);

        let containers = self.containers.read().await;
        let container = containers
            .get(&dir_name)
            .ok_or_else(|| AgentError::NotFound {
                container: dir_name.clone(),
                reason: "Container not found".to_string(),
            })?;

        if container.pid > 0 {
            Ok(Some(container.pid))
        } else {
            Ok(None)
        }
    }

    /// Get the IP address of a container.
    ///
    /// On macOS, all sandboxed processes share the host network stack.
    /// Returns `127.0.0.1` (localhost) for all containers. Port-based
    /// differentiation is handled by the proxy manager.
    async fn get_container_ip(&self, id: &ContainerId) -> Result<Option<IpAddr>> {
        let dir_name = Self::container_dir_name(id);

        let containers = self.containers.read().await;
        if !containers.contains_key(&dir_name) {
            return Err(AgentError::NotFound {
                container: dir_name,
                reason: "Container not found".to_string(),
            });
        }

        // On macOS, all sandboxed processes share the host network.
        // Return 127.0.0.1 -- the proxy manager routes traffic by port.
        Ok(Some(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)))
    }

    /// Get the runtime-assigned port for a container.
    ///
    /// On macOS sandbox, each container is assigned a unique dynamic port
    /// (via OS port-0 binding) to avoid port conflicts between replicas
    /// sharing the host network. This port was passed to the process as
    /// `PORT` / `ZLAYER_PORT` environment variables.
    ///
    /// The proxy uses this port instead of the spec-declared endpoint port
    /// when constructing backend addresses, allowing multiple replicas to
    /// coexist on `127.0.0.1` with distinct ports.
    async fn get_container_port_override(&self, id: &ContainerId) -> Result<Option<u16>> {
        let dir_name = Self::container_dir_name(id);

        let containers = self.containers.read().await;
        let container = containers
            .get(&dir_name)
            .ok_or_else(|| AgentError::NotFound {
                container: dir_name,
                reason: "Container not found".to_string(),
            })?;

        Ok(Some(container.assigned_port))
    }

    /// Send a signal to the sandboxed process using `libc::kill`.
    #[allow(unsafe_code)]
    #[allow(clippy::cast_possible_wrap)]
    async fn kill_container(&self, id: &ContainerId, signal: Option<&str>) -> Result<()> {
        let canonical = crate::runtime::validate_signal(signal.unwrap_or("SIGKILL"))?;
        let dir_name = Self::container_dir_name(id);

        let pid = {
            let containers = self.containers.read().await;
            let container = containers
                .get(&dir_name)
                .ok_or_else(|| AgentError::NotFound {
                    container: dir_name.clone(),
                    reason: "Container not found".to_string(),
                })?;
            container.pid
        };

        if pid == 0 {
            return Err(AgentError::InvalidSpec(format!(
                "container '{dir_name}' is not running (no pid)"
            )));
        }

        let signum = match canonical.as_str() {
            "SIGKILL" => libc::SIGKILL,
            "SIGTERM" => libc::SIGTERM,
            "SIGINT" => libc::SIGINT,
            "SIGHUP" => libc::SIGHUP,
            "SIGUSR1" => libc::SIGUSR1,
            "SIGUSR2" => libc::SIGUSR2,
            other => {
                return Err(AgentError::InvalidSpec(format!(
                    "unsupported signal '{other}'"
                )));
            }
        };

        tracing::info!(container = %dir_name, pid = pid, signal = %canonical, "killing sandboxed process");

        let ret = unsafe { libc::kill(pid as i32, signum) };
        if ret != 0 {
            let err = std::io::Error::last_os_error();
            return Err(AgentError::Internal(format!(
                "kill({pid}, {canonical}) failed: {err}"
            )));
        }
        Ok(())
    }

    /// Tagging is not supported by the macOS sandbox runtime.
    ///
    /// Images are pulled directly into per-container rootfs directories rather
    /// than a shared content-addressed store, so there is nothing to tag.
    async fn tag_image(&self, _source: &str, _target: &str) -> Result<()> {
        Err(AgentError::Unsupported(
            "tag_image is not supported by the macOS sandbox runtime".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `SandboxRuntime` rooted at a fresh temp `data_dir`.
    ///
    /// Returns `(runtime, tempdir)`; the [`tempfile::TempDir`] must be kept
    /// alive for the lifetime of the test so the directory is not removed out
    /// from under the runtime. `new()` pre-creates `images/` and `containers/`
    /// under the data dir.
    fn runtime() -> (SandboxRuntime, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let config = MacSandboxConfig {
            data_dir: tmp.path().join("data"),
            log_dir: tmp.path().join("logs"),
            gpu_access: false,
        };
        let rt = SandboxRuntime::new(config, None).expect("construct SandboxRuntime");
        (rt, tmp)
    }

    /// Create `{images}/{dir}/rootfs/{file_name}` with `contents`.
    fn write_image_file(rt: &SandboxRuntime, dir: &str, file_name: &str, contents: &[u8]) {
        let rootfs = rt.images_dir().join(dir).join("rootfs");
        std::fs::create_dir_all(&rootfs).expect("create rootfs dir");
        std::fs::write(rootfs.join(file_name), contents).expect("write image file");
    }

    /// Write `{images}/{dir}/ref` containing `reference`.
    fn write_ref_file(rt: &SandboxRuntime, dir: &str, reference: &str) {
        let image_dir = rt.images_dir().join(dir);
        std::fs::create_dir_all(&image_dir).expect("create image dir");
        std::fs::write(image_dir.join("ref"), reference).expect("write ref file");
    }

    /// Write `{containers}/{name}/config.json` holding a minimal [`ServiceSpec`]
    /// whose image sanitizes to `image` (chosen so `sanitize == raw`).
    ///
    /// Serialized exactly like `create_container` does
    /// (`serde_json::to_string_pretty`), so the byte content matches what the
    /// daemon writes and what `referenced_image_dirs` reads back.
    fn write_container_config(rt: &SandboxRuntime, name: &str, image: &str) {
        let spec = ServiceSpec::minimal(name, image);
        // The image must round-trip to its raw form so the sanitized directory
        // name equals `image`. A bare lowercase name has no '/', ':', or '@'.
        assert_eq!(
            sanitize_image_name(&spec.image.name.to_string()),
            image,
            "test image must sanitize to its raw directory name"
        );
        let dir = rt.config.data_dir.join("containers").join(name);
        std::fs::create_dir_all(&dir).expect("create container dir");
        let json = serde_json::to_string_pretty(&spec).expect("serialize spec");
        std::fs::write(dir.join("config.json"), json).expect("write config.json");
    }

    #[tokio::test]
    async fn prune_removes_unreferenced_image_dirs() {
        let (rt, _tmp) = runtime();

        // imgA is referenced by an on-disk container; imgB is dangling.
        let imgb_contents = b"imgB-rootfs-bytes";
        write_image_file(&rt, "imga", "file", b"imgA-rootfs-bytes");
        write_image_file(&rt, "imgb", "file", imgb_contents);
        write_container_config(&rt, "svc-0", "imga");

        let result = rt.prune_images().await.expect("prune succeeds");

        assert_eq!(result.deleted, vec!["imgb".to_string()]);
        assert_eq!(result.space_reclaimed, imgb_contents.len() as u64);
        assert!(
            rt.images_dir().join("imga").is_dir(),
            "referenced image must survive prune"
        );
        assert!(
            !rt.images_dir().join("imgb").exists(),
            "unreferenced image must be removed"
        );
    }

    #[tokio::test]
    async fn prune_missing_images_dir_returns_default() {
        let (rt, _tmp) = runtime();

        // `new()` pre-creates `images/`; remove it to exercise the NotFound path.
        std::fs::remove_dir_all(rt.images_dir()).expect("remove images dir");
        assert!(!rt.images_dir().exists());

        let result = rt.prune_images().await.expect("prune succeeds");

        assert!(result.deleted.is_empty());
        assert_eq!(result.space_reclaimed, 0);
    }

    #[tokio::test]
    async fn list_images_prefers_ref_file() {
        let (rt, _tmp) = runtime();

        // One image with a `ref` file (trailing newline must be trimmed), one
        // without (falls back to the sanitized directory name).
        write_image_file(&rt, "withref", "layer", b"abc");
        write_ref_file(&rt, "withref", "alpine:latest\n");
        write_image_file(&rt, "noref", "layer", b"de");

        let mut images = rt.list_images().await.expect("list succeeds");
        images.sort_by(|a, b| a.reference.cmp(&b.reference));

        assert_eq!(images.len(), 2);

        let by_ref = |r: &str| {
            images
                .iter()
                .find(|i| i.reference == r)
                .unwrap_or_else(|| panic!("missing image with reference {r}"))
                .clone()
        };

        let withref = by_ref("alpine:latest");
        assert_eq!(withref.reference, "alpine:latest");
        assert_eq!(withref.size_bytes, Some(3));

        let noref = by_ref("noref");
        assert_eq!(noref.reference, "noref");
        assert_eq!(noref.size_bytes, Some(2));

        // No `blobs.redb` cache exists, so digests are best-effort `None`.
        assert!(withref.digest.is_none());
        assert!(noref.digest.is_none());
    }

    #[tokio::test]
    async fn remove_image_not_found() {
        let (rt, _tmp) = runtime();

        let err = rt
            .remove_image("does-not-exist", false)
            .await
            .expect_err("missing image must error");
        assert!(
            matches!(err, AgentError::NotFound { .. }),
            "expected NotFound, got {err:?}"
        );
    }

    #[tokio::test]
    async fn remove_image_in_use_requires_force() {
        let (rt, _tmp) = runtime();

        write_image_file(&rt, "imga", "file", b"imgA-rootfs-bytes");
        write_container_config(&rt, "svc-0", "imga");

        // Without force: in-use conflict -> InvalidSpec.
        let err = rt
            .remove_image("imga", false)
            .await
            .expect_err("in-use image without force must error");
        assert!(
            matches!(err, AgentError::InvalidSpec(_)),
            "expected InvalidSpec, got {err:?}"
        );
        assert!(
            rt.images_dir().join("imga").is_dir(),
            "image must remain after a refused removal"
        );

        // With force: removal succeeds and the directory is gone.
        rt.remove_image("imga", true)
            .await
            .expect("forced removal succeeds");
        assert!(
            !rt.images_dir().join("imga").exists(),
            "forced removal must delete the image dir"
        );
    }

    #[tokio::test]
    async fn dir_size_bytes_sums_recursively() {
        let (_rt, tmp) = runtime();

        let root = tmp.path().join("tree");
        let nested = root.join("a").join("b");
        std::fs::create_dir_all(&nested).expect("create nested dirs");
        std::fs::write(root.join("top.bin"), vec![0u8; 10]).expect("write top file");
        std::fs::write(root.join("a").join("mid.bin"), vec![0u8; 20]).expect("write mid file");
        std::fs::write(nested.join("leaf.bin"), vec![0u8; 30]).expect("write leaf file");

        let total = SandboxRuntime::dir_size_bytes(&root).await;
        assert_eq!(total, 60);
    }
}
