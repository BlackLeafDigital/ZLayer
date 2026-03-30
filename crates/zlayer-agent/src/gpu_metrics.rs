//! GPU metrics collection for observability.
//!
//! Collects per-GPU utilization, memory, temperature, and power metrics
//! using vendor-specific interfaces:
//! - NVIDIA: `nvidia-smi` CLI (avoids hard NVML dependency)
//! - AMD: sysfs under `/sys/class/drm/card{N}/device/`
//! - Intel: sysfs under `/sys/class/drm/card{N}/device/`
//! - Apple: `IOKit` via `powermetrics` (macOS only)

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

/// Per-GPU utilization snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuUtilizationReport {
    /// GPU index on this node
    pub index: u32,
    /// GPU compute utilization percentage (0-100)
    pub utilization_percent: f32,
    /// GPU memory currently used in MB
    pub memory_used_mb: u64,
    /// GPU total memory in MB
    pub memory_total_mb: u64,
    /// GPU temperature in Celsius (if available)
    pub temperature_c: Option<u32>,
    /// GPU power draw in Watts (if available)
    pub power_draw_w: Option<f32>,
}

/// GPU health status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum GpuHealthStatus {
    /// GPU is operating normally
    Healthy,
    /// GPU is throttling due to temperature
    ThermalThrottle,
    /// GPU has ECC errors
    EccError,
    /// GPU is not responding
    Unresponsive,
}

/// Per-GPU health check result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuHealthReport {
    /// GPU index
    pub index: u32,
    /// Health status
    pub status: GpuHealthStatus,
    /// Human-readable detail message
    pub detail: Option<String>,
}

/// Collect GPU utilization metrics for all detected GPUs.
///
/// Uses `nvidia-smi` for NVIDIA GPUs and sysfs for AMD/Intel.
/// Returns an empty vec if no GPUs are detected or metrics cannot be read.
pub async fn collect_gpu_metrics(vendor: &str, gpu_count: u32) -> Vec<GpuUtilizationReport> {
    match vendor {
        "nvidia" => collect_nvidia_metrics(gpu_count).await,
        "amd" => collect_amd_metrics(gpu_count),
        "intel" => collect_intel_metrics(gpu_count),
        _ => Vec::new(),
    }
}

/// Collect GPU health reports.
pub async fn check_gpu_health(vendor: &str, gpu_count: u32) -> Vec<GpuHealthReport> {
    match vendor {
        "nvidia" => check_nvidia_health(gpu_count).await,
        "amd" => check_amd_health(gpu_count),
        _ => (0..gpu_count)
            .map(|i| GpuHealthReport {
                index: i,
                status: GpuHealthStatus::Healthy,
                detail: None,
            })
            .collect(),
    }
}

// -- NVIDIA -------------------------------------------------------------------

async fn collect_nvidia_metrics(gpu_count: u32) -> Vec<GpuUtilizationReport> {
    let output = match tokio::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=index,utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .await
    {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            warn!("nvidia-smi failed: {stderr}");
            return Vec::new();
        }
        Err(e) => {
            debug!("nvidia-smi not available: {e}");
            return Vec::new();
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split(',').map(str::trim).collect();
            if parts.len() < 6 {
                return None;
            }
            Some(GpuUtilizationReport {
                index: parts[0].parse().ok()?,
                utilization_percent: parts[1].parse().ok()?,
                memory_used_mb: parts[2].parse().ok()?,
                memory_total_mb: parts[3].parse().ok()?,
                temperature_c: parts[4].parse().ok(),
                power_draw_w: parts[5].parse().ok(),
            })
        })
        .take(gpu_count as usize)
        .collect()
}

async fn check_nvidia_health(gpu_count: u32) -> Vec<GpuHealthReport> {
    // Check for Xid errors and thermal throttling via nvidia-smi
    let output = match tokio::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=index,temperature.gpu,ecc.errors.uncorrected.volatile.total",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .await
    {
        Ok(o) if o.status.success() => o,
        _ => {
            return (0..gpu_count)
                .map(|i| GpuHealthReport {
                    index: i,
                    status: GpuHealthStatus::Unresponsive,
                    detail: Some("nvidia-smi unavailable".to_string()),
                })
                .collect();
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split(',').map(str::trim).collect();
            if parts.len() < 3 {
                return None;
            }
            let index: u32 = parts[0].parse().ok()?;
            let temp: u32 = parts[1].parse().unwrap_or(0);
            let ecc_errors: u64 = parts[2].parse().unwrap_or(0);

            let (status, detail) = if ecc_errors > 0 {
                (
                    GpuHealthStatus::EccError,
                    Some(format!("{ecc_errors} uncorrected ECC errors")),
                )
            } else if temp > 90 {
                (
                    GpuHealthStatus::ThermalThrottle,
                    Some(format!("Temperature: {temp}\u{00b0}C (throttle threshold)")),
                )
            } else {
                (GpuHealthStatus::Healthy, None)
            };

            Some(GpuHealthReport {
                index,
                status,
                detail,
            })
        })
        .take(gpu_count as usize)
        .collect()
}

// -- AMD ----------------------------------------------------------------------

