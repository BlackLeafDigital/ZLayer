//! macOS VZ base-image **builder**.
//!
//! Produces the Tart-style base bundle that [`super::macos_vz`] consumes — a
//! directory containing `disk.img`, `hardware-model.bin`, and `aux.img` — by
//! driving a macOS `.ipsw` restore image through Apple's `VZMacOSInstaller`.
//! This is the producer end of the VZ runtime: `zlayer vz build-base` calls
//! [`build_base_image_bundle`], then packs + pushes the bundle as an OCI
//! artifact (`crates/zlayer-registry/src/pack.rs` +
//! `ImagePuller::push_artifact`).
//!
//! The actual install (a) requires the `com.apple.security.virtualization`
//! entitlement (sign with `scripts/sign-vz.sh` / `make build`), (b) downloads or
//! reads a multi-gigabyte `.ipsw`, and (c) runs for 20–40 minutes. The
//! end-to-end test is therefore `#[ignore]`d (it needs an `.ipsw` and a signed
//! binary); the full code path still compiles on macOS.
//!
//! All `Virtualization.framework` objects are touched on a single serial
//! dispatch queue (the VM is not thread-safe). The framework's async completions
//! (`loadFileURL:`, `fetchLatestSupported…`, `install…`) are bridged to std
//! channels exactly as [`super::macos_vz`] does for start/stop.

#![allow(unsafe_code)]

use crate::error::{AgentError, Result};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::time::Duration;

use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::AnyThread;
use objc2_foundation::NSError;
use objc2_virtualization::{
    VZMacAuxiliaryStorage, VZMacAuxiliaryStorageInitializationOptions, VZMacOSInstaller,
    VZMacOSRestoreImage, VZVirtualMachine,
};

use super::macos_vz::{
    clamp_cpu_count, clamp_memory_bytes, file_url, ns_error_message, QueuePinned, VmBuildInputs,
    VzContainer,
};

/// Where the `.ipsw` restore image comes from.
#[derive(Debug, Clone)]
pub enum IpswSource {
    /// A local `.ipsw` file already on disk.
    Local(PathBuf),
    /// An HTTP(S) URL to download before installing.
    Url(String),
    /// Fetch the latest restore image supported by this host (requires network +
    /// the virtualization entitlement).
    Latest,
}

/// Parameters for [`build_base_image_bundle`].
#[derive(Debug, Clone)]
pub struct BuildBaseParams {
    /// Restore-image source.
    pub source: IpswSource,
    /// Output bundle directory (`disk.img` / `hardware-model.bin` / `aux.img`
    /// are written here).
    pub output_dir: PathBuf,
    /// Size of the blank system disk, in GiB (created sparse).
    pub disk_size_gib: u64,
    /// vCPU override; clamped to `[minimum-required, host]`. `None` uses the
    /// restore image's minimum.
    pub cpu_count: Option<u32>,
    /// Memory override in MiB; clamped to `[minimum-required, framework-max]`.
    /// `None` uses the restore image's minimum.
    pub memory_mib: Option<u32>,
}

/// Result of a successful base-image build.
#[derive(Debug, Clone)]
pub struct BuiltBundle {
    /// The bundle directory (== `params.output_dir`).
    pub dir: PathBuf,
    /// macOS build version baked into the image (e.g. `24A335`).
    pub build_version: String,
    /// macOS marketing-ish version `major.minor.patch` from the restore image.
    pub os_version: String,
}

/// Default blank-disk size — generous enough for a fresh macOS system volume;
/// sparse, so it does not consume the full size until written.
pub const DEFAULT_DISK_SIZE_GIB: u64 = 50;

/// Convert a GiB count to bytes.
#[must_use]
pub fn disk_bytes(gib: u64) -> u64 {
    gib * 1024 * 1024 * 1024
}

/// A fixed, valid locally-administered MAC for the install VM (the per-container
/// clone later assigns its own random MAC).
const INSTALL_MAC: &str = "0a:00:00:00:00:01";

