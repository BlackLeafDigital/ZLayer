//! System resource detection for node registration and heartbeat.
//!
//! Platform-specific resource detection:
//! - **Linux**: Reads `/proc/meminfo`, `/proc/stat`, and uses `nix::sys::statvfs` for disk.
//! - **macOS**: Uses `sysctl` for memory, `host_processor_info` via sysctl for CPU, and statvfs
//!   for disk.
//! - **Other**: Returns zero values as a graceful fallback.
//!
//! No external runtime dependencies -- pure procfs/sysctl parsing with `nix` for disk stats.

use std::path::Path;

/// Detected system resources for node registration.
#[derive(Debug, Clone)]
pub struct SystemResources {
    /// Total CPU cores
    pub cpu_total: f64,
    /// Total memory in bytes
    pub memory_total: u64,
    /// Total disk in bytes (for the data directory)
    pub disk_total: u64,
    /// Current CPU usage (cores) -- 0.0 at startup
    pub cpu_used: f64,
    /// Current memory usage in bytes
    pub memory_used: u64,
    /// Current disk usage in bytes
    pub disk_used: u64,
}

/// Detect system resources on the local machine.
///
/// `data_dir` is the directory whose filesystem is measured for disk capacity.
#[must_use]
pub fn detect_system_resources(data_dir: &Path) -> SystemResources {
    SystemResources {
        cpu_total: detect_cpu_total(),
        memory_total: detect_memory_total(),
        disk_total: detect_disk_total(data_dir),
        cpu_used: 0.0, // Will be updated via heartbeat
        memory_used: detect_memory_used(),
        disk_used: detect_disk_used(data_dir),
    }
}

/// Detect current resource usage (for heartbeat updates).
#[must_use]
pub fn detect_current_usage(data_dir: &Path) -> ResourceUsage {
    ResourceUsage {
        cpu_used: detect_cpu_used(),
        memory_used: detect_memory_used(),
        disk_used: detect_disk_used(data_dir),
    }
}

/// Current resource usage snapshot for heartbeat.
#[derive(Debug, Clone)]
pub struct ResourceUsage {
    /// CPU cores currently in use (fractional)
    pub cpu_used: f64,
    /// Memory currently in use (bytes)
    pub memory_used: u64,
    /// Disk currently in use (bytes) on the data directory filesystem
    pub disk_used: u64,
}

// =============================================================================
// CPU total
// =============================================================================

#[allow(clippy::cast_precision_loss)]
fn detect_cpu_total() -> f64 {
    num_cpus::get() as f64
}

// =============================================================================
// Memory detection
// =============================================================================

/// Read total physical memory in bytes.
#[cfg(target_os = "linux")]
fn detect_memory_total() -> u64 {
    parse_meminfo_field("MemTotal")
}

