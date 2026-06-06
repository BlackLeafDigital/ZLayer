//! GPU inventory detection
//!
//! Platform-specific GPU detection:
//! - **Linux**: Scans `/sys/bus/pci/devices` for display controllers (VGA and 3D controllers).
//!   Identifies vendor (NVIDIA, AMD, Intel) by PCI vendor ID, reads VRAM from PCI BAR regions,
//!   and optionally uses `nvidia-smi` for NVIDIA-specific model and memory information.
//! - **macOS**: Uses `system_profiler SPDisplaysDataType -json` to detect Apple Silicon GPUs
//!   and unified memory via `sysctl -n hw.memsize`.
//! - **Windows**: Layers NVML (via `nvml-wrapper`, loads `nvml.dll` at runtime) on top of
//!   WMI `Win32_VideoController` enumeration (via the `wmi` crate). AMD VRAM is corrected by
//!   reading `HardwareInformation.qwMemorySize` from the display-class registry subtree
//!   (`HKLM\SYSTEM\CurrentControlSet\Control\Class\{4d36e968-...}\<xxxx>`) because WMI's
//!   `AdapterRAM` is a `u32` capped at 4 GiB. NVML data is preferred when both surface the
//!   same card.
//! - **Other**: Returns an empty GPU list.
//!
//! Linux/macOS require no external crates -- pure `sysfs/system_profiler` scanning with
//! optional subprocess calls. Windows pulls in `nvml-wrapper`, `wmi`, and `windows-registry`
//! (all gated to `target_os = "windows"` in the crate manifest).

use serde::{Deserialize, Serialize};

/// Detected GPU information
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GpuInfo {
    /// PCI bus ID (e.g., "0000:01:00.0" on Linux, "apple:0" on macOS)
    pub pci_bus_id: String,
    /// Vendor: "nvidia", "amd", "intel", "apple", or "unknown"
    pub vendor: String,
    /// Model name (e.g., "Apple M2 Pro" or "NVIDIA A100-SXM4-80GB")
    pub model: String,
    /// VRAM in MB (0 if unknown; on Apple Silicon, this is unified memory)
    pub memory_mb: u64,
    /// Device path (e.g., "/dev/nvidia0", "/dev/dri/card0", "<iokit://AppleGPU/0>")
    pub device_path: String,
    /// Render node path if applicable (e.g., "/dev/dri/renderD128"); None on macOS
    pub render_path: Option<String>,
}

// =============================================================================
// Linux GPU detection
// =============================================================================

/// Scan the system for GPU devices via sysfs PCI enumeration (Linux only)
///
/// Iterates over `/sys/bus/pci/devices` looking for PCI class codes that
/// indicate display controllers:
/// - `0x0300xx` -- VGA compatible controller
/// - `0x0302xx` -- 3D controller (e.g., NVIDIA Tesla/datacenter GPUs)
///
/// For each GPU found, determines vendor, model name, VRAM, and device paths.
#[cfg(target_os = "linux")]
#[must_use]
pub fn detect_gpus() -> Vec<GpuInfo> {
    use std::path::Path;

    let mut gpus = Vec::new();

    let pci_dir = Path::new("/sys/bus/pci/devices");
    if !pci_dir.exists() {
        return gpus;
    }

    let Ok(entries) = std::fs::read_dir(pci_dir) else {
        return gpus;
    };

    // Optionally pre-fetch nvidia-smi data once for all NVIDIA GPUs
    let nvidia_data = NvidiaSmiData::fetch();

    for entry in entries.flatten() {
        let device_dir = entry.path();

        // Read PCI device class
        let class_path = device_dir.join("class");
        let class = match std::fs::read_to_string(&class_path) {
            Ok(c) => c.trim().to_string(),
            Err(_) => continue,
        };

        // Filter to display controllers only
        if !class.starts_with("0x0302") && !class.starts_with("0x0300") {
            continue;
        }

        // Read PCI vendor ID
        let vendor_path = device_dir.join("vendor");
        let vendor_id = std::fs::read_to_string(&vendor_path)
            .unwrap_or_default()
            .trim()
            .to_string();

        let vendor = match vendor_id.as_str() {
            "0x10de" => "nvidia",
            "0x1002" => "amd",
            "0x8086" => "intel",
            _ => "unknown",
        }
        .to_string();

        let pci_bus_id = entry.file_name().to_string_lossy().to_string();

        // Count how many GPUs of this vendor we've already seen (for device path indexing)
        let vendor_index = gpus
            .iter()
            .filter(|g: &&GpuInfo| g.vendor == vendor)
            .count();

        let model = read_gpu_model(&device_dir, &vendor, &nvidia_data, vendor_index);
        let memory_mb = read_gpu_memory(&device_dir, &vendor, &nvidia_data, vendor_index);
        let (device_path, render_path) = find_device_paths(&pci_bus_id, &vendor, vendor_index);

        gpus.push(GpuInfo {
            pci_bus_id,
            vendor,
            model,
            memory_mb,
            device_path,
            render_path,
        });
    }

    gpus
}