/// Build a macOS VZ base-image bundle.
///
/// Resolves the restore image (downloading if necessary), reads its hardware
/// requirements, writes `hardware-model.bin` + a fresh `aux.img`, creates a
/// blank `disk.img`, then runs `VZMacOSInstaller` to install macOS onto the
/// disk. On success the bundle directory is ready to be packed + pushed.
///
/// # Errors
/// Returns [`AgentError`] if virtualization is unavailable/unsigned, the restore
/// image cannot be loaded, the host supports no configuration in the image, or
/// the install fails.
pub async fn build_base_image_bundle(params: BuildBaseParams) -> Result<BuiltBundle> {
    if !unsafe { VZVirtualMachine::isSupported() } {
        return Err(AgentError::Internal(
            "Virtualization.framework is unavailable. On Apple Silicon, sign the binary with \
             `scripts/sign-vz.sh` (or `make build`, which auto-signs) to grant the \
             com.apple.security.virtualization entitlement."
                .to_string(),
        ));
    }

    std::fs::create_dir_all(&params.output_dir).map_err(|e| {
        AgentError::Internal(format!(
            "create output dir {}: {e}",
            params.output_dir.display()
        ))
    })?;

    // Resolve to a local .ipsw path (downloading for Url/Latest).
    let local_ipsw = resolve_local_ipsw(&params.source, &params.output_dir).await?;

    let disk_size = disk_bytes(params.disk_size_gib);
    let output_dir = params.output_dir.clone();
    let cpu_override = params.cpu_count;
    let mem_override = params.memory_mib;

    // All blocking Obj-C / VZ work happens off the async runtime.
    tokio::task::spawn_blocking(move || {
        run_install_blocking(
            &local_ipsw,
            &output_dir,
            disk_size,
            cpu_override,
            mem_override,
        )
    })
    .await
    .map_err(|e| AgentError::Internal(format!("install task panicked: {e}")))?
}

/// Resolve a [`IpswSource`] to a concrete local `.ipsw` path.
async fn resolve_local_ipsw(source: &IpswSource, output_dir: &Path) -> Result<PathBuf> {
    match source {
        IpswSource::Local(p) => {
            if !p.exists() {
                return Err(AgentError::Internal(format!(
                    "ipsw not found: {}",
                    p.display()
                )));
            }
            Ok(p.clone())
        }
        IpswSource::Url(url) => {
            let dest = output_dir.join("restore.ipsw");
            download_to(url, &dest).await?;
            Ok(dest)
        }
        IpswSource::Latest => {
            let url = fetch_latest_supported_url().await?;
            tracing::info!(%url, "fetched latest supported restore-image URL");
            let dest = output_dir.join("restore.ipsw");
            download_to(&url, &dest).await?;
            Ok(dest)
        }
    }
}

/// Stream an HTTP(S) body to `dest` without buffering the whole (multi-GB) body.
async fn download_to(url: &str, dest: &Path) -> Result<()> {
    use tokio::io::AsyncWriteExt;

    tracing::info!(%url, dest = %dest.display(), "downloading restore image");
    let resp = reqwest::get(url)
        .await
        .map_err(|e| AgentError::Internal(format!("GET {url}: {e}")))?
        .error_for_status()
        .map_err(|e| AgentError::Internal(format!("GET {url}: {e}")))?;

    let mut file = tokio::fs::File::create(dest)
        .await
        .map_err(|e| AgentError::Internal(format!("create {}: {e}", dest.display())))?;
    let mut resp = resp;
    let mut total: u64 = 0;
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| AgentError::Internal(format!("download chunk: {e}")))?
    {
        total += chunk.len() as u64;
        file.write_all(&chunk)
            .await
            .map_err(|e| AgentError::Internal(format!("write {}: {e}", dest.display())))?;
    }
    file.flush()
        .await
        .map_err(|e| AgentError::Internal(format!("flush {}: {e}", dest.display())))?;
    tracing::info!(bytes = total, dest = %dest.display(), "restore image downloaded");
    Ok(())
}

