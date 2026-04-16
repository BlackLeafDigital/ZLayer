//! Notifier storage implementations
//!
//! Provides both persistent (ZQL) and in-memory storage backends for notifiers
//! (named notification channels that send alerts to Slack, Discord, generic
//! webhooks, or SMTP endpoints).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use super::zql_json::{JsonTable, ZqlJsonStore};
use super::{StorageError, StoredNotifier};

/// Trait for notifier storage backends.
#[async_trait]
pub trait NotifierStorage: Send + Sync {
    /// Store (create or update) a notifier by id.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Database`] if the backing store rejects the
    /// write.
    async fn store(&self, notifier: &StoredNotifier) -> Result<(), StorageError>;

    /// Fetch a notifier by id.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the backing store fails.
    async fn get(&self, id: &str) -> Result<Option<StoredNotifier>, StorageError>;

    /// List all notifiers. Results are sorted by name ascending.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the backing store fails.
    async fn list(&self) -> Result<Vec<StoredNotifier>, StorageError>;

    /// Delete a notifier by id. Returns `true` if the row existed.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Database`] if the delete operation fails.
    async fn delete(&self, id: &str) -> Result<bool, StorageError>;
}

/// In-memory notifier store.
pub struct InMemoryNotifierStore {
    notifiers: Arc<RwLock<HashMap<String, StoredNotifier>>>,
}

impl InMemoryNotifierStore {
    /// Create a new empty in-memory notifier store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            notifiers: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Default for InMemoryNotifierStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl NotifierStorage for InMemoryNotifierStore {
    async fn store(&self, notifier: &StoredNotifier) -> Result<(), StorageError> {
        let mut notifiers = self.notifiers.write().await;
        notifiers.insert(notifier.id.clone(), notifier.clone());
        Ok(())
    }

    async fn get(&self, id: &str) -> Result<Option<StoredNotifier>, StorageError> {
        let notifiers = self.notifiers.read().await;
        Ok(notifiers.get(id).cloned())
    }

    async fn list(&self) -> Result<Vec<StoredNotifier>, StorageError> {
        let notifiers = self.notifiers.read().await;
        let mut list: Vec<_> = notifiers.values().cloned().collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(list)
    }

    async fn delete(&self, id: &str) -> Result<bool, StorageError> {
        let mut notifiers = self.notifiers.write().await;
        Ok(notifiers.remove(id).is_some())
    }
}

/// Static table specification for the notifiers table.
///
/// Notifiers are stored blob-only — there are no peeled secondary columns.
/// Name uniqueness is deliberately NOT enforced at the schema level: the
/// existing in-memory store allows duplicate names and the trait contract
/// does not promise uniqueness. If that becomes a requirement it should be
/// added here (via an `IndexSpec { column: "name", unique: true, ... }`) and
/// to `InMemoryNotifierStore` together, so the two backends stay in lock-step.
const NOTIFIERS_TABLE: JsonTable<StoredNotifier> = JsonTable {
    name: "notifiers",
    indexes: &[],
    unique_constraints: &[],
};

/// ZQL-backed persistent notifier store.
///
/// Delegates all storage operations to a [`ZqlJsonStore<StoredNotifier>`]
/// configured for the `notifiers` table. Records are serialised as typed
/// values into the underlying ZQL store keyed by `id`; the daemon reopens the
/// database on start-up and notifier configurations survive restarts.
pub struct ZqlNotifierStore {
    inner: ZqlJsonStore<StoredNotifier>,
}

impl ZqlNotifierStore {
    /// Open or create a ZQL database at the given path.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the database connection or table creation
    /// fails.
    pub async fn open<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        let inner = ZqlJsonStore::open(path, NOTIFIERS_TABLE).await?;
        Ok(Self { inner })
    }

    /// Create a ZQL database in a temporary directory (useful for tests).
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the temporary database cannot be created.
    #[cfg(test)]
    pub async fn in_memory() -> Result<Self, StorageError> {
        let inner = ZqlJsonStore::in_memory(NOTIFIERS_TABLE).await?;
        Ok(Self { inner })
    }
}

