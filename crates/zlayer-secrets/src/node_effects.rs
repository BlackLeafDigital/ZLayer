//! Node-local side-effect channel fired by the Raft apply wrapper.
//!
//! The Raft state machine apply is pure/deterministic — every replica must
//! reach the same `SecretsState` on the same op sequence. Filesystem and
//! network effects that should happen on every node when a particular op
//! applies cannot live inside `SecretsState::apply` itself. Instead the
//! apply *wrapper* (the closure handed to the openraft `RaftStateMachine`)
//! inspects the op post-apply and fires the corresponding handle here;
//! the daemon owns watcher tasks that await each handle and execute the
//! local effect (idempotently).
//!
//! New ops needing per-node effects should add a [`tokio::sync::Notify`]
//! field plus matching `fire_…` / `wait_…` methods rather than fanning
//! out into per-op types — the handle is a long-lived `Arc` shared with
//! the apply wrapper and the daemon's watcher tasks.

use std::sync::Arc;
use tokio::sync::Notify;

/// Node-local side-effect channel.
///
/// Held as an `Arc` shared between the Raft apply wrapper (which fires
/// notifies post-apply) and the daemon's watcher tasks (which await them
/// and run the local effect). All methods are cheap and lock-free.
#[derive(Debug, Default)]
pub struct NodeSideEffects {
    /// Fired when `SecretsRaftOp::WipeJoinSecret` applies successfully.
    /// The daemon's watcher deletes `{data_dir}/join_secret` on every
    /// wake. Idempotent if the file is already absent.
    wipe_join_secret: Notify,
}

impl NodeSideEffects {
    /// Construct a fresh `Arc<NodeSideEffects>` ready to share with the
    /// apply wrapper and watcher tasks.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Wake every current waiter on the `WipeJoinSecret` notify.
    ///
    /// If no task is currently awaiting, the fire is dropped on the
    /// floor — the boot-time reconcile in `zlayer serve` is what
    /// guarantees the wipe eventually happens for a node that wasn't
    /// awaiting at apply time (e.g. snapshot install before the
    /// watcher spawned).
    pub fn fire_wipe_join_secret(&self) {
        self.wipe_join_secret.notify_waiters();
    }

    /// Await the next `WipeJoinSecret` fire. Multiple concurrent
    /// awaiters are all woken when [`Self::fire_wipe_join_secret`]
    /// is called.
    pub async fn wait_wipe_join_secret(&self) {
        self.wipe_join_secret.notified().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fire_wakes_single_waiter() {
        let effects = NodeSideEffects::new();
        let cloned = Arc::clone(&effects);
        let handle = tokio::spawn(async move {
            cloned.wait_wipe_join_secret().await;
        });
        // Yield so the spawned task registers its waiter before we fire.
        tokio::task::yield_now().await;
        effects.fire_wipe_join_secret();
        tokio::time::timeout(std::time::Duration::from_secs(1), handle)
            .await
            .expect("waiter woke within timeout")
            .expect("waiter task did not panic");
    }

    #[tokio::test]
    async fn fire_wakes_multiple_waiters() {
        let effects = NodeSideEffects::new();
        let mut handles = Vec::new();
        for _ in 0..4 {
            let cloned = Arc::clone(&effects);
            handles.push(tokio::spawn(async move {
                cloned.wait_wipe_join_secret().await;
            }));
        }
        // Yield until all waiters are registered. One yield is
        // sufficient because spawned tasks run before the next
        // poll of this task on the current-thread runtime.
        tokio::task::yield_now().await;
        effects.fire_wipe_join_secret();
        for h in handles {
            tokio::time::timeout(std::time::Duration::from_secs(1), h)
                .await
                .expect("waiter woke within timeout")
                .expect("waiter task did not panic");
        }
    }

    #[tokio::test]
    async fn fire_without_waiter_is_harmless() {
        let effects = NodeSideEffects::new();
        // Fire with no current waiter; subsequent waiter must NOT
        // observe the prior fire (documents the boot-reconcile
        // trade-off).
        effects.fire_wipe_join_secret();
        let waited = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            effects.wait_wipe_join_secret(),
        )
        .await;
        assert!(
            waited.is_err(),
            "Notify::notify_waiters only wakes current waiters; \
             a post-fire await must NOT complete without a fresh fire",
        );
    }
}