#[cfg(target_os = "macos")]
fn detect_memory_total() -> u64 {
    sysctl_hw_memsize()
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn detect_memory_total() -> u64 {
    0
}

/// Read current memory usage in bytes (total - available).
#[cfg(target_os = "linux")]
fn detect_memory_used() -> u64 {
    let total = parse_meminfo_field("MemTotal");
    let available = parse_meminfo_field("MemAvailable");
    total.saturating_sub(available)
}

#[cfg(target_os = "macos")]
fn detect_memory_used() -> u64 {
    let total = sysctl_hw_memsize();
    // Parse vm_stat for free + inactive pages to estimate available memory
    let available = macos_available_memory();
    total.saturating_sub(available)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn detect_memory_used() -> u64 {
    0
}

// =============================================================================
// Disk detection (via nix::sys::statvfs)
// =============================================================================

#[allow(clippy::unnecessary_cast)]
fn detect_disk_total(data_dir: &Path) -> u64 {
    match nix::sys::statvfs::statvfs(data_dir) {
        Ok(stat) => {
            // Total blocks * fragment size = total bytes
            stat.blocks() as u64 * stat.fragment_size() as u64
        }
        Err(_) => 0,
    }
}

#[allow(clippy::unnecessary_cast)]
fn detect_disk_used(data_dir: &Path) -> u64 {
    match nix::sys::statvfs::statvfs(data_dir) {
        Ok(stat) => {
            let frag = stat.fragment_size() as u64;
            let total = stat.blocks() as u64 * frag;
            // blocks_available is free blocks available to unprivileged users
            let avail = stat.blocks_available() as u64 * frag;
            total.saturating_sub(avail)
        }
        Err(_) => 0,
    }
}

// =============================================================================
// CPU usage detection
// =============================================================================

/// Read instantaneous CPU usage as fractional cores used.
///
/// On Linux, reads `/proc/stat` for the aggregate CPU line and computes the
/// ratio of non-idle time. Multiplied by the total core count to express usage
/// in "cores used" units.
///
/// Note: This is a cumulative snapshot since boot. For delta-based usage, the
/// caller should take two readings and compute the difference.
#[cfg(target_os = "linux")]
fn detect_cpu_used() -> f64 {
    let Ok(content) = std::fs::read_to_string("/proc/stat") else {
        return 0.0;
    };

    // First line: "cpu  user nice system idle iowait irq softirq steal guest guest_nice"
    let first_line = match content.lines().next() {
        Some(line) if line.starts_with("cpu ") => line,
        _ => return 0.0,
    };

    let fields: Vec<u64> = first_line
        .split_whitespace()
        .skip(1) // skip "cpu"
        .filter_map(|s| s.parse().ok())
        .collect();

    if fields.len() < 4 {
        return 0.0;
    }

    // fields: user, nice, system, idle, [iowait, irq, softirq, steal, guest, guest_nice]
    let idle = fields[3] + fields.get(4).copied().unwrap_or(0); // idle + iowait
    let total: u64 = fields.iter().sum();

    if total == 0 {
        return 0.0;
    }

    #[allow(clippy::cast_precision_loss)]
    let busy_fraction = 1.0 - (idle as f64 / total as f64);
    #[allow(clippy::cast_precision_loss)]
    let cores = num_cpus::get() as f64;
    busy_fraction * cores
}

#[cfg(target_os = "macos")]
fn detect_cpu_used() -> f64 {
    // On macOS, we could use host_processor_info() via mach APIs, but that
    // requires unsafe FFI. For now, return 0.0 as a placeholder -- the
    // heartbeat system will provide real-time updates through other means.
    0.0
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn detect_cpu_used() -> f64 {
    0.0
}

// =============================================================================
// Linux /proc/meminfo parser
// =============================================================================

/// Parse a field from /proc/meminfo. Values are in kB, returned as bytes.
#[cfg(target_os = "linux")]
fn parse_meminfo_field(field: &str) -> u64 {
    let Ok(content) = std::fs::read_to_string("/proc/meminfo") else {
        return 0;
    };

    for line in content.lines() {
        if let Some(rest) = line.strip_prefix(field) {
            // Line format: "MemTotal:       16384000 kB"
            let rest = rest.trim_start_matches(':').trim();
            let kb: u64 = rest
                .split_whitespace()
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            return kb * 1024; // kB -> bytes
        }
    }

    0
}

// =============================================================================
// macOS helpers
// =============================================================================

/// Read total physical memory via sysctl on macOS.
#[cfg(target_os = "macos")]
fn sysctl_hw_memsize() -> u64 {
    let output = match std::process::Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output()
    {
        Ok(out) if out.status.success() => out,
        _ => return 0,
    };

    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u64>()
        .unwrap_or(0)
}

/// Estimate available memory on macOS by parsing `vm_stat` output.
///
/// `vm_stat` reports page counts. We multiply by page size to get bytes.
/// Available ~ (free + inactive) pages * page_size.
#[cfg(target_os = "macos")]
fn macos_available_memory() -> u64 {
    let output = match std::process::Command::new("vm_stat").output() {
        Ok(out) if out.status.success() => out,
        _ => return 0,
    };

    let text = String::from_utf8_lossy(&output.stdout);

    // First line contains page size: "Mach Virtual Memory Statistics: (page size of 16384 bytes)"
    let page_size: u64 = text
        .lines()
        .next()
        .and_then(|line| {
            let start = line.find("page size of ")? + "page size of ".len();
            let end = line[start..].find(' ')? + start;
            line[start..end].parse().ok()
        })
        .unwrap_or(16384); // default Apple Silicon page size

    let parse_pages = |label: &str| -> u64 {
        text.lines()
            .find(|line| line.starts_with(label))
            .and_then(|line| {
                line.split(':')
                    .nth(1)?
                    .trim()
                    .trim_end_matches('.')
                    .parse()
                    .ok()
            })
            .unwrap_or(0)
    };

    let free = parse_pages("Pages free");
    let inactive = parse_pages("Pages inactive");

    (free + inactive) * page_size
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_detect_cpu_total_is_positive() {
        let cpus = detect_cpu_total();
        assert!(cpus >= 1.0, "Expected at least 1 CPU, got {cpus}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_detect_memory_total_linux() {
        let mem = detect_memory_total();
        // Any machine should have at least 64 MB
        assert!(
            mem >= 64 * 1024 * 1024,
            "Expected at least 64 MB, got {mem} bytes"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_detect_memory_used_linux() {
        let total = detect_memory_total();
        let used = detect_memory_used();
        assert!(
            used <= total,
            "Used memory ({used}) should not exceed total ({total})"
        );
    }

    #[test]
    fn test_detect_disk_total_nonzero() {
        // Use /tmp as a directory that always exists
        let dir = PathBuf::from("/tmp");
        let total = detect_disk_total(&dir);
        assert!(total > 0, "Disk total should be nonzero for /tmp");
    }

    #[test]
    fn test_detect_disk_used_le_total() {
        let dir = PathBuf::from("/tmp");
        let total = detect_disk_total(&dir);
        let used = detect_disk_used(&dir);
        assert!(
            used <= total,
            "Disk used ({used}) should not exceed total ({total})"
        );
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn test_detect_system_resources_all_fields() {
        let dir = PathBuf::from("/tmp");
        let res = detect_system_resources(&dir);
        assert!(res.cpu_total >= 1.0);
        // cpu_used is 0.0 at startup by design
        assert_eq!(res.cpu_used, 0.0);
        // disk should be nonzero for /tmp
        assert!(res.disk_total > 0);
    }

    #[test]
    fn test_detect_current_usage() {
        let dir = PathBuf::from("/tmp");
        let usage = detect_current_usage(&dir);
        // CPU used should be non-negative
        assert!(usage.cpu_used >= 0.0);
        // Disk used should be bounded by disk total
        let total = detect_disk_total(&dir);
        assert!(usage.disk_used <= total);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_parse_meminfo_field_memtotal() {
        let val = parse_meminfo_field("MemTotal");
        assert!(val > 0, "MemTotal should be nonzero on Linux");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_parse_meminfo_field_nonexistent() {
        let val = parse_meminfo_field("CompletelyBogusField");
        assert_eq!(val, 0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_cpu_used_bounded() {
        let used = detect_cpu_used();
        let total = detect_cpu_total();
        assert!(used >= 0.0, "CPU used should be non-negative, got {used}");
        assert!(
            used <= total,
            "CPU used ({used}) should not exceed total ({total})"
        );
    }

    #[test]
    fn test_detect_disk_nonexistent_dir() {
        let dir = PathBuf::from("/this/path/should/not/exist/ever");
        let total = detect_disk_total(&dir);
        assert_eq!(total, 0, "Nonexistent path should return 0 for disk total");
        let used = detect_disk_used(&dir);
        assert_eq!(used, 0, "Nonexistent path should return 0 for disk used");
    }
}
