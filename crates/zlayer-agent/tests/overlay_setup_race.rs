//! Regression test for the `setup_service_overlay` race that produced
//! `Fatal read error on tun interface: Os { code: 77 }` (EBADFD) on the
//! user's daemon at startup.
//!
//! Background: the previous (v0.50, pre-bridge) implementation held a read
//! lock during the existence check and then dropped it before acquiring a
//! write lock for the insert. Two concurrent callers (e.g.
//! `restore_deployments` racing the deploy API handler) could both pass
//! the read-lock check, both enter resource creation, and the second
//! one's netlink activity would yank the first's live device out from
//! under a running boringtun worker.
//!
//! The fix holds a single write lock across the entire check-and-create
//! so concurrent callers see the existing entry and short-circuit.
//!
//! v0.51 rewrote `setup_service_overlay` to create a per-service Linux
//! **bridge** (instead of a per-service `WireGuard` TUN), so the device
//! lifetime concern is different — but the lock semantics still matter.
//! Two concurrent callers must produce exactly one `ServiceBridge`
//! (otherwise the second `netlink::create_bridge` would fail with
//! `EEXIST` or silently leak state across nodes).
//!
//! All tests in this file require `CAP_NET_ADMIN` because
//! `netlink::create_bridge` is a privileged operation. They are
//! `#[ignore]`d so the workspace test suite stays green in CI without
//! elevated caps. Run them locally with:
//!
//! ```sh
//! sudo -E cargo test -p zlayer-agent --test overlay_setup_race -- --ignored --nocapture
//! ```

use std::sync::Arc;
use zlayer_agent::overlay_manager::OverlayManager;
use zlayer_types::overlay::OverlayMode;

/// Spawn 16 concurrent `setup_service_overlay` calls for the same service
/// name on a shared `OverlayManager`. Assert that all callers receive the
/// same bridge name and that exactly one `service_bridges` entry exists
/// after the storm.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CAP_NET_ADMIN to create real Linux bridges; run with: sudo -E cargo test -p zlayer-agent --test overlay_setup_race -- --ignored --nocapture"]
async fn concurrent_setup_service_overlay_inserts_once() {
    let om = Arc::new(
        OverlayManager::new("racetest".to_string(), "test".to_string())
            .await
            .expect("OverlayManager::new"),
    );

    let mut handles = Vec::with_capacity(16);
    for _ in 0..16 {
        let om = Arc::clone(&om);
        handles.push(tokio::spawn(async move {
            om.setup_service_overlay("svc-x", OverlayMode::Shared).await
        }));
    }

    let mut names = Vec::with_capacity(16);
    for h in handles {
        let result = h
            .await
            .expect("task join")
            .expect("setup_service_overlay should succeed under CAP_NET_ADMIN");
        names.push(result.name);
    }

    let first = names[0].clone();
    assert!(
        names.iter().all(|n| n == &first),
        "all 16 concurrent callers should resolve to the same bridge name; \
         got at least two distinct results: {names:?}"
    );

    // Bridges-side assertion: under the new per-service bridge model the
    // authoritative state is in `service_bridges`. With the lock fix, 16
    // concurrent callers produce exactly one bridge entry.
    #[cfg(target_os = "linux")]
    assert_eq!(
        om.service_bridge_count().await,
        1,
        "exactly one service_bridges entry should exist after concurrent setup"
    );

    // Interface-map mirror assertion: `service_interfaces` is populated in
    // lockstep with `service_bridges` and now maps to bridge names.
    let count = om.service_count().await;
    assert_eq!(
        count, 1,
        "exactly one service_interfaces entry should exist after concurrent setup; got {count}"
    );
}

