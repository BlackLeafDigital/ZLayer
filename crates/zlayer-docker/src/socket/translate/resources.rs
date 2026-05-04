//! Docker `HostConfig` resource fields -> `ZLayer` `ResourcesSpec`.
//!
//! Folds the dozen-or-so Docker resource knobs (CPU shares, memory tiers,
//! pids limit, blkio weight, OOM tuning, etc.) into `ZLayer`'s flat
//! [`ResourcesSpec`].
//!
//! Conversion rules:
//! - `NanoCpus` (1e9 = 1 CPU) -> `cpu: f64`
//! - `Memory` (bytes) -> `memory: String` in `"<bytes>b"` form, which the
//!   memory-string parser accepts (suffix `b` = bytes).
//! - Memory swap / reservation get the same byte-suffixed string treatment.
//! - Numeric ranges that don't fit `ZLayer`'s narrower types (e.g. `i64`
//!   `cpu_shares` -> `u32`) are saturated rather than dropped, so a
//!   too-large input still produces a usable spec.

use zlayer_types::spec::ResourcesSpec;

use crate::socket::types::container_create::HostConfig;

/// Translate the resource-knob subset of [`HostConfig`] into a
/// [`ResourcesSpec`].
///
/// Always returns a value (never `None`) because every field is optional —
/// a `HostConfig` with no resource knobs simply produces a default
/// [`ResourcesSpec`]. Callers should treat the result as additive: combine
/// with any other `ResourcesSpec` source on the request and the daemon will
/// pick the most-specific value.
#[must_use]
pub fn translate(hc: &HostConfig) -> ResourcesSpec {
    let cpu = hc.nano_cpus.filter(|n| *n > 0).map(|n| {
        // `nano_cpus` is a Docker count; even a 64-bit value comfortably
        // fits in an f64 mantissa for any practical CPU configuration
        // (~9e15 nanoCPUs ~= 9 million CPUs). Suppress the lint locally
        // rather than introducing a dependency on a fractional type.
        #[allow(clippy::cast_precision_loss)]
        let nanos = n as f64;
        nanos / 1_000_000_000.0_f64
    });

    let memory = hc.memory.filter(|n| *n > 0).map(|n| format!("{n}b"));

    let memory_swap = hc.memory_swap.filter(|n| *n > 0).map(|n| format!("{n}b"));

    let memory_reservation = hc
        .memory_reservation
        .filter(|n| *n > 0)
        .map(|n| format!("{n}b"));

    let memory_swappiness = hc
        .memory_swappiness
        .filter(|n| (0..=100).contains(n))
        .and_then(|n| u8::try_from(n).ok());

    let cpu_shares = hc
        .cpu_shares
        .filter(|n| *n > 0)
        .map(|n| u32::try_from(n).unwrap_or(u32::MAX));

    let blkio_weight = hc
        .blkio_weight
        .filter(|n| *n > 0)
        .map(|n| u16::try_from(n).unwrap_or(u16::MAX));

    let cpuset = hc.cpuset_cpus.as_ref().filter(|s| !s.is_empty()).cloned();

    ResourcesSpec {
        cpu,
        memory,
        gpu: None,
        pids_limit: hc.pids_limit.filter(|n| *n > 0),
        cpuset,
        cpu_shares,
        memory_swap,
        memory_reservation,
        memory_swappiness,
        oom_score_adj: hc.oom_score_adj,
        oom_kill_disable: hc.oom_kill_disable,
        blkio_weight,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_host_config_yields_default_resources() {
        let hc = HostConfig::default();
        let r = translate(&hc);
        assert!(r.cpu.is_none());
        assert!(r.memory.is_none());
        assert!(r.pids_limit.is_none());
        assert!(r.cpu_shares.is_none());
        assert!(r.cpuset.is_none());
    }

    #[test]
    fn nano_cpus_translates_to_fractional_cpu() {
        let hc = HostConfig {
            nano_cpus: Some(1_500_000_000),
            ..HostConfig::default()
        };
        let r = translate(&hc);
        let cpu = r.cpu.unwrap();
        assert!((cpu - 1.5).abs() < f64::EPSILON);
    }

    #[test]
    fn memory_translates_to_byte_string() {
        let hc = HostConfig {
            memory: Some(536_870_912),
            ..HostConfig::default()
        };
        let r = translate(&hc);
        assert_eq!(r.memory.as_deref(), Some("536870912b"));
    }

    #[test]
    fn memory_swap_and_reservation_translate() {
        let hc = HostConfig {
            memory_swap: Some(2_147_483_648),
            memory_reservation: Some(268_435_456),
            ..HostConfig::default()
        };
        let r = translate(&hc);
        assert_eq!(r.memory_swap.as_deref(), Some("2147483648b"));
        assert_eq!(r.memory_reservation.as_deref(), Some("268435456b"));
    }

    #[test]
    fn pids_limit_threads_through() {
        let hc = HostConfig {
            pids_limit: Some(2048),
            ..HostConfig::default()
        };
        let r = translate(&hc);
        assert_eq!(r.pids_limit, Some(2048));
    }

    #[test]
    fn pids_limit_zero_is_dropped() {
        let hc = HostConfig {
            pids_limit: Some(0),
            ..HostConfig::default()
        };
        let r = translate(&hc);
        assert!(r.pids_limit.is_none());
    }

    #[test]
    fn cpu_shares_clamps_to_u32() {
        let hc = HostConfig {
            cpu_shares: Some(i64::MAX),
            ..HostConfig::default()
        };
        let r = translate(&hc);
        assert_eq!(r.cpu_shares, Some(u32::MAX));
    }

    #[test]
    fn cpuset_threads_through() {
        let hc = HostConfig {
            cpuset_cpus: Some("0-3".to_string()),
            ..HostConfig::default()
        };
        let r = translate(&hc);
        assert_eq!(r.cpuset.as_deref(), Some("0-3"));
    }

    #[test]
    fn empty_cpuset_string_is_dropped() {
        let hc = HostConfig {
            cpuset_cpus: Some(String::new()),
            ..HostConfig::default()
        };
        let r = translate(&hc);
        assert!(r.cpuset.is_none());
    }

    #[test]
    fn memory_swappiness_clamped_to_valid_range() {
        let in_range = HostConfig {
            memory_swappiness: Some(60),
            ..HostConfig::default()
        };
        assert_eq!(translate(&in_range).memory_swappiness, Some(60));

        let out_of_range = HostConfig {
            memory_swappiness: Some(150),
            ..HostConfig::default()
        };
        assert!(translate(&out_of_range).memory_swappiness.is_none());

        let negative = HostConfig {
            memory_swappiness: Some(-1),
            ..HostConfig::default()
        };
        assert!(translate(&negative).memory_swappiness.is_none());
    }

    #[test]
    fn oom_fields_thread_through() {
        let hc = HostConfig {
            oom_score_adj: Some(-500),
            oom_kill_disable: Some(true),
            ..HostConfig::default()
        };
        let r = translate(&hc);
        assert_eq!(r.oom_score_adj, Some(-500));
        assert_eq!(r.oom_kill_disable, Some(true));
    }

    #[test]
    fn blkio_weight_threads_through() {
        let hc = HostConfig {
            blkio_weight: Some(500),
            ..HostConfig::default()
        };
        assert_eq!(translate(&hc).blkio_weight, Some(500));
    }
}