#[async_trait]
impl NotifierStorage for ZqlNotifierStore {
    async fn store(&self, notifier: &StoredNotifier) -> Result<(), StorageError> {
        self.inner.put(&notifier.id, notifier).await
    }

    async fn get(&self, id: &str) -> Result<Option<StoredNotifier>, StorageError> {
        self.inner.get(id).await
    }

    async fn list(&self) -> Result<Vec<StoredNotifier>, StorageError> {
        // The adapter orders by `id`; the trait contract sorts by name.
        let mut list = self.inner.list().await?;
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(list)
    }

    async fn delete(&self, id: &str) -> Result<bool, StorageError> {
        self.inner.delete(id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{NotifierConfig, NotifierKind, StoredNotifier};

    fn make_notifier(name: &str, kind: NotifierKind) -> StoredNotifier {
        let config = match kind {
            NotifierKind::Slack => NotifierConfig::Slack {
                webhook_url: "https://hooks.slack.com/test".to_string(),
            },
            NotifierKind::Discord => NotifierConfig::Discord {
                webhook_url: "https://discord.com/api/webhooks/test".to_string(),
            },
            NotifierKind::Webhook => NotifierConfig::Webhook {
                url: "https://example.com/hook".to_string(),
                method: None,
                headers: None,
            },
            NotifierKind::Smtp => NotifierConfig::Smtp {
                host: "smtp.example.com".to_string(),
                port: 587,
                username: "user".to_string(),
                password: "pass".to_string(),
                from: "noreply@example.com".to_string(),
                to: vec!["admin@example.com".to_string()],
            },
        };
        StoredNotifier::new(name, kind, config)
    }

    #[tokio::test]
    async fn test_store_and_get() {
        let store = InMemoryNotifierStore::new();
        let n = make_notifier("slack-alerts", NotifierKind::Slack);
        let id = n.id.clone();

        store.store(&n).await.unwrap();

        let retrieved = store.get(&id).await.unwrap().expect("notifier must exist");
        assert_eq!(retrieved.name, "slack-alerts");
        assert_eq!(retrieved.kind, NotifierKind::Slack);
        assert!(retrieved.enabled);
    }

    #[tokio::test]
    async fn test_list_sorted() {
        let store = InMemoryNotifierStore::new();
        store
            .store(&make_notifier("beta", NotifierKind::Discord))
            .await
            .unwrap();
        store
            .store(&make_notifier("alpha", NotifierKind::Slack))
            .await
            .unwrap();
        store
            .store(&make_notifier("gamma", NotifierKind::Webhook))
            .await
            .unwrap();

        let all = store.list().await.unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].name, "alpha");
        assert_eq!(all[1].name, "beta");
        assert_eq!(all[2].name, "gamma");
    }

    #[tokio::test]
    async fn test_delete() {
        let store = InMemoryNotifierStore::new();
        let n = make_notifier("doomed", NotifierKind::Slack);
        let id = n.id.clone();
        store.store(&n).await.unwrap();

        assert!(store.delete(&id).await.unwrap());
        assert!(store.get(&id).await.unwrap().is_none());
        assert!(!store.delete(&id).await.unwrap());
    }

