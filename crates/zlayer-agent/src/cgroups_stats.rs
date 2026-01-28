//! Cgroups v2 statistics reader for container metrics
//!
//! Provides functionality to read CPU and memory statistics from cgroups v2 filesystem
//! for container resource monitoring and autoscaling decisions.

use std::path::Path;
use std::time::Instant;

/// Container resource statistics from cgroups v2
#[derive(Debug, Clone)]
pub struct ContainerStats {
    /// CPU usage in microseconds
    pub cpu_usage_usec: u64,
    /// Current memory usage in bytes
    pub memory_bytes: u64,
    /// Memory limit in bytes (u64::MAX if unlimited)
    pub memory_limit: u64,
    /// Timestamp when stats were collected
    pub timestamp: Instant,
}

impl ContainerStats {
    /// Calculate memory usage as a percentage of the limit
    ///
    /// Returns 0.0 if there is no limit (memory_limit == u64::MAX)
    pub fn memory_percent(&self) -> f64 {
        if self.memory_limit == u64::MAX || self.memory_limit == 0 {
            0.0
        } else {
            (self.memory_bytes as f64 / self.memory_limit as f64) * 100.0
        }
    }
}

/// Read container statistics from cgroups v2 filesystem
///
/// Reads the following cgroup files:
/// - `cpu.stat` for CPU usage (usage_usec field)
/// - `memory.current` for current memory usage
/// - `memory.max` for memory limit
///
/// # Arguments
/// * `cgroup_path` - Path to the container's cgroup directory
///
/// # Returns
/// * `Ok(ContainerStats)` - Container statistics on success
/// * `Err(io::Error)` - If any cgroup file cannot be read
///
/// # Example
/// ```no_run
/// use std::path::Path;
/// use zlayer_agent::cgroups_stats::read_container_stats;
///
/// # async fn example() -> std::io::Result<()> {
/// let cgroup_path = Path::new("/sys/fs/cgroup/system.slice/zlayer-mycontainer.scope");
/// let stats = read_container_stats(cgroup_path).await?;
/// println!("CPU usage: {} usec", stats.cpu_usage_usec);
/// println!("Memory: {} bytes", stats.memory_bytes);
/// # Ok(())
/// # }
/// ```
pub async fn read_container_stats(cgroup_path: &Path) -> std::io::Result<ContainerStats> {
    // Read cpu.stat file
    let cpu_stat_path = cgroup_path.join("cpu.stat");
    let cpu_stat = tokio::fs::read_to_string(&cpu_stat_path).await?;

    // Read memory.current file
    let memory_current_path = cgroup_path.join("memory.current");
    let memory_current = tokio::fs::read_to_string(&memory_current_path).await?;

    // Read memory.max file
    let memory_max_path = cgroup_path.join("memory.max");
    let memory_max = tokio::fs::read_to_string(&memory_max_path).await?;

    // Parse cpu.stat
    // Format: "usage_usec 12345\nuser_usec 6789\nsystem_usec 5556\n..."
    let cpu_usage_usec = cpu_stat
        .lines()
        .find(|line| line.starts_with("usage_usec"))
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);

    // Parse memory.current (single integer value)
    let memory_bytes = memory_current.trim().parse::<u64>().unwrap_or(0);

    // Parse memory.max
    // Can be "max" (unlimited) or an integer
    let memory_limit = memory_max.trim().parse::<u64>().unwrap_or(u64::MAX); // "max" will fail to parse, use u64::MAX

    Ok(ContainerStats {
        cpu_usage_usec,
        memory_bytes,
        memory_limit,
        timestamp: Instant::now(),
    })
}

/// Calculate CPU percentage from two consecutive samples
///
/// The CPU percentage is calculated as:
/// ```text
/// cpu_percent = (delta_usage_usec / (delta_time_usec * num_cpus)) * 100
/// ```
///
/// This accounts for multi-core systems by dividing by the number of CPUs.
/// A value of 100% means full utilization of one CPU core.
/// Values can exceed 100% on multi-core systems if multiple cores are utilized.
///
/// # Arguments
/// * `prev` - Previous statistics sample
/// * `curr` - Current statistics sample
///
/// # Returns
/// CPU usage percentage (0.0 to N*100.0 where N is number of CPUs)
///
/// # Example
/// ```
/// use zlayer_agent::cgroups_stats::{ContainerStats, calculate_cpu_percent};
/// use std::time::Instant;
///
/// let prev = ContainerStats {
///     cpu_usage_usec: 1000000,  // 1 second of CPU time
///     memory_bytes: 1024,
///     memory_limit: 2048,
///     timestamp: Instant::now(),
/// };
///
/// // Simulate 0.5 seconds later with 0.25 seconds more CPU time
/// let curr = ContainerStats {
///     cpu_usage_usec: 1250000,  // 0.25 more seconds of CPU time
///     memory_bytes: 1024,
///     memory_limit: 2048,
///     timestamp: Instant::now(),  // In reality this would be ~0.5s later
/// };
///
/// let cpu_pct = calculate_cpu_percent(&prev, &curr);
/// // Result depends on elapsed time and num_cpus
/// ```
pub fn calculate_cpu_percent(prev: &ContainerStats, curr: &ContainerStats) -> f64 {
    // Calculate CPU usage delta in microseconds
    let usage_delta_usec = curr.cpu_usage_usec.saturating_sub(prev.cpu_usage_usec);

    // Calculate time delta in microseconds
    let time_delta = curr.timestamp.duration_since(prev.timestamp);
    let time_delta_usec = time_delta.as_micros() as u64;

    // Avoid division by zero
    if time_delta_usec == 0 {
        return 0.0;
    }

    // Get number of CPU cores
    let num_cpus = num_cpus::get() as u64;

    // Calculate percentage
    // Formula: (usage_delta / (time_delta * num_cpus)) * 100
    // This normalizes to 100% = full single-core utilization
    (usage_delta_usec as f64 / (time_delta_usec * num_cpus) as f64) * 100.0
}