// =============================================================================
// macOS GPU detection
// =============================================================================

/// Detect Apple Silicon GPUs via `system_profiler` (macOS only)
///
/// Runs `system_profiler SPDisplaysDataType -json` to enumerate GPUs, then
/// queries `sysctl -n hw.memsize` for the unified memory pool size. Apple Silicon
/// shares system memory between CPU and GPU, so the full physical memory is
/// reported as the GPU's available memory.
#[cfg(target_os = "macos")]
#[must_use]
pub fn detect_gpus() -> Vec<GpuInfo> {
    detect_apple_gpus()
}

/// Internal macOS GPU detection implementation
#[cfg(target_os = "macos")]
fn detect_apple_gpus() -> Vec<GpuInfo> {
    let output = match std::process::Command::new("system_profiler")
        .args(["SPDisplaysDataType", "-json"])
        .output()
    {
        Ok(out) if out.status.success() => out,
        _ => return Vec::new(),
    };

    let json_str = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = match serde_json::from_str(&json_str) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let unified_memory_mb = detect_unified_memory_mb();

    let mut gpus = Vec::new();

    // system_profiler returns { "SPDisplaysDataType": [ { ... }, ... ] }
    let Some(displays) = parsed.get("SPDisplaysDataType").and_then(|v| v.as_array()) else {
        return gpus;
    };

    for (idx, display) in displays.iter().enumerate() {
        let model = display
            .get("sppci_model")
            .and_then(|v| v.as_str())
            .or_else(|| display.get("_name").and_then(|v| v.as_str()))
            .unwrap_or("Apple GPU")
            .to_string();

        let chip_type = display
            .get("sppci_chiptype")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Use chip type in model name if the model doesn't already include it
        let model = if !chip_type.is_empty() && !model.contains(chip_type) {
            format!("{model} ({chip_type})")
        } else {
            model
        };

        // Apple Silicon uses unified memory -- report the full system memory
        // as GPU-accessible memory. For discrete AMD GPUs in older Macs,
        // try to read the VRAM field from system_profiler.
        let memory_mb = display
            .get("sppci_vram")
            .and_then(|v| v.as_str())
            .and_then(|s| {
                // Format is like "16 GB" or "8192 MB"
                let parts: Vec<&str> = s.split_whitespace().collect();
                if parts.len() >= 2 {
                    let amount: u64 = parts[0].parse().ok()?;
                    match parts[1].to_uppercase().as_str() {
                        "GB" => Some(amount * 1024),
                        "MB" => Some(amount),
                        _ => None,
                    }
                } else {
                    None
                }
            })
            .unwrap_or(unified_memory_mb);

        let vendor_str = display
            .get("sppci_vendor")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Determine vendor from vendor string or chip type
        let vendor = if vendor_str.to_lowercase().contains("apple")
            || chip_type.to_lowercase().starts_with("apple")
            || model.to_lowercase().contains("apple m")
        {
            "apple".to_string()
        } else if vendor_str.to_lowercase().contains("amd")
            || vendor_str.to_lowercase().contains("ati")
        {
            "amd".to_string()
        } else if vendor_str.to_lowercase().contains("intel") {
            "intel".to_string()
        } else {
            // Default to "apple" on macOS when vendor is ambiguous
            "apple".to_string()
        };

        gpus.push(GpuInfo {
            pci_bus_id: format!("apple:{idx}"),
            vendor,
            model,
            memory_mb,
            device_path: format!("iokit://AppleGPU/{idx}"),
            render_path: None,
        });
    }

    gpus
}

/// Query unified memory size via sysctl on macOS
#[cfg(target_os = "macos")]
fn detect_unified_memory_mb() -> u64 {
    let output = match std::process::Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output()
    {
        Ok(out) if out.status.success() => out,
        _ => return 0,
    };

    let text = String::from_utf8_lossy(&output.stdout);
    text.trim()
        .parse::<u64>()
        .map(|bytes| bytes / (1024 * 1024))
        .unwrap_or(0)
}

// =============================================================================
// Windows GPU detection
// =============================================================================