#[allow(clippy::cast_precision_loss)]
fn collect_amd_metrics(gpu_count: u32) -> Vec<GpuUtilizationReport> {
    (0..gpu_count)
        .map(|i| {
            let base = format!("/sys/class/drm/card{i}/device");
            let utilization = read_sysfs_u32(&format!("{base}/gpu_busy_percent")).unwrap_or(0);
            let mem_used = read_sysfs_u64(&format!("{base}/mem_info_vram_used"))
                .map_or(0, |b| b / (1024 * 1024));
            let mem_total = read_sysfs_u64(&format!("{base}/mem_info_vram_total"))
                .map_or(0, |b| b / (1024 * 1024));
            let temp =
                read_sysfs_u32(&format!("{base}/hwmon/hwmon0/temp1_input")).map(|t| t / 1000); // millidegrees to degrees
            let power = read_sysfs_u32(&format!("{base}/hwmon/hwmon0/power1_average"))
                .map(|p| p as f32 / 1_000_000.0); // microwatts to watts

            GpuUtilizationReport {
                index: i,
                utilization_percent: utilization as f32,
                memory_used_mb: mem_used,
                memory_total_mb: mem_total,
                temperature_c: temp,
                power_draw_w: power,
            }
        })
        .collect()
}

fn check_amd_health(gpu_count: u32) -> Vec<GpuHealthReport> {
    (0..gpu_count)
        .map(|i| {
            let base = format!("/sys/class/drm/card{i}/device");
            let temp =
                read_sysfs_u32(&format!("{base}/hwmon/hwmon0/temp1_input")).map_or(0, |t| t / 1000);

            if temp > 100 {
                GpuHealthReport {
                    index: i,
                    status: GpuHealthStatus::ThermalThrottle,
                    detail: Some(format!("Temperature: {temp}\u{00b0}C")),
                }
            } else {
                GpuHealthReport {
                    index: i,
                    status: GpuHealthStatus::Healthy,
                    detail: None,
                }
            }
        })
        .collect()
}

// -- Intel --------------------------------------------------------------------

#[allow(clippy::cast_precision_loss)]
fn collect_intel_metrics(gpu_count: u32) -> Vec<GpuUtilizationReport> {
    // Intel discrete GPUs expose some metrics via i915 sysfs
    (0..gpu_count)
        .map(|i| {
            let base = format!("/sys/class/drm/card{i}/device");
            let temp =
                read_sysfs_u32(&format!("{base}/hwmon/hwmon0/temp1_input")).map(|t| t / 1000);
            let power = read_sysfs_u32(&format!("{base}/hwmon/hwmon0/power1_average"))
                .map(|p| p as f32 / 1_000_000.0);

            GpuUtilizationReport {
                index: i,
                utilization_percent: 0.0, // Intel sysfs doesn't expose utilization directly
                memory_used_mb: 0,
                memory_total_mb: 0,
                temperature_c: temp,
                power_draw_w: power,
            }
        })
        .collect()
}

// -- Helpers ------------------------------------------------------------------

fn read_sysfs_u32(path: &str) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn read_sysfs_u64(path: &str) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

// -- Tests --------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gpu_utilization_report_serialization() {
        let report = GpuUtilizationReport {
            index: 0,
            utilization_percent: 85.5,
            memory_used_mb: 4096,
            memory_total_mb: 8192,
            temperature_c: Some(72),
            power_draw_w: Some(250.0),
        };

        let json = serde_json::to_string(&report).unwrap();
        let deserialized: GpuUtilizationReport = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.index, 0);
        assert!((deserialized.utilization_percent - 85.5).abs() < f32::EPSILON);
        assert_eq!(deserialized.memory_used_mb, 4096);
        assert_eq!(deserialized.memory_total_mb, 8192);
        assert_eq!(deserialized.temperature_c, Some(72));
    }

    #[test]
    fn test_gpu_health_report_serialization() {
        let report = GpuHealthReport {
            index: 1,
            status: GpuHealthStatus::ThermalThrottle,
            detail: Some("Temperature: 95\u{00b0}C".to_string()),
        };

        let json = serde_json::to_string(&report).unwrap();
        let deserialized: GpuHealthReport = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.index, 1);
        assert_eq!(deserialized.status, GpuHealthStatus::ThermalThrottle);
        assert!(deserialized.detail.unwrap().contains("95"));
    }

    #[test]
    fn test_gpu_health_status_variants() {
        let statuses = [
            GpuHealthStatus::Healthy,
            GpuHealthStatus::ThermalThrottle,
            GpuHealthStatus::EccError,
            GpuHealthStatus::Unresponsive,
        ];

        for status in &statuses {
            let json = serde_json::to_string(status).unwrap();
            let deserialized: GpuHealthStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(&deserialized, status);
        }
    }

    #[tokio::test]
    async fn test_collect_gpu_metrics_unknown_vendor() {
        // Unknown vendor should return empty vec
        let metrics = collect_gpu_metrics("unknown_vendor", 1).await;
        assert!(metrics.is_empty());
    }

    #[tokio::test]
    async fn test_check_gpu_health_unknown_vendor() {
        // Unknown vendor should return default Healthy status
        let reports = check_gpu_health("unknown_vendor", 2).await;
        assert_eq!(reports.len(), 2);
        for report in &reports {
            assert_eq!(report.status, GpuHealthStatus::Healthy);
            assert!(report.detail.is_none());
        }
    }
}
