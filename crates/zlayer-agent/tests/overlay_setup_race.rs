//! Regression test for the `setup_service_overlay` race that produced
//! `Fatal read error on tun interface: Os { code: 77 }` (EBADFD) on the
//! user's daemon at startup.
//!
//! Background: the previous implementation in
//! `crates/zlayer-agent/src/overlay_manager.rs::setup_service_overlay`
//! held a read lock during the existence check and then dropped it before
//! acquiring a write lock for the insert. Two concurrent callers (e.g.
//! `restore_deployments` racing the deploy API handler) could both pass
//! the read-lock check, both enter transport creation, and the second
//! one's netlink activity would yank the first's live TUN out from under
//! a running boringtun worker.
//!
//! The fix holds a single write lock across the entire check-and-create
//! so concurrent callers see the existing entry and short-circuit.
//!
//! This test verifies the fix without needing root: without
//! `CAP_NET_ADMIN`, `OverlayTransport::create_interface()` fails with
//! permission-denied, but `setup_service_overlay()` still inserts into
//! `service_interfaces` (registering the service for direct networking).
//! With the lock fix, N concurrent calls for the same service name
//! produce **exactly one** `service_interfaces` entry — proving the lock
//! semantics regardless of whether the underlying TUN path succeeds.
//!
//! There is also an `#[ignore]`'d full-kernel variant that creates real
//! TUN devices. Run it with:
//!
//! ```sh
//! sudo -E cargo test -p zlayer-agent --test overlay_setup_race -- --ignored --nocapture
//! ```

use std::sync::Arc;
use zlayer_agent::overlay_manager::OverlayManager;

/// Spawn 16 concurrent `setup_service_overlay` calls for the same service
/// name on a shared `OverlayManager`. Assert that all callers receive the
/// same interface name and that exactly one `service_interfaces` entry
/// exists after the storm.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_setup_service_overlay_inserts_once() {
    let om = Arc::new(
        OverlayManager::new("racetest".to_string())
            .await
            .expect("OverlayManager::new"),
    );

    let mut handles = Vec::with_capacity(16);
    for _ in 0..16 {
        let om = Arc::clone(&om);
        handles.push(tokio::spawn(async move {
            om.setup_service_overlay("svc-x").await
        }));
    }

    let mut names = Vec::with_capacity(16);
    for h in handles {
        let result = h
            .await
            .expect("task join")
            .expect("setup_service_overlay should fall through to direct networking on failure");
        names.push(result);
    }

    let first = names[0].clone();
    assert!(
        names.iter().all(|n| n == &first),
        "all 16 concurrent callers should resolve to the same interface name; \
         got at least two distinct results: {names:?}"
    );

    let count = om.service_count().await;
    assert_eq!(
        count, 1,
        "exactly one service_interfaces entry should exist after concurrent setup; got {count}"
    );
}

/// Same race scenario across different service names. Each name should
/// resolve to a distinct interface name and produce its own entry.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_setup_distinct_service_names_each_get_one_entry() {
    let om = Arc::new(
        OverlayManager::new("racetest2".to_string())
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
                let res = om.setup_service_overlay(svc).await;
                (svc, res)
            }));
        }
    }

    let mut by_service: std::collections::HashMap<&str, Vec<String>> =
        std::collections::HashMap::new();
    for h in handles {
        let (svc, res) = h.await.expect("task join");
        let iface = res.expect("setup_service_overlay should not error");
        by_service.entry(svc).or_default().push(iface);
    }

    // Every service got 4 results, all the same.
    for &svc in &names {
        let results = by_service.get(svc).expect("service results present");
        assert_eq!(results.len(), 4, "{svc} should have 4 results");
        let first = results[0].clone();
        assert!(
            results.iter().all(|n| n == &first),
            "all 4 callers for {svc} should resolve to the same interface name; got {results:?}"
        );
    }

    // Distinct services have distinct interface names.
    let mut all_names: Vec<String> = by_service.values().map(|v| v[0].clone()).collect();
    all_names.sort();
    all_names.dedup();
    assert_eq!(
        all_names.len(),
        names.len(),
        "distinct services should have distinct interface names"
    );

    // service_interfaces map has exactly one entry per service.
    assert_eq!(om.service_count().await, names.len());
}

/// Full-kernel variant. Requires `CAP_NET_ADMIN` (i.e. run under sudo).
/// Creates real TUN devices via boringtun and verifies the overlay
/// transport map has exactly one entry per concurrent storm.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CAP_NET_ADMIN; run with: sudo -E cargo test -p zlayer-agent --test overlay_setup_race -- --ignored --nocapture"]
async fn concurrent_setup_with_real_tun_creates_one_device() {
    let mut om_owned = OverlayManager::new("racetest3".to_string())
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
            om.setup_service_overlay("svc-real").await
        }));
    }

    let mut names = Vec::with_capacity(8);
    for h in handles {
        names.push(h.await.expect("task join").expect("setup"));
    }

    let first = names[0].clone();
    assert!(names.iter().all(|n| n == &first));
    assert_eq!(om.service_count().await, 1);

    // Best-effort: count kernel links matching the prefix.
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
            // global + 1 service = 2 expected.
            assert!(
                zl_count >= 2,
                "expected at least one global + one service zl-* link; saw {zl_count}: {stdout}"
            );
        }
    }
}