/// Detect GPUs on Windows via NVML + WMI + registry
///
/// Runs three layered probes:
/// 1. **NVML** (`nvml-wrapper`): loads `nvml.dll` via libloading; cleanly returns `Err`
///    when the NVIDIA driver is absent, which we silence and move on.
/// 2. **WMI** (`Win32_VideoController`): enumerates every GPU `PnP` device. Filters
///    `PNPDeviceID LIKE 'PCI\\VEN_%'` to exclude Remote Desktop / Hyper-V synthetic
///    adapters. Extracts PCI vendor + device IDs from `PNPDeviceID`.
/// 3. **Registry** (`HKLM\SYSTEM\...\Class\{4d36e968-...}\<xxxx>`): for AMD cards
///    we read `HardwareInformation.qwMemorySize` (u64) because WMI's `AdapterRAM`
///    is a 32-bit field capped at 4 GiB and lies for modern cards.
///
/// When NVML and WMI surface the same NVIDIA card (matched by PCI bus ID), NVML wins
/// for VRAM (accurate) and WMI wins for model-name enrichment when NVML returned an
/// empty/placeholder name.
#[cfg(target_os = "windows")]
#[must_use]
pub fn detect_gpus() -> Vec<GpuInfo> {
    windows_impl::detect_gpus_windows()
}

// =============================================================================
// Fallback for unsupported platforms
// =============================================================================

/// Returns an empty GPU list on unsupported platforms
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
#[must_use]
pub fn detect_gpus() -> Vec<GpuInfo> {
    Vec::new()
}

// =============================================================================
// Linux-only helpers: nvidia-smi, sysfs scanning
// =============================================================================

/// Pre-fetched nvidia-smi data to avoid calling the subprocess multiple times
#[cfg(target_os = "linux")]
struct NvidiaSmiData {
    /// GPU names, one per line
    names: Vec<String>,
    /// GPU memory in MB, one per line
    memories: Vec<u64>,
}

#[cfg(target_os = "linux")]
impl NvidiaSmiData {
    /// Attempt to fetch GPU info from nvidia-smi. Returns empty data on failure.
    fn fetch() -> Self {
        let names = Self::query("name");
        let memories = Self::query("memory.total")
            .iter()
            .map(|s| s.trim().parse::<u64>().unwrap_or(0))
            .collect();

        Self { names, memories }
    }

    fn query(field: &str) -> Vec<String> {
        let output = std::process::Command::new("nvidia-smi")
            .args([
                &format!("--query-gpu={field}"),
                "--format=csv,noheader,nounits",
            ])
            .output();

        match output {
            Ok(out) if out.status.success() => {
                let text = String::from_utf8_lossy(&out.stdout);
                text.lines().map(|l| l.trim().to_string()).collect()
            }
            _ => Vec::new(),
        }
    }
}

// =============================================================================
// Model detection (Linux only)
// =============================================================================

/// Read GPU model name from sysfs or nvidia-smi
#[cfg(target_os = "linux")]
fn read_gpu_model(
    device_dir: &std::path::Path,
    vendor: &str,
    nvidia_data: &NvidiaSmiData,
    vendor_index: usize,
) -> String {
    // Try DRM subsystem product name first (works for all vendors on recent kernels)
    if let Some(name) = read_drm_product_name(device_dir) {
        return name;
    }

    match vendor {
        "nvidia" => {
            // Use pre-fetched nvidia-smi data
            if let Some(name) = nvidia_data.names.get(vendor_index) {
                if !name.is_empty() {
                    return name.clone();
                }
            }
            "NVIDIA GPU".to_string()
        }
        "amd" => "AMD GPU".to_string(),
        "intel" => "Intel GPU".to_string(),
        _ => "Unknown GPU".to_string(),
    }
}

/// Try to read GPU product name from the DRM subsystem
///
/// Checks `/sys/bus/pci/devices/XXXX/drm/cardN/device/product_name` and similar paths.
#[cfg(target_os = "linux")]
fn read_drm_product_name(device_dir: &std::path::Path) -> Option<String> {
    // Try the product_name file under the PCI device
    let product_name_path = device_dir.join("label");
    if let Ok(name) = std::fs::read_to_string(&product_name_path) {
        let name = name.trim().to_string();
        if !name.is_empty() {
            return Some(name);
        }
    }

    // Try reading from the DRM card's device directory
    let drm_dir = device_dir.join("drm");
    if let Ok(entries) = std::fs::read_dir(&drm_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("card") {
                let product_path = entry.path().join("device").join("product_name");
                if let Ok(product) = std::fs::read_to_string(&product_path) {
                    let product = product.trim().to_string();
                    if !product.is_empty() {
                        return Some(product);
                    }
                }
            }
        }
    }

    None
}

// =============================================================================
// VRAM detection (Linux only)
// =============================================================================