/// Calculate CPU percentage with a specified number of CPUs
///
/// Same as `calculate_cpu_percent` but allows specifying the number of CPUs
/// for testing or container-specific CPU limits.
///
/// # Arguments
/// * `prev` - Previous statistics sample
/// * `curr` - Current statistics sample
/// * `num_cpus` - Number of CPUs to use in calculation
///
/// # Returns
/// CPU usage percentage
pub fn calculate_cpu_percent_with_cpus(
    prev: &ContainerStats,
    curr: &ContainerStats,
    num_cpus: u64,
) -> f64 {
    let usage_delta_usec = curr.cpu_usage_usec.saturating_sub(prev.cpu_usage_usec);
    let time_delta = curr.timestamp.duration_since(prev.timestamp);
    let time_delta_usec = time_delta.as_micros() as u64;

    if time_delta_usec == 0 || num_cpus == 0 {
        return 0.0;
    }

    (usage_delta_usec as f64 / (time_delta_usec * num_cpus) as f64) * 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_memory_percent() {
        let stats = ContainerStats {
            cpu_usage_usec: 0,
            memory_bytes: 512,
            memory_limit: 1024,
            timestamp: Instant::now(),
        };
        assert!((stats.memory_percent() - 50.0).abs() < 0.01);
    }

    #[test]
    fn test_memory_percent_unlimited() {
        let stats = ContainerStats {
            cpu_usage_usec: 0,
            memory_bytes: 512,
            memory_limit: u64::MAX,
            timestamp: Instant::now(),
        };
        assert_eq!(stats.memory_percent(), 0.0);
    }

    #[test]
    fn test_memory_percent_zero_limit() {
        let stats = ContainerStats {
            cpu_usage_usec: 0,
            memory_bytes: 512,
            memory_limit: 0,
            timestamp: Instant::now(),
        };
        assert_eq!(stats.memory_percent(), 0.0);
    }

    #[test]
    fn test_calculate_cpu_percent_with_cpus() {
        let now = Instant::now();
        let prev = ContainerStats {
            cpu_usage_usec: 1_000_000, // 1 second
            memory_bytes: 1024,
            memory_limit: 2048,
            timestamp: now,
        };

        // Simulate 1 second later with 500ms more CPU time (50% of one core)
        let later = now + Duration::from_secs(1);
        let curr = ContainerStats {
            cpu_usage_usec: 1_500_000, // 1.5 seconds total
            memory_bytes: 1024,
            memory_limit: 2048,
            timestamp: later,
        };

        // With 1 CPU, 500ms usage over 1s = 50%
        let cpu_pct = calculate_cpu_percent_with_cpus(&prev, &curr, 1);
        assert!((cpu_pct - 50.0).abs() < 1.0);

        // With 2 CPUs, 500ms usage over 1s = 25% (per-core normalized)
        let cpu_pct_2 = calculate_cpu_percent_with_cpus(&prev, &curr, 2);
        assert!((cpu_pct_2 - 25.0).abs() < 1.0);
    }

    #[test]
    fn test_calculate_cpu_percent_zero_time() {
        let now = Instant::now();
        let stats = ContainerStats {
            cpu_usage_usec: 1_000_000,
            memory_bytes: 1024,
            memory_limit: 2048,
            timestamp: now,
        };

        // Same timestamp should return 0
        let cpu_pct = calculate_cpu_percent_with_cpus(&stats, &stats, 1);
        assert_eq!(cpu_pct, 0.0);
    }

    #[test]
    fn test_calculate_cpu_percent_zero_cpus() {
        let now = Instant::now();
        let prev = ContainerStats {
            cpu_usage_usec: 1_000_000,
            memory_bytes: 1024,
            memory_limit: 2048,
            timestamp: now,
        };

        let later = now + Duration::from_secs(1);
        let curr = ContainerStats {
            cpu_usage_usec: 1_500_000,
            memory_bytes: 1024,
            memory_limit: 2048,
            timestamp: later,
        };

        // Zero CPUs should return 0 (avoid division by zero)
        let cpu_pct = calculate_cpu_percent_with_cpus(&prev, &curr, 0);
        assert_eq!(cpu_pct, 0.0);
    }

    #[test]
    fn test_stats_clone() {
        let stats = ContainerStats {
            cpu_usage_usec: 1000,
            memory_bytes: 2000,
            memory_limit: 4000,
            timestamp: Instant::now(),
        };

        let cloned = stats.clone();
        assert_eq!(cloned.cpu_usage_usec, stats.cpu_usage_usec);
        assert_eq!(cloned.memory_bytes, stats.memory_bytes);
        assert_eq!(cloned.memory_limit, stats.memory_limit);
    }
}