    #[tokio::test]
    async fn test_get_nonexistent() {
        let store = InMemoryNotifierStore::new();
        assert!(store.get("nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_default_trait() {
        let store = InMemoryNotifierStore::default();
        let all = store.list().await.unwrap();
        assert!(all.is_empty());
    }

    #[tokio::test]
    async fn test_update_existing() {
        let store = InMemoryNotifierStore::new();
        let mut n = make_notifier("updatable", NotifierKind::Slack);
        let id = n.id.clone();
        store.store(&n).await.unwrap();

        n.enabled = false;
        n.name = "updated".to_string();
        store.store(&n).await.unwrap();

        let retrieved = store.get(&id).await.unwrap().expect("notifier must exist");
        assert_eq!(retrieved.name, "updated");
        assert!(!retrieved.enabled);
    }

    // =========================================================================
    // ZqlNotifierStore tests
    // =========================================================================

    #[tokio::test]
    async fn test_zql_store_and_get() {
        let store = ZqlNotifierStore::in_memory().await.unwrap();
        let n = make_notifier("slack-alerts", NotifierKind::Slack);
        let id = n.id.clone();

        store.store(&n).await.unwrap();

        let retrieved = store.get(&id).await.unwrap().expect("notifier must exist");
        assert_eq!(retrieved.name, "slack-alerts");
        assert_eq!(retrieved.kind, NotifierKind::Slack);
        assert!(retrieved.enabled);
        match retrieved.config {
            NotifierConfig::Slack { webhook_url } => {
                assert_eq!(webhook_url, "https://hooks.slack.com/test");
            }
            other => panic!("expected Slack config, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_zql_list_sorted_by_name() {
        let store = ZqlNotifierStore::in_memory().await.unwrap();
        store
            .store(&make_notifier("beta", NotifierKind::Discord))
            .await
            .unwrap();
        store
            .store(&make_notifier("alpha", NotifierKind::Slack))
            .await
            .unwrap();
        store
            .store(&make_notifier("gamma", NotifierKind::Webhook))
            .await
            .unwrap();

        let all = store.list().await.unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].name, "alpha");
        assert_eq!(all[1].name, "beta");
        assert_eq!(all[2].name, "gamma");
    }

    #[tokio::test]
    async fn test_zql_delete_existing_and_missing() {
        let store = ZqlNotifierStore::in_memory().await.unwrap();
        let n = make_notifier("doomed", NotifierKind::Slack);
        let id = n.id.clone();
        store.store(&n).await.unwrap();

        assert!(store.delete(&id).await.unwrap());
        assert!(store.get(&id).await.unwrap().is_none());
        // Deleting a row that never existed (or was already removed) must
        // return Ok(false), not an error.
        assert!(!store.delete(&id).await.unwrap());
        assert!(!store.delete("nonexistent-id").await.unwrap());
    }

    #[tokio::test]
    async fn test_zql_get_nonexistent() {
        let store = ZqlNotifierStore::in_memory().await.unwrap();
        assert!(store.get("nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_zql_upsert_same_id() {
        // Storing the same id twice must succeed as an upsert — the second
        // call updates the existing row rather than raising a conflict.
        let store = ZqlNotifierStore::in_memory().await.unwrap();
        let mut n = make_notifier("upsertable", NotifierKind::Slack);
        let id = n.id.clone();
        store.store(&n).await.unwrap();

        n.enabled = false;
        n.name = "upsertable-updated".to_string();
        store.store(&n).await.unwrap();

        let retrieved = store.get(&id).await.unwrap().expect("notifier must exist");
        assert_eq!(retrieved.name, "upsertable-updated");
        assert!(!retrieved.enabled);

        // Only one row — the upsert didn't spawn a duplicate.
        let all = store.list().await.unwrap();
        assert_eq!(all.len(), 1);
    }

    #[tokio::test]
    async fn test_zql_persistent_storage() {
        // Survives closing + reopening the database — the whole point of the
        // persistent backend.
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("notifiers_zql_db");

        let n = make_notifier("persist-me", NotifierKind::Webhook);
        let id = n.id.clone();

        {
            let store = ZqlNotifierStore::open(&db_path).await.unwrap();
            store.store(&n).await.unwrap();
        }

        {
            let store = ZqlNotifierStore::open(&db_path).await.unwrap();
            let retrieved = store.get(&id).await.unwrap().expect("must persist");
            assert_eq!(retrieved.name, "persist-me");
            assert_eq!(retrieved.kind, NotifierKind::Webhook);
        }
    }
}