/// Read GPU VRAM from sysfs PCI BAR regions or nvidia-smi
#[cfg(target_os = "linux")]
fn read_gpu_memory(
    device_dir: &std::path::Path,
    vendor: &str,
    nvidia_data: &NvidiaSmiData,
    vendor_index: usize,
) -> u64 {
    // For NVIDIA, prefer nvidia-smi data (more accurate than PCI BAR)
    if vendor == "nvidia" {
        if let Some(&mem) = nvidia_data.memories.get(vendor_index) {
            if mem > 0 {
                return mem;
            }
        }
    }

    // For AMD, try the VRAM-specific sysfs file
    if vendor == "amd" {
        let vram_path = device_dir.join("mem_info_vram_total");
        if let Ok(content) = std::fs::read_to_string(&vram_path) {
            if let Ok(bytes) = content.trim().parse::<u64>() {
                return bytes / (1024 * 1024);
            }
        }
    }

    // Fall back to reading PCI resource file for BAR sizes
    // The largest BAR region is typically VRAM
    let resource_path = device_dir.join("resource");
    if let Ok(content) = std::fs::read_to_string(&resource_path) {
        let mut max_size: u64 = 0;
        for line in content.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                if let (Ok(start), Ok(end)) = (
                    u64::from_str_radix(parts[0].trim_start_matches("0x"), 16),
                    u64::from_str_radix(parts[1].trim_start_matches("0x"), 16),
                ) {
                    if end > start {
                        let size = end - start + 1;
                        if size > max_size {
                            max_size = size;
                        }
                    }
                }
            }
        }
        if max_size > 0 {
            return max_size / (1024 * 1024);
        }
    }

    0
}

// =============================================================================
// Device path resolution (Linux only)
// =============================================================================

/// Find device paths for a GPU based on vendor and index
#[cfg(target_os = "linux")]
fn find_device_paths(
    _pci_bus_id: &str,
    vendor: &str,
    vendor_index: usize,
) -> (String, Option<String>) {
    if vendor == "nvidia" {
        let dev = format!("/dev/nvidia{vendor_index}");
        (dev, None)
    } else {
        // AMD, Intel, and unknown vendors use DRI device nodes
        let card = format!("/dev/dri/card{vendor_index}");
        let render = format!("/dev/dri/renderD{}", 128 + vendor_index);
        (card, Some(render))
    }
}

// =============================================================================
// Windows implementation module
// =============================================================================

#[cfg(target_os = "windows")]
mod windows_impl {
    use super::GpuInfo;
    use std::collections::HashMap;
    use wmi::{Variant, WMIConnection};

    /// Display-adapter class GUID. Every `DISPLAY` driver instance registers
    /// itself under `HKLM\SYSTEM\CurrentControlSet\Control\Class\{4d36e968-...}`
    /// with a 4-digit index (`0000`, `0001`, ...). AMD's driver writes
    /// `HardwareInformation.qwMemorySize` (`REG_QWORD`, bytes) there — this is the
    /// authoritative VRAM size, not WMI's 32-bit-capped `AdapterRAM`.
    const DISPLAY_CLASS_GUID: &str = "{4d36e968-e325-11ce-bfc1-08002be10318}";

