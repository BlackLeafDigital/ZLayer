//! Cross-module diagnostic helpers shared by the GCS bridge and any host-side
//! code that wants to align its log timeline against ours.
//!
//! In particular: every "Hyper-V step N:" log emitted by
//! `crates/zlayer-agent/src/runtimes/hcs.rs::hyperv_create_via_gcs` is
//! mirrored to stderr alongside the bridge's `gcs-bridge-send` /
//! `gcs-bridge-reader` lines, so when the in-guest GCS bugchecks ~0.8 s
//! after Negotiate we can pin the failure to a specific host-side step
//! transition without manually time-aligning two different log timelines.

use std::sync::OnceLock;
use std::time::Instant;

/// Microseconds since the first call to [`ts_us`] in this process.
///
/// Anchored on first call so the timeline starts at "the first thing that
/// emitted a diagnostic," not unix-time. Lets a reader visually diff bridge
/// send/recv timing against guest-side WER 1000/1001 events captured via
/// the writable VSMB share.
pub fn ts_us() -> u128 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_micros()
}