/// Same race scenario across different service names. Each name should
/// resolve to a distinct bridge name and produce its own entry.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CAP_NET_ADMIN to create real Linux bridges; run with: sudo -E cargo test -p zlayer-agent --test overlay_setup_race -- --ignored --nocapture"]
async fn concurrent_setup_distinct_service_names_each_get_one_entry() {
    let om = Arc::new(
        OverlayManager::new("racetest2".to_string(), "test".to_string())
            .await
            .expect("OverlayManager::new"),
    );

    let names = ["svc-a", "svc-b", "svc-c", "svc-d"];
    let mut handles = Vec::new();
    // 4 callers per service, racing.
    for &svc in &names {
        for _ in 0..4 {
            let om = Arc::clone(&om);
            handles.push(tokio::spawn(async move {
                let res = om.setup_service_overlay(svc, OverlayMode::Shared).await;
                (svc, res)
            }));
        }
    }

    let mut by_service: std::collections::HashMap<&str, Vec<String>> =
        std::collections::HashMap::new();
    for h in handles {
        let (svc, res) = h.await.expect("task join");
        let iface = res.expect("setup_service_overlay should not error");
        by_service.entry(svc).or_default().push(iface.name);
    }

    // Every service got 4 results, all the same.
    for &svc in &names {
        let results = by_service.get(svc).expect("service results present");
        assert_eq!(results.len(), 4, "{svc} should have 4 results");
        let first = results[0].clone();
        assert!(
            results.iter().all(|n| n == &first),
            "all 4 callers for {svc} should resolve to the same bridge name; got {results:?}"
        );
    }

    // Distinct services have distinct bridge names.
    let mut all_names: Vec<String> = by_service.values().map(|v| v[0].clone()).collect();
    all_names.sort();
    all_names.dedup();
    assert_eq!(
        all_names.len(),
        names.len(),
        "distinct services should have distinct bridge names"
    );

    // service_interfaces map has exactly one entry per service.
    assert_eq!(om.service_count().await, names.len());

    // service_bridges (the new authoritative map) likewise has one entry per service.
    #[cfg(target_os = "linux")]
    assert_eq!(om.service_bridge_count().await, names.len());
}

/// Full-kernel variant that also exercises the cluster overlay path:
/// brings up the global `OverlayTransport` (real TUN), then storms 8
/// `setup_service_overlay` callers, and verifies the per-service bridge
/// map has exactly one entry. Requires `CAP_NET_ADMIN`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CAP_NET_ADMIN; run with: sudo -E cargo test -p zlayer-agent --test overlay_setup_race -- --ignored --nocapture"]
async fn concurrent_setup_with_global_overlay_creates_one_bridge_per_service() {
    let mut om_owned = OverlayManager::new("racetest3".to_string(), "test".to_string())
        .await
        .expect("OverlayManager::new");
    if let Err(e) = om_owned.setup_global_overlay().await {
        eprintln!(
            "setup_global_overlay failed (expected without CAP_NET_ADMIN): {e}; \
             skipping further kernel assertions"
        );
        return;
    }
    let om = Arc::new(om_owned);

    let mut handles = Vec::with_capacity(8);
    for _ in 0..8 {
        let om = Arc::clone(&om);
        handles.push(tokio::spawn(async move {
            om.setup_service_overlay("svc-real", OverlayMode::Shared)
                .await
        }));
    }

    let mut names = Vec::with_capacity(8);
    for h in handles {
        names.push(h.await.expect("task join").expect("setup").name);
    }

    let first = names[0].clone();
    assert!(names.iter().all(|n| n == &first));
    assert_eq!(om.service_count().await, 1);
    #[cfg(target_os = "linux")]
    assert_eq!(om.service_bridge_count().await, 1);

    // Best-effort: count kernel links matching the prefix. With one global
    // transport (`zl-...-g`) and one service bridge (`zl-...-b`), there
    // should be at least 2 `zl-` links visible on the host.
    if let Ok(out) = tokio::process::Command::new("ip")
        .args(["-br", "link"])
        .output()
        .await
    {
        if out.status.success() {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let zl_count = stdout
                .lines()
                .filter(|line| {
                    line.split_whitespace()
                        .next()
                        .is_some_and(|i| i.starts_with("zl-"))
                })
                .count();
            // global + 1 service bridge = 2 expected.
            assert!(
                zl_count >= 2,
                "expected at least one global + one service zl-* link; saw {zl_count}: {stdout}"
            );
        }
    }
}