/// Fetch the network URL of the latest host-supported restore image.
async fn fetch_latest_supported_url() -> Result<String> {
    tokio::task::spawn_blocking(|| {
        let (tx, rx) = mpsc::channel::<std::result::Result<String, String>>();
        let completion = RcBlock::new(move |img: *mut VZMacOSRestoreImage, err: *mut NSError| {
            let r = if !err.is_null() {
                Err(ns_error_message(err))
            } else if img.is_null() {
                Err("fetchLatestSupported returned null".to_string())
            } else {
                // SAFETY: non-null framework-provided restore image; we retain
                // it for the duration of this handler to read its URL.
                match unsafe { Retained::retain(img) } {
                    Some(image) => Ok(unsafe { image.URL() }
                        .absoluteString()
                        .map_or_else(|| "<unknown>".to_string(), |s| s.to_string())),
                    None => Err("retain restore image failed".to_string()),
                }
            };
            let _ = tx.send(r);
        });
        // SAFETY: class method; the completion (RcBlock) is retained by the
        // framework until it fires on an arbitrary libdispatch thread.
        unsafe { VZMacOSRestoreImage::fetchLatestSupportedWithCompletionHandler(&completion) };
        rx.recv()
            .unwrap_or_else(|_| Err("fetch-latest channel closed".to_string()))
    })
    .await
    .map_err(|e| AgentError::Internal(format!("fetch-latest task panicked: {e}")))?
    .map_err(AgentError::Internal)
}