    /// Entry point for the `windows` target's `detect_gpus()`.
    pub fn detect_gpus_windows() -> Vec<GpuInfo> {
        let mut gpus: Vec<GpuInfo> = Vec::new();

        // --- Pass 1: NVML (best-effort; absent driver is not fatal) ---------
        let nvml_gpus = detect_via_nvml();
        gpus.extend(nvml_gpus);

        // --- Pass 2: WMI Win32_VideoController ------------------------------
        let wmi_gpus = match detect_via_wmi() {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    "WMI Win32_VideoController query failed: {e}; \
                     Windows GPU detection falling back to NVML-only"
                );
                Vec::new()
            }
        };

        // --- Pass 3: Registry VRAM correction for AMD, and dedupe ----------
        let amd_registry = collect_amd_registry_vram().unwrap_or_default();

        for mut wmi_gpu in wmi_gpus {
            // AMD: replace WMI's AdapterRAM with qwMemorySize if we have it.
            if wmi_gpu.vendor == "amd" {
                if let Some(key) = pci_key_from_bus_id(&wmi_gpu.pci_bus_id) {
                    if let Some(&vram_bytes) = amd_registry.get(&key) {
                        wmi_gpu.memory_mb = vram_bytes / (1024 * 1024);
                    }
                }
            }

            // Dedupe against NVML entries (same PCI bus id).
            if let Some(existing) = gpus.iter_mut().find(|g| g.pci_bus_id == wmi_gpu.pci_bus_id) {
                // NVML wins for VRAM (accurate), but let WMI enrich an empty
                // / placeholder NVML model name.
                if existing.model.trim().is_empty() || existing.model == "NVIDIA GPU" {
                    wmi_gpu.model.clone_into(&mut existing.model);
                }
                continue;
            }

            gpus.push(wmi_gpu);
        }

        gpus
    }

    // ------------------------------------------------------------------------
    // NVML probe
    // ------------------------------------------------------------------------

    fn detect_via_nvml() -> Vec<GpuInfo> {
        // `Nvml::init()` dlopens `nvml.dll`. When the NVIDIA driver isn't
        // installed, this returns a clean `Err` — treat that as "no NVIDIA
        // GPUs" and move on without polluting logs.
        let nvml = match nvml_wrapper::Nvml::init() {
            Ok(n) => n,
            Err(e) => {
                tracing::debug!("NVML unavailable (no NVIDIA driver?): {e}");
                return Vec::new();
            }
        };

        let count = match nvml.device_count() {
            Ok(c) => c,
            Err(e) => {
                tracing::debug!("nvmlDeviceGetCount failed: {e}");
                return Vec::new();
            }
        };

        let mut out = Vec::with_capacity(count as usize);
        for i in 0..count {
            let device = match nvml.device_by_index(i) {
                Ok(d) => d,
                Err(e) => {
                    tracing::debug!("nvmlDeviceGetHandleByIndex({i}) failed: {e}");
                    continue;
                }
            };

            let model = device.name().unwrap_or_else(|_| "NVIDIA GPU".to_string());

            // `memory_info()` returns bytes.
            let memory_mb = device
                .memory_info()
                .map(|m| m.total / (1024 * 1024))
                .unwrap_or(0);

            let pci_bus_id = match device.pci_info() {
                Ok(info) => canonicalize_pci_bus_id(&info.bus_id),
                Err(e) => {
                    tracing::debug!("nvmlDeviceGetPciInfo({i}) failed: {e}");
                    // Without a PCI bus id we can't dedupe against WMI, so
                    // fall back to an index-based id.
                    format!("nvml:{i}")
                }
            };

            out.push(GpuInfo {
                pci_bus_id,
                vendor: "nvidia".to_string(),
                model,
                memory_mb,
                // Windows doesn't expose /dev nodes — report the NVML handle
                // index so downstream consumers can map back to the GPU.
                device_path: format!("nvml://{i}"),
                render_path: None,
            });
        }

        out
    }

    /// NVML returns a PCI bus ID like `00000000:01:00.0` (8-char domain).
    /// sysfs / Win32 tooling conventionally uses 4-char domain (`0000:01:00.0`),
    /// so trim the leading zeros in the domain segment to keep our bus IDs
    /// consistent across probes (NVML vs WMI registry parsing).
    fn canonicalize_pci_bus_id(raw: &str) -> String {
        let mut parts = raw.splitn(2, ':');
        let (Some(domain), Some(rest)) = (parts.next(), parts.next()) else {
            return raw.to_ascii_lowercase();
        };
        let trimmed_domain = domain.trim_start_matches('0').to_string();
        let domain_out = if trimmed_domain.is_empty() {
            "0000".to_string()
        } else if trimmed_domain.len() < 4 {
            format!("{trimmed_domain:0>4}")
        } else {
            trimmed_domain
        };
        format!("{domain_out}:{rest}").to_ascii_lowercase()
    }

    // ------------------------------------------------------------------------
    // WMI probe
    // ------------------------------------------------------------------------

    fn detect_via_wmi() -> Result<Vec<GpuInfo>, String> {
        // `wmi::WMIConnection::new` defaults to the `ROOT\CIMV2` namespace and
        // auto-initializes COM via `CoIncrementMTAUsage` when COM hasn't been
        // initialized on the current thread. No explicit `COMLibrary` needed
        // in wmi 0.18 — the reffcount is tracked internally.
        let wmi = WMIConnection::new().map_err(|e| format!("WMIConnection::new: {e}"))?;

        // `PNPDeviceID LIKE 'PCI\\VEN_%'` filters out Remote Desktop Mirror
        // drivers, Hyper-V synthetic adapters, and software renderers — only
        // real PCI display controllers match.
        let query = "SELECT Name, PNPDeviceID, AdapterRAM \
                     FROM Win32_VideoController \
                     WHERE PNPDeviceID LIKE 'PCI\\\\VEN_%'";

        let rows: Vec<HashMap<String, Variant>> = wmi
            .raw_query(query)
            .map_err(|e| format!("raw_query({query}): {e}"))?;

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let Some(pnp) = variant_string(row.get("PNPDeviceID")) else {
                continue;
            };
            let Some((vendor_id, device_id)) = parse_ven_dev(&pnp) else {
                continue;
            };

            let vendor = vendor_from_pci_id(vendor_id);
            let model = variant_string(row.get("Name"))
                .unwrap_or_else(|| format!("{} GPU", vendor.to_ascii_uppercase()));

            // AdapterRAM is a uint32 in bytes (CIM_UINT32), so WMI surfaces it
            // as Variant::UI4 / I4. Cap-at-4-GiB is the known lie we correct
            // below for AMD via the registry.
            let memory_mb = variant_u64(row.get("AdapterRAM")).map_or(0, |b| b / (1024 * 1024));

            let pci_bus_id = pci_bus_id_from_pnp(&pnp, vendor_id, device_id);

            out.push(GpuInfo {
                pci_bus_id,
                vendor: vendor.to_string(),
                model,
                memory_mb,
                device_path: format!(r"\\.\DISPLAY#{pnp}"),
                render_path: None,
            });
        }

        Ok(out)
    }

    /// Extract `(vendor_id, device_id)` from a `PNPDeviceID` like
    /// `PCI\VEN_10DE&DEV_2204&SUBSYS_...&REV_A1\4&31DE5EF7&0&0008`.
    fn parse_ven_dev(pnp: &str) -> Option<(u16, u16)> {
        // Matches case-insensitively. The PnP id format is stable across
        // Windows versions since XP.
        let upper = pnp.to_ascii_uppercase();
        let ven = extract_hex(&upper, "VEN_", 4)?;
        let dev = extract_hex(&upper, "DEV_", 4)?;
        Some((ven, dev))
    }

    fn extract_hex(s: &str, marker: &str, nibbles: usize) -> Option<u16> {
        let start = s.find(marker)? + marker.len();
        let hex = s.get(start..start + nibbles)?;
        u16::from_str_radix(hex, 16).ok()
    }

    fn vendor_from_pci_id(vendor_id: u16) -> &'static str {
        match vendor_id {
            0x10DE => "nvidia",
            // 0x1002 = AMD discrete (ATI); 0x1022 = AMD APU IGP.
            0x1002 | 0x1022 => "amd",
            0x8086 => "intel",
            _ => "unknown",
        }
    }

    /// Best-effort synthetic PCI bus ID derived from the `PnP` path. Windows
    /// doesn't hand us the bus:device.function triple directly in the `PnP`
    /// string, so we fall back to `0000:<ven>:<dev>.0` — consistent across
    /// NVML-vs-WMI dedupe runs because NVML's `bus_id` is different shape.
    ///
    /// NVML's `pci_info().bus_id` has the real bus:device.function, so for
    /// NVIDIA GPUs present in both probes we always prefer the NVML entry
    /// (dedup runs in `detect_gpus_windows` by NVML-side bus id). WMI-only
    /// entries (AMD, Intel) use this synthetic form, which remains stable per
    /// host for registry matching.
    fn pci_bus_id_from_pnp(pnp: &str, vendor_id: u16, device_id: u16) -> String {
        // Try to pull the instance-id tail (`&0008` above → `0008`) as a weak
        // "slot" signal so multiple same-vendor cards on one host get distinct
        // ids. Falls back to `0000`.
        let slot = pnp
            .rsplit_once('&')
            .and_then(|(_, tail)| tail.chars().take(4).collect::<String>().parse::<u16>().ok())
            .unwrap_or(0);
        format!("0000:{vendor_id:04x}:{device_id:04x}.{slot:x}")
    }

    fn variant_string(v: Option<&Variant>) -> Option<String> {
        match v? {
            Variant::String(s) => Some(s.clone()),
            _ => None,
        }
    }

    fn variant_u64(v: Option<&Variant>) -> Option<u64> {
        match v? {
            Variant::UI1(n) => Some(u64::from(*n)),
            Variant::UI2(n) => Some(u64::from(*n)),
            Variant::UI4(n) => Some(u64::from(*n)),
            Variant::UI8(n) => Some(*n),
            Variant::I1(n) if *n >= 0 => Some(u64::try_from(*n).unwrap_or(0)),
            Variant::I2(n) if *n >= 0 => Some(u64::try_from(*n).unwrap_or(0)),
            Variant::I4(n) if *n >= 0 => Some(u64::try_from(*n).unwrap_or(0)),
            Variant::I8(n) if *n >= 0 => Some(u64::try_from(*n).unwrap_or(0)),
            _ => None,
        }
    }

    // ------------------------------------------------------------------------
    // Registry probe (AMD VRAM correction)
    // ------------------------------------------------------------------------

    /// Walk `HKLM\SYSTEM\CurrentControlSet\Control\Class\{4d36e968-...}` for
    /// AMD display-adapter subkeys and collect their `qwMemorySize` values.
    /// Returns a map from `(vendor_id, device_id)` → VRAM-in-bytes.
    fn collect_amd_registry_vram() -> Result<HashMap<(u16, u16), u64>, String> {
        let class_path = format!(r"SYSTEM\CurrentControlSet\Control\Class\{DISPLAY_CLASS_GUID}");
        let class_key = windows_registry::LOCAL_MACHINE
            .open(&class_path)
            .map_err(|e| format!("open HKLM\\{class_path}: {e}"))?;

        let mut out: HashMap<(u16, u16), u64> = HashMap::new();

        // Subkeys are 4-digit instance indices: 0000, 0001, ...
        // Non-4-digit subkeys (Properties, Configuration) are harmless to
        // attempt — they just fail the open/value reads and we move on.
        let subkeys = match class_key.keys() {
            Ok(it) => it,
            Err(e) => return Err(format!("enumerate class subkeys: {e}")),
        };

        for name in subkeys {
            // Skip obvious non-adapter subkeys quickly; still allow anything
            // that parses as a 4-digit hex-ish index.
            if name.len() != 4 || !name.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }

            let Ok(adapter_key) = class_key.open(&name) else {
                continue;
            };

            let vendor_id = adapter_key
                .get_string("MatchingDeviceId")
                .ok()
                .as_deref()
                .and_then(parse_matching_device_id);
            let Some((ven, dev)) = vendor_id else {
                continue;
            };

            if vendor_from_pci_id(ven) != "amd" {
                continue;
            }

            // qwMemorySize is a REG_QWORD written by modern AMD drivers.
            // Older drivers may only publish `MemorySize` (REG_DWORD) — but
            // that hits the same 32-bit-cap problem as WMI, so we skip it
            // rather than report a wrong number.
            if let Ok(bytes) = adapter_key
                .open("HardwareInformation")
                .and_then(|hw| hw.get_u64("qwMemorySize"))
            {
                out.insert((ven, dev), bytes);
            }
        }

        Ok(out)
    }

    /// `MatchingDeviceId` is formatted like `PCI\VEN_1002&DEV_73A5&SUBSYS_...`.
    /// Reuse the `PnP` parser.
    fn parse_matching_device_id(s: &str) -> Option<(u16, u16)> {
        parse_ven_dev(s)
    }

    /// Build the AMD-registry lookup key from a WMI bus id (`0000:1002:73a5.0`).
    fn pci_key_from_bus_id(bus_id: &str) -> Option<(u16, u16)> {
        let mut parts = bus_id.split(':');
        let _domain = parts.next()?;
        let ven = u16::from_str_radix(parts.next()?, 16).ok()?;
        let dev_fn = parts.next()?;
        let dev = dev_fn
            .split('.')
            .next()
            .and_then(|h| u16::from_str_radix(h, 16).ok())?;
        Some((ven, dev))
    }

    // ------------------------------------------------------------------------
    // Unit tests for pure helpers (no Windows APIs touched)
    // ------------------------------------------------------------------------

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parse_ven_dev_nvidia() {
            let pnp = r"PCI\VEN_10DE&DEV_2204&SUBSYS_38811462&REV_A1\4&31DE5EF7&0&0008";
            assert_eq!(parse_ven_dev(pnp), Some((0x10DE, 0x2204)));
        }

        #[test]
        fn parse_ven_dev_amd() {
            let pnp = r"PCI\VEN_1002&DEV_73A5&SUBSYS_E4571DA2&REV_C0\4&1A2B3C4D&0&0010";
            assert_eq!(parse_ven_dev(pnp), Some((0x1002, 0x73A5)));
        }

        #[test]
        fn parse_ven_dev_intel_lowercase() {
            let pnp = r"pci\ven_8086&dev_9a49&subsys_00000000&rev_01\3&11583659&0&10";
            assert_eq!(parse_ven_dev(pnp), Some((0x8086, 0x9A49)));
        }

        #[test]
        fn parse_ven_dev_rejects_malformed() {
            assert_eq!(parse_ven_dev("USB\\VID_1234&PID_5678"), None);
            assert_eq!(parse_ven_dev(""), None);
        }

        #[test]
        fn vendor_id_mapping() {
            assert_eq!(vendor_from_pci_id(0x10DE), "nvidia");
            assert_eq!(vendor_from_pci_id(0x1002), "amd");
            assert_eq!(vendor_from_pci_id(0x1022), "amd");
            assert_eq!(vendor_from_pci_id(0x8086), "intel");
            assert_eq!(vendor_from_pci_id(0x1234), "unknown");
        }

        #[test]
        fn canonicalize_strips_nvml_domain_padding() {
            assert_eq!(canonicalize_pci_bus_id("00000000:01:00.0"), "0000:01:00.0");
            assert_eq!(canonicalize_pci_bus_id("0000:17:00.0"), "0000:17:00.0");
            assert_eq!(canonicalize_pci_bus_id("000a:17:00.0"), "000a:17:00.0");
        }

        #[test]
        fn canonicalize_handles_missing_colon() {
            // No colon -> lower-case passthrough (defensive).
            assert_eq!(canonicalize_pci_bus_id("WEIRD"), "weird");
        }

        #[test]
        fn pci_key_from_bus_id_roundtrip() {
            let bus = pci_bus_id_from_pnp(
                r"PCI\VEN_1002&DEV_73A5&SUBSYS_E4571DA2&REV_C0\4&1A2B3C4D&0&0010",
                0x1002,
                0x73A5,
            );
            assert_eq!(pci_key_from_bus_id(&bus), Some((0x1002, 0x73A5)));
        }

        #[test]
        fn variant_u64_accepts_unsigned_widths() {
            assert_eq!(variant_u64(Some(&Variant::UI4(4096))), Some(4096));
            assert_eq!(
                variant_u64(Some(&Variant::UI8(17_179_869_184))),
                Some(17_179_869_184)
            );
            assert_eq!(variant_u64(Some(&Variant::UI1(7))), Some(7));
        }

        #[test]
        fn variant_u64_rejects_negative_signed() {
            assert_eq!(variant_u64(Some(&Variant::I4(-1))), None);
        }

        #[test]
        fn variant_string_unwraps() {
            assert_eq!(
                variant_string(Some(&Variant::String("NVIDIA GeForce RTX 4090".into()))),
                Some("NVIDIA GeForce RTX 4090".to_string())
            );
            assert_eq!(variant_string(Some(&Variant::UI4(7))), None);
            assert_eq!(variant_string(None), None);
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gpu_info_serialization_roundtrip() {
        let info = GpuInfo {
            pci_bus_id: "0000:01:00.0".to_string(),
            vendor: "nvidia".to_string(),
            model: "NVIDIA A100-SXM4-80GB".to_string(),
            memory_mb: 81920,
            device_path: "/dev/nvidia0".to_string(),
            render_path: None,
        };

        let json = serde_json::to_string(&info).unwrap();
        let deserialized: GpuInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(info, deserialized);
    }

    #[test]
    fn test_gpu_info_amd_serialization() {
        let info = GpuInfo {
            pci_bus_id: "0000:03:00.0".to_string(),
            vendor: "amd".to_string(),
            model: "AMD GPU".to_string(),
            memory_mb: 16384,
            device_path: "/dev/dri/card0".to_string(),
            render_path: Some("/dev/dri/renderD128".to_string()),
        };

        let json = serde_json::to_string(&info).unwrap();
        let deserialized: GpuInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(info, deserialized);
    }

    #[test]
    fn test_gpu_info_apple_serialization() {
        let info = GpuInfo {
            pci_bus_id: "apple:0".to_string(),
            vendor: "apple".to_string(),
            model: "Apple M2 Pro".to_string(),
            memory_mb: 32768,
            device_path: "iokit://AppleGPU/0".to_string(),
            render_path: None,
        };

        let json = serde_json::to_string(&info).unwrap();
        let deserialized: GpuInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(info, deserialized);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_find_device_paths_nvidia() {
        let (dev, render) = find_device_paths("0000:01:00.0", "nvidia", 0);
        assert_eq!(dev, "/dev/nvidia0");
        assert!(render.is_none());

        let (dev, render) = find_device_paths("0000:02:00.0", "nvidia", 1);
        assert_eq!(dev, "/dev/nvidia1");
        assert!(render.is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_find_device_paths_amd() {
        let (dev, render) = find_device_paths("0000:03:00.0", "amd", 0);
        assert_eq!(dev, "/dev/dri/card0");
        assert_eq!(render, Some("/dev/dri/renderD128".to_string()));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_find_device_paths_intel() {
        let (dev, render) = find_device_paths("0000:00:02.0", "intel", 0);
        assert_eq!(dev, "/dev/dri/card0");
        assert_eq!(render, Some("/dev/dri/renderD128".to_string()));
    }

    #[test]
    fn test_detect_gpus_returns_vec() {
        // On CI/dev machines without GPUs this should return an empty vec
        // On machines with GPUs it should return valid entries
        let gpus = detect_gpus();
        for gpu in &gpus {
            assert!(!gpu.pci_bus_id.is_empty());
            assert!(!gpu.vendor.is_empty());
            assert!(!gpu.model.is_empty());
            assert!(!gpu.device_path.is_empty());
        }
    }
}
