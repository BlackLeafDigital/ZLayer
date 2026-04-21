//! Container statistics (CPU, memory, storage) read from HCS.
//!
//! HCS returns statistics as a `Statistics` property on the compute system,
//! fetched via `HcsGetComputeSystemProperties`. This module wraps the JSON
//! query/response round-trip into a typed Rust API. Network statistics are
//! NOT in HCS Statistics — they come from a separate HNS endpoint call
//! (deferred to Phase C).

#![allow(clippy::missing_errors_doc)]

use serde::Serialize;

use crate::error::{HcsError, HcsResult};
use crate::schema::Statistics;
use crate::system::ComputeSystem;

/// Property query payload recognised by `HcsGetComputeSystemProperties`.
/// We only ask for `Statistics` here — asking for all properties is more
/// expensive (vmcompute.exe walks the process list, mounts, etc).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
struct PropertyQuery<'a> {
    property_types: &'a [&'a str],
}

impl ComputeSystem {
    /// Fetch the compute system's current statistics (CPU, memory, storage IO).
    ///
    /// Typical cadence: 2 seconds. Don't poll below ~500ms — `vmcompute.exe`
    /// CPU cost scales with container count × frequency.
    pub async fn read_statistics(&self) -> HcsResult<Statistics> {
        let query = PropertyQuery {
            property_types: &["Statistics"],
        };
        let query_json = serde_json::to_string(&query)?;
        let raw = self.properties(&query_json).await?;
        if raw.trim().is_empty() {
            return Ok(Statistics::default());
        }
        // HCS returns the compute-system properties as one JSON object; the
        // Statistics block is at the top level when requested via
        // PropertyTypes=["Statistics"]. Some HCS builds wrap it in a parent
        // envelope like `{"Statistics": { ... }}` — try both shapes.
        let parsed: Statistics = try_parse_stats_envelope(&raw)?;
        Ok(parsed)
    }
}

/// Accept both `{ ... Statistics fields ...}` and `{"Statistics": {...}}`.
fn try_parse_stats_envelope(raw: &str) -> HcsResult<Statistics> {
    #[derive(serde::Deserialize)]
    struct Envelope {
        #[serde(default, rename = "Statistics")]
        statistics: Option<Statistics>,
    }

    if let Ok(env) = serde_json::from_str::<Envelope>(raw) {
        if let Some(stats) = env.statistics {
            return Ok(stats);
        }
    }
    let flat: Statistics = serde_json::from_str(raw).map_err(HcsError::from)?;
    Ok(flat)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "Timestamp":"2026-04-21T12:34:56Z",
        "Uptime100ns":2960000000,
        "Processor":{"TotalRuntime100ns":1234567,"RuntimeUser100ns":900000,"RuntimeKernel100ns":334567},
        "Memory":{"MemoryUsageCommitBytes":268435456,"MemoryUsageCommitPeakBytes":314572800,"MemoryUsagePrivateWorkingSetBytes":201326592},
        "Storage":{"ReadCountNormalized":42,"ReadSizeBytes":1048576,"WriteCountNormalized":13,"WriteSizeBytes":262144}
    }"#;

    const ENVELOPED: &str = r#"{"Statistics":{
        "Timestamp":"2026-04-21T12:34:56Z",
        "Uptime100ns":42,
        "Processor":{"TotalRuntime100ns":100,"RuntimeUser100ns":60,"RuntimeKernel100ns":40}
    }}"#;

    #[test]
    fn parse_flat_statistics_json() {
        let stats = try_parse_stats_envelope(SAMPLE).expect("parse");
        assert_eq!(stats.uptime_100ns, 2_960_000_000);
        let proc = stats.processor.expect("processor present");
        assert_eq!(proc.total_runtime_100ns, 1_234_567);
        let mem = stats.memory.expect("memory present");
        assert_eq!(mem.memory_usage_private_working_set_bytes, 201_326_592);
    }

    #[test]
    fn parse_enveloped_statistics_json() {
        let stats = try_parse_stats_envelope(ENVELOPED).expect("parse enveloped");
        assert_eq!(stats.uptime_100ns, 42);
        assert_eq!(stats.processor.unwrap().total_runtime_100ns, 100);
    }

    #[test]
    fn parse_empty_returns_default() {
        // Defensive: garbage in → typed error, not panic.
        let r = try_parse_stats_envelope("not json");
        assert!(r.is_err());
    }
}