/// The blocking install pipeline (load image -> requirements -> aux/disk ->
/// config -> VM -> install). Runs entirely on a `spawn_blocking` thread.
#[allow(clippy::too_many_lines)]
fn run_install_blocking(
    local_ipsw: &Path,
    output_dir: &Path,
    disk_size: u64,
    cpu_override: Option<u32>,
    mem_override: Option<u32>,
) -> Result<BuiltBundle> {
    // --- load the restore image from the local file ---
    let loaded = load_restore_image(local_ipsw)?;
    let image = loaded.0;

    // --- read the most-featureful supported configuration ---
    let requirements =
        unsafe { image.mostFeaturefulSupportedConfiguration() }.ok_or_else(|| {
            AgentError::Internal(
                "this host supports no configuration in the restore image (wrong arch / too old)"
                    .to_string(),
            )
        })?;
    let hardware_model = unsafe { requirements.hardwareModel() };
    if !unsafe { hardware_model.isSupported() } {
        return Err(AgentError::Internal(
            "restore image hardware model is not supported on this host".to_string(),
        ));
    }
    let min_cpu =
        u32::try_from(unsafe { requirements.minimumSupportedCPUCount() }).unwrap_or(u32::MAX);
    let min_mem_mib =
        u32::try_from(unsafe { requirements.minimumSupportedMemorySize() } / (1024 * 1024))
            .unwrap_or(u32::MAX);
    let build_version = unsafe { image.buildVersion() }.to_string();
    let os = unsafe { image.operatingSystemVersion() };
    let os_version = format!(
        "{}.{}.{}",
        os.majorVersion, os.minorVersion, os.patchVersion
    );
    tracing::info!(
        %build_version,
        %os_version,
        min_cpu,
        min_mem_mib,
        "loaded macOS restore image"
    );

    // --- write hardware-model.bin ---
    let hw_model_path = output_dir.join("hardware-model.bin");
    let hw_bytes = unsafe { hardware_model.dataRepresentation() }.to_vec();
    std::fs::write(&hw_model_path, &hw_bytes)
        .map_err(|e| AgentError::Internal(format!("write hardware-model.bin: {e}")))?;

    // --- create a fresh auxiliary storage (aux.img) for this hardware model ---
    let aux_path = output_dir.join("aux.img");
    let aux_url = file_url(&aux_path);
    // SAFETY: synchronous initializer; writes the aux storage file for the model.
    unsafe {
        VZMacAuxiliaryStorage::initCreatingStorageAtURL_hardwareModel_options_error(
            VZMacAuxiliaryStorage::alloc(),
            &aux_url,
            &hardware_model,
            VZMacAuxiliaryStorageInitializationOptions::AllowOverwrite,
        )
    }
    .map_err(|e| AgentError::Internal(format!("create aux.img: {}", e.localizedDescription())))?;

    // --- create the blank system disk (sparse) ---
    let disk_path = output_dir.join("disk.img");
    create_blank_disk(&disk_path, disk_size)?;

    // --- build the VM configuration on a serial queue, reusing the runtime's
    //     proven config builder (it reads hardware-model.bin + aux.img back) ---
    let console_log = output_dir.join("install-console.log");
    let machine_id_path = output_dir.join("machine-id.bin");
    let inputs = VmBuildInputs {
        bundle_dir: output_dir.to_path_buf(),
        disk_path: disk_path.clone(),
        aux_path: aux_path.clone(),
        machine_id_path,
        console_log,
        mac: INSTALL_MAC.to_string(),
        cpu_count: clamp_cpu_count(cpu_override.unwrap_or(0).max(min_cpu)),
        memory_bytes: clamp_memory_bytes(mem_override.unwrap_or(0).max(min_mem_mib)),
    };

    let queue = DispatchQueue::new("com.zlayer.vz.build", None);

    // Build config + create the VM on the queue.
    let (tx_vm, rx_vm) =
        mpsc::channel::<std::result::Result<QueuePinned<Retained<VZVirtualMachine>>, String>>();
    let inputs_q = inputs.clone();
    let queue_for_vm = queue.clone();
    queue.exec_sync(move || {
        let built = VzContainer::build_configuration(&inputs_q).map(|config| {
            // SAFETY: created on this serial queue, the only place it is used.
            let vm = unsafe {
                VZVirtualMachine::initWithConfiguration_queue(
                    VZVirtualMachine::alloc(),
                    &config,
                    &queue_for_vm,
                )
            };
            QueuePinned(vm)
        });
        let _ = tx_vm.send(built);
    });
    let vm = Arc::new(
        rx_vm
            .recv()
            .unwrap_or_else(|_| Err("vm build channel closed".to_string()))
            .map_err(AgentError::Internal)?,
    );

    // Create the installer on the queue.
    let (tx_inst, rx_inst) = mpsc::channel::<QueuePinned<Retained<VZMacOSInstaller>>>();
    let vm_for_inst = Arc::clone(&vm);
    let ipsw_q = local_ipsw.to_path_buf();
    queue.exec_sync(move || {
        let url = file_url(&ipsw_q);
        // SAFETY: must be on the VM's queue (it is); returns the installer.
        let inst = unsafe {
            VZMacOSInstaller::initWithVirtualMachine_restoreImageURL(
                VZMacOSInstaller::alloc(),
                &vm_for_inst.0,
                &url,
            )
        };
        let _ = tx_inst.send(QueuePinned(inst));
    });
    let installer = Arc::new(
        rx_inst
            .recv()
            .map_err(|_| AgentError::Internal("installer channel closed".to_string()))?,
    );

    // Start the install on the queue; bridge completion + poll progress.
    let (tx_run, rx_run) = mpsc::channel::<std::result::Result<(), String>>();
    let inst_run = Arc::clone(&installer);
    queue.exec_async(move || {
        let completion = RcBlock::new(move |err: *mut NSError| {
            let r = if err.is_null() {
                Ok(())
            } else {
                Err(ns_error_message(err))
            };
            let _ = tx_run.send(r);
        });
        // SAFETY: on the VM's queue; the RcBlock is retained by the framework
        // until installation finishes (or fails).
        unsafe { inst_run.0.installWithCompletionHandler(&completion) };
    });

    tracing::info!("macOS installation started; this typically takes 20-40 minutes");
    loop {
        match rx_run.recv_timeout(Duration::from_secs(15)) {
            Ok(result) => {
                result.map_err(AgentError::Internal)?;
                break;
            }
            Err(RecvTimeoutError::Timeout) => {
                // SAFETY: NSProgress.fractionCompleted is safe to read off the VM
                // queue (Apple's own sample observes it from another thread).
                let frac = unsafe { installer.0.progress().fractionCompleted() };
                tracing::info!(percent = format!("{:.1}", frac * 100.0), "installing macOS");
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(AgentError::Internal(
                    "install completion channel closed".to_string(),
                ));
            }
        }
    }

    // Validate the bundle is complete.
    for f in ["disk.img", "hardware-model.bin", "aux.img"] {
        if !output_dir.join(f).exists() {
            return Err(AgentError::Internal(format!(
                "install finished but {f} is missing from the bundle"
            )));
        }
    }
    tracing::info!(dir = %output_dir.display(), "macOS VZ base bundle built");

    Ok(BuiltBundle {
        dir: output_dir.to_path_buf(),
        build_version,
        os_version,
    })
}

