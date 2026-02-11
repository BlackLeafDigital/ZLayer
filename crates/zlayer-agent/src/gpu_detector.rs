//! GPU inventory detection
//!
//! Platform-specific GPU detection:
//! - **Linux**: Scans `/sys/bus/pci/devices` for display controllers (VGA and 3D controllers).
//!   Identifies vendor (NVIDIA, AMD, Intel) by PCI vendor ID, reads VRAM from PCI BAR regions,
//!   and optionally uses `nvidia-smi` for NVIDIA-specific model and memory information.
//! - **macOS**: Uses `system_profiler SPDisplaysDataType -json` to detect Apple Silicon GPUs
//!   and unified memory via `sysctl -n hw.memsize`.
//! - **Other**: Returns an empty GPU list.
//!
//! No external dependencies required -- pure sysfs/system_profiler scanning with optional
//! subprocess calls for enrichment.

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
    /// Device path (e.g., "/dev/nvidia0", "/dev/dri/card0", "iokit://AppleGPU/0")
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
pub fn detect_gpus() -> Vec<GpuInfo> {
    use std::path::Path;

    let mut gpus = Vec::new();

    let pci_dir = Path::new("/sys/bus/pci/devices");
    if !pci_dir.exists() {
        return gpus;
    }

    let entries = match std::fs::read_dir(pci_dir) {
        Ok(entries) => entries,
        Err(_) => return gpus,
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

/// Detect Apple Silicon GPUs via system_profiler (macOS only)
///
/// Runs `system_profiler SPDisplaysDataType -json` to enumerate GPUs, then
/// queries `sysctl -n hw.memsize` for the unified memory pool size. Apple Silicon
/// shares system memory between CPU and GPU, so the full physical memory is
/// reported as the GPU's available memory.
#[cfg(target_os = "macos")]
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
    let displays = match parsed.get("SPDisplaysDataType").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return gpus,
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
            format!("{} ({})", model, chip_type)
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
            pci_bus_id: format!("apple:{}", idx),
            vendor,
            model,
            memory_mb,
            device_path: format!("iokit://AppleGPU/{}", idx),
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
// Fallback for unsupported platforms
// =============================================================================

/// Returns an empty GPU list on unsupported platforms
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
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
                &format!("--query-gpu={}", field),
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
    match vendor {
        "nvidia" => {
            let dev = format!("/dev/nvidia{}", vendor_index);
            (dev, None)
        }
        _ => {
            // AMD, Intel, and unknown vendors use DRI device nodes
            let card = format!("/dev/dri/card{}", vendor_index);
            let render = format!("/dev/dri/renderD{}", 128 + vendor_index);
            (card, Some(render))
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