/// Load a restore image from a local `.ipsw`, blocking until the framework's
/// completion fires.
fn load_restore_image(local_ipsw: &Path) -> Result<QueuePinned<Retained<VZMacOSRestoreImage>>> {
    let (tx, rx) =
        mpsc::channel::<std::result::Result<QueuePinned<Retained<VZMacOSRestoreImage>>, String>>();
    let url = file_url(local_ipsw);
    let completion = RcBlock::new(move |img: *mut VZMacOSRestoreImage, err: *mut NSError| {
        let r = if !err.is_null() {
            Err(ns_error_message(err))
        } else if img.is_null() {
            Err("restore image load returned null".to_string())
        } else {
            // SAFETY: non-null framework restore image; retain to own it past
            // this handler.
            match unsafe { Retained::retain(img) } {
                Some(image) => Ok(QueuePinned(image)),
                None => Err("retain restore image failed".to_string()),
            }
        };
        let _ = tx.send(r);
    });
    // SAFETY: class method; completion retained by the framework until it fires.
    unsafe { VZMacOSRestoreImage::loadFileURL_completionHandler(&url, &completion) };
    rx.recv()
        .unwrap_or_else(|_| Err("restore-image load channel closed".to_string()))
        .map_err(AgentError::Internal)
}

/// Create a sparse blank disk image of `size` bytes.
fn create_blank_disk(path: &Path, size: u64) -> Result<()> {
    let file = std::fs::File::create(path)
        .map_err(|e| AgentError::Internal(format!("create {}: {e}", path.display())))?;
    file.set_len(size)
        .map_err(|e| AgentError::Internal(format!("size {}: {e}", path.display())))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_bytes_converts_gib() {
        assert_eq!(disk_bytes(1), 1_073_741_824);
        assert_eq!(disk_bytes(50), 50 * 1024 * 1024 * 1024);
    }

    #[test]
    fn create_blank_disk_is_sparse_and_sized() {
        let dir = tempfile::tempdir().unwrap();
        let disk = dir.path().join("disk.img");
        create_blank_disk(&disk, disk_bytes(2)).unwrap();
        let meta = std::fs::metadata(&disk).unwrap();
        assert_eq!(meta.len(), disk_bytes(2));
    }

    #[test]
    fn local_source_missing_is_error() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let err = rt
            .block_on(resolve_local_ipsw(
                &IpswSource::Local(dir.path().join("nope.ipsw")),
                dir.path(),
            ))
            .unwrap_err();
        assert!(err.to_string().contains("nope.ipsw"));
    }

    /// Full end-to-end build. Requires a signed binary (virtualization
    /// entitlement) and `ZLAYER_TEST_IPSW` pointing at a macOS `.ipsw`. Ignored
    /// by default — it downloads/installs a multi-GB macOS guest (~30 min).
    #[tokio::test]
    #[ignore = "requires entitlement + a macOS .ipsw via ZLAYER_TEST_IPSW; ~30 min"]
    async fn build_base_from_ipsw() {
        let ipsw = std::env::var("ZLAYER_TEST_IPSW").expect("set ZLAYER_TEST_IPSW");
        let out = tempfile::tempdir().unwrap();
        let bundle = build_base_image_bundle(BuildBaseParams {
            source: IpswSource::Local(PathBuf::from(ipsw)),
            output_dir: out.path().to_path_buf(),
            disk_size_gib: DEFAULT_DISK_SIZE_GIB,
            cpu_count: None,
            memory_mib: None,
        })
        .await
        .expect("build base image");
        assert!(bundle.dir.join("disk.img").exists());
        assert!(bundle.dir.join("hardware-model.bin").exists());
        assert!(bundle.dir.join("aux.img").exists());
    }
}
