//! Standalone-container storage trait and backends.
//!
//! Provides a backend-agnostic abstraction for persisting metadata about
//! "standalone" containers — those created via the raw container endpoints
//! (see [`crate::handlers::containers`]) rather than through a deployment
//! spec.
//!
//! Two implementations are shipped:
//!
//! - [`InMemoryStandaloneContainerStorage`] — a `RwLock<HashMap>`-backed
//!   store useful for tests and for daemons running with persistent state
//!   disabled.
//! - [`SqliteStandaloneContainerStorage`] — a `sqlx`-backed `SQLite` store
//!   that persists records across daemon restarts. Mirrors the inline
//!   `CREATE TABLE IF NOT EXISTS` pattern used by the deployment store
//!   (`storage::deployments`).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use tokio::sync::RwLock;
use zlayer_agent::runtime::ContainerId;

use super::StorageError;
use crate::handlers::containers::StandaloneContainer;
#[cfg(test)]
use zlayer_paths::ZLayerDirs;

/// Trait for standalone-container storage backends.
///
/// The unique key for a [`StandaloneContainer`] is the stringified form of
/// its [`container_id`](StandaloneContainer::container_id) field
/// (i.e. `ContainerId::to_string()`, formatted as `{service}-rep-{replica}`).
#[async_trait]
pub trait StandaloneContainerStorage: Send + Sync {
    /// Insert a new standalone container record.
    async fn insert(&self, container: StandaloneContainer) -> Result<(), StorageError>;

    /// Fetch a standalone container by its stringified container id.
    async fn get(&self, id: &str) -> Result<Option<StandaloneContainer>, StorageError>;

    /// List all standalone container records.
    async fn list(&self) -> Result<Vec<StandaloneContainer>, StorageError>;

    /// Delete a standalone container by its stringified container id.
    ///
    /// Returns `true` if a record existed and was removed, `false` otherwise.
    async fn delete(&self, id: &str) -> Result<bool, StorageError>;

    /// Update an existing standalone container record (overwrites by key).
    async fn update(&self, container: StandaloneContainer) -> Result<(), StorageError>;
}

/// In-memory standalone-container storage backed by an `RwLock<HashMap>`.
///
/// Useful for tests and for hosts that disable persistent state.
pub struct InMemoryStandaloneContainerStorage {
    inner: Arc<RwLock<HashMap<String, StandaloneContainer>>>,
}

impl InMemoryStandaloneContainerStorage {
    /// Create a new empty in-memory standalone-container store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Default for InMemoryStandaloneContainerStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl StandaloneContainerStorage for InMemoryStandaloneContainerStorage {
    async fn insert(&self, container: StandaloneContainer) -> Result<(), StorageError> {
        let key = container.container_id.to_string();
        let mut map = self.inner.write().await;
        map.insert(key, container);
        Ok(())
    }

    async fn get(&self, id: &str) -> Result<Option<StandaloneContainer>, StorageError> {
        let map = self.inner.read().await;
        Ok(map.get(id).cloned())
    }

    async fn list(&self) -> Result<Vec<StandaloneContainer>, StorageError> {
        let map = self.inner.read().await;
        Ok(map.values().cloned().collect())
    }

    async fn delete(&self, id: &str) -> Result<bool, StorageError> {
        let mut map = self.inner.write().await;
        Ok(map.remove(id).is_some())
    }

    async fn update(&self, container: StandaloneContainer) -> Result<(), StorageError> {
        let key = container.container_id.to_string();
        let mut map = self.inner.write().await;
        map.insert(key, container);
        Ok(())
    }
}

// =============================================================================
// SQLite (sqlx) backend
// =============================================================================

/// `SQLite`/`sqlx` standalone-container storage.
///
/// Records are keyed by the stringified [`ContainerId`]
/// (`{service}-rep-{replica}`, see [`StandaloneContainer::container_id`]) and
/// laid out across explicit columns rather than a single blob, so the
/// `service` and `replica` halves remain queryable without parsing the id
/// string. The `labels` map is JSON-encoded into a single `TEXT` column;
/// nothing else lives in JSON form, so changes to [`StandaloneContainer`]'s
/// scalar fields will surface as compile errors rather than silent shape
/// drift.
///
/// Schema:
///
/// ```sql
/// CREATE TABLE IF NOT EXISTS standalone_containers (
///     id              TEXT PRIMARY KEY NOT NULL,
///     service         TEXT NOT NULL,
///     replica         INTEGER NOT NULL,
///     image           TEXT NOT NULL,
///     name            TEXT,
///     labels_json     TEXT NOT NULL,
///     created_at      TEXT NOT NULL,
///     delete_on_exit  INTEGER NOT NULL DEFAULT 0
/// )
/// ```
///
/// The `delete_on_exit` column mirrors
/// [`crate::handlers::containers::StandaloneContainer::delete_on_exit`] (Docker
/// `--rm` / `HostConfig.AutoRemove`); old rows from before the column existed
/// load as `0` (retain on exit) thanks to the `ALTER TABLE` shim in
/// [`Self::init_schema`].
pub struct SqliteStandaloneContainerStorage {
    pool: SqlitePool,
}

impl SqliteStandaloneContainerStorage {
    /// Open or create a `SQLite` database at the given path and ensure the
    /// `standalone_containers` table exists.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the database cannot be opened or the
    /// schema cannot be created.
    pub async fn open<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        let path_str = path.as_ref().display().to_string();
        let connection_string = format!("sqlite:{path_str}?mode=rwc");

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&connection_string)
            .await?;

        // Match the durability/concurrency settings used by `SqlxStorage` so
        // multiple stores running against the same database file behave
        // consistently.
        sqlx::query("PRAGMA journal_mode=WAL")
            .execute(&pool)
            .await?;
        sqlx::query("PRAGMA busy_timeout=5000")
            .execute(&pool)
            .await?;

        let this = Self { pool };
        this.init_schema().await?;
        Ok(this)
    }

    /// Create an in-memory `SQLite` database (useful for tests).
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the in-memory database cannot be created
    /// or the schema cannot be initialised.
    pub async fn in_memory() -> Result<Self, StorageError> {
        let pool = SqlitePool::connect(":memory:").await?;
        let this = Self { pool };
        this.init_schema().await?;
        Ok(this)
    }

    async fn init_schema(&self) -> Result<(), StorageError> {
        sqlx::query(
            r"
            CREATE TABLE IF NOT EXISTS standalone_containers (
                id              TEXT PRIMARY KEY NOT NULL,
                service         TEXT NOT NULL,
                replica         INTEGER NOT NULL,
                image           TEXT NOT NULL,
                name            TEXT,
                labels_json     TEXT NOT NULL,
                created_at      TEXT NOT NULL,
                delete_on_exit  INTEGER NOT NULL DEFAULT 0
            )
            ",
        )
        .execute(&self.pool)
        .await?;

        // Backfill the `delete_on_exit` column on databases that pre-date it:
        // SQLite's `ADD COLUMN` is a no-op error if the column already exists,
        // so we swallow the duplicate-column failure and surface anything else
        // as a real error. New deployments hit `CREATE TABLE` above and skip
        // this branch entirely.
        let alter = sqlx::query(
            "ALTER TABLE standalone_containers ADD COLUMN delete_on_exit INTEGER NOT NULL DEFAULT 0",
        )
        .execute(&self.pool)
        .await;
        match alter {
            Ok(_) => {}
            Err(sqlx::Error::Database(db_err))
                if db_err.message().contains("duplicate column name") =>
            {
                // Column already present — older daemon already migrated.
            }
            Err(e) => return Err(StorageError::from(e)),
        }
        Ok(())
    }

    /// Encode a `StandaloneContainer`'s `labels` map to JSON.
    fn encode_labels(labels: &HashMap<String, String>) -> Result<String, StorageError> {
        serde_json::to_string(labels).map_err(StorageError::from)
    }

    /// Decode the stored `labels_json` column back into a `HashMap`.
    fn decode_labels(json: &str) -> Result<HashMap<String, String>, StorageError> {
        serde_json::from_str(json).map_err(StorageError::from)
    }

    /// Reconstruct a `StandaloneContainer` from its row tuple.
    fn row_to_container(
        service: String,
        replica: i64,
        image: String,
        name: Option<String>,
        labels_json: &str,
        created_at: String,
        delete_on_exit: i64,
    ) -> Result<StandaloneContainer, StorageError> {
        let replica_u32 = u32::try_from(replica).map_err(|_| {
            StorageError::Other(format!(
                "replica {replica} stored in DB does not fit in u32"
            ))
        })?;
        let labels = Self::decode_labels(labels_json)?;
        Ok(StandaloneContainer {
            container_id: ContainerId::new(service, replica_u32),
            image,
            name,
            labels,
            created_at,
            delete_on_exit: delete_on_exit != 0,
        })
    }

    /// Upsert primitive shared by `insert` and `update`. The trait
    /// distinguishes the two operations semantically, but `SQLite`'s
    /// `INSERT OR REPLACE` collapses both onto the same physical write —
    /// matching the `RwLock<HashMap>::insert(...)` semantics of the
    /// in-memory backend.
    async fn upsert(&self, container: &StandaloneContainer) -> Result<(), StorageError> {
        let id = container.container_id.to_string();
        let replica = i64::from(container.container_id.replica);
        let labels_json = Self::encode_labels(&container.labels)?;
        let delete_on_exit = i64::from(container.delete_on_exit);

        sqlx::query(
            r"
            INSERT OR REPLACE INTO standalone_containers
                (id, service, replica, image, name, labels_json, created_at, delete_on_exit)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            ",
        )
        .bind(&id)
        .bind(&container.container_id.service)
        .bind(replica)
        .bind(&container.image)
        .bind(&container.name)
        .bind(&labels_json)
        .bind(&container.created_at)
        .bind(delete_on_exit)
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}

#[async_trait]
impl StandaloneContainerStorage for SqliteStandaloneContainerStorage {
    async fn insert(&self, container: StandaloneContainer) -> Result<(), StorageError> {
        self.upsert(&container).await
    }

    async fn get(&self, id: &str) -> Result<Option<StandaloneContainer>, StorageError> {
        let row: Option<(String, i64, String, Option<String>, String, String, i64)> =
            sqlx::query_as(
                r"
            SELECT service, replica, image, name, labels_json, created_at, delete_on_exit
            FROM standalone_containers
            WHERE id = ?
            ",
            )
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;

        match row {
            Some((service, replica, image, name, labels_json, created_at, delete_on_exit)) => {
                Ok(Some(Self::row_to_container(
                    service,
                    replica,
                    image,
                    name,
                    &labels_json,
                    created_at,
                    delete_on_exit,
                )?))
            }
            None => Ok(None),
        }
    }

    async fn list(&self) -> Result<Vec<StandaloneContainer>, StorageError> {
        let rows: Vec<(String, i64, String, Option<String>, String, String, i64)> = sqlx::query_as(
            r"
            SELECT service, replica, image, name, labels_json, created_at, delete_on_exit
            FROM standalone_containers
            ORDER BY id ASC
            ",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut out = Vec::with_capacity(rows.len());
        for (service, replica, image, name, labels_json, created_at, delete_on_exit) in rows {
            out.push(Self::row_to_container(
                service,
                replica,
                image,
                name,
                &labels_json,
                created_at,
                delete_on_exit,
            )?);
        }
        Ok(out)
    }

    async fn delete(&self, id: &str) -> Result<bool, StorageError> {
        let result = sqlx::query("DELETE FROM standalone_containers WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn update(&self, container: StandaloneContainer) -> Result<(), StorageError> {
        self.upsert(&container).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_container(service: &str, replica: u32) -> StandaloneContainer {
        StandaloneContainer {
            container_id: ContainerId::new(service.to_string(), replica),
            image: format!("registry.example.com/{service}:latest"),
            name: Some(format!("{service}-{replica}")),
            labels: HashMap::new(),
            created_at: "2026-05-03T00:00:00Z".to_string(),
            delete_on_exit: false,
        }
    }

    #[tokio::test]
    async fn insert_then_get_round_trip() {
        let store = InMemoryStandaloneContainerStorage::new();
        let container = make_container("svc-a", 0);
        let key = container.container_id.to_string();

        store.insert(container.clone()).await.unwrap();

        let got = store.get(&key).await.unwrap().expect("container present");
        assert_eq!(got.container_id, container.container_id);
        assert_eq!(got.image, container.image);
        assert_eq!(got.name, container.name);
        assert_eq!(got.created_at, container.created_at);
    }

    #[tokio::test]
    async fn get_missing_returns_none() {
        let store = InMemoryStandaloneContainerStorage::new();
        let result = store.get("no-such-thing-rep-0").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn delete_existing_returns_true() {
        let store = InMemoryStandaloneContainerStorage::new();
        let container = make_container("svc-b", 1);
        let key = container.container_id.to_string();

        store.insert(container).await.unwrap();
        let removed = store.delete(&key).await.unwrap();
        assert!(removed);

        let after = store.get(&key).await.unwrap();
        assert!(after.is_none());
    }

    #[tokio::test]
    async fn delete_missing_returns_false() {
        let store = InMemoryStandaloneContainerStorage::new();
        let removed = store.delete("ghost-rep-0").await.unwrap();
        assert!(!removed);
    }

    #[tokio::test]
    async fn list_includes_all_inserted() {
        let store = InMemoryStandaloneContainerStorage::new();
        let a = make_container("svc-a", 0);
        let b = make_container("svc-b", 0);
        let c = make_container("svc-c", 0);

        store.insert(a.clone()).await.unwrap();
        store.insert(b.clone()).await.unwrap();
        store.insert(c.clone()).await.unwrap();

        let listed = store.list().await.unwrap();
        assert_eq!(listed.len(), 3);

        let keys: Vec<String> = listed.iter().map(|c| c.container_id.to_string()).collect();
        assert!(keys.contains(&a.container_id.to_string()));
        assert!(keys.contains(&b.container_id.to_string()));
        assert!(keys.contains(&c.container_id.to_string()));
    }

    #[tokio::test]
    async fn update_replaces_existing() {
        let store = InMemoryStandaloneContainerStorage::new();
        let mut container = make_container("svc-a", 0);
        let key = container.container_id.to_string();

        store.insert(container.clone()).await.unwrap();

        // Mutate a non-key field and update.
        container.image = "registry.example.com/svc-a:v2".to_string();
        container.name = Some("svc-a-renamed".to_string());
        store.update(container.clone()).await.unwrap();

        let got = store.get(&key).await.unwrap().expect("container present");
        assert_eq!(got.image, "registry.example.com/svc-a:v2");
        assert_eq!(got.name, Some("svc-a-renamed".to_string()));

        // Still exactly one record under that key.
        let all = store.list().await.unwrap();
        assert_eq!(all.len(), 1);
    }

    #[tokio::test]
    async fn concurrent_inserts_consistent() {
        const N: u32 = 64;

        let store = Arc::new(InMemoryStandaloneContainerStorage::new());

        let mut handles = Vec::with_capacity(N as usize);
        for i in 0..N {
            let store = Arc::clone(&store);
            handles.push(tokio::spawn(async move {
                let container = StandaloneContainer {
                    container_id: ContainerId::new(format!("svc-{i}"), i),
                    image: "img:latest".to_string(),
                    name: None,
                    labels: HashMap::new(),
                    created_at: "2026-05-03T00:00:00Z".to_string(),
                    delete_on_exit: false,
                };
                store.insert(container).await.unwrap();
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        let all = store.list().await.unwrap();
        assert_eq!(all.len(), N as usize);
    }

    // =========================================================================
    // SqliteStandaloneContainerStorage tests
    //
    // Mirror the in-memory coverage: insert+get round-trip, list ordering,
    // delete (existing + missing), update, and a labels round-trip to prove
    // the JSON column survives the encode/decode cycle.
    // =========================================================================

    #[tokio::test]
    async fn sqlite_insert_then_get_round_trip() {
        let store = SqliteStandaloneContainerStorage::in_memory().await.unwrap();
        let mut container = make_container("svc-a", 0);
        // Non-empty labels to exercise the JSON column.
        container
            .labels
            .insert("zlayer.kind".to_string(), "standalone".to_string());
        container
            .labels
            .insert("env".to_string(), "production".to_string());
        let key = container.container_id.to_string();

        store.insert(container.clone()).await.unwrap();

        let got = store.get(&key).await.unwrap().expect("container present");
        assert_eq!(got.container_id, container.container_id);
        assert_eq!(got.image, container.image);
        assert_eq!(got.name, container.name);
        assert_eq!(got.created_at, container.created_at);
        assert_eq!(got.labels, container.labels);
    }

    #[tokio::test]
    async fn sqlite_get_missing_returns_none() {
        let store = SqliteStandaloneContainerStorage::in_memory().await.unwrap();
        let result = store.get("no-such-thing-rep-0").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn sqlite_list_orders_by_id_ascending() {
        let store = SqliteStandaloneContainerStorage::in_memory().await.unwrap();

        // Insert out of order; SqliteStandaloneContainerStorage::list orders
        // by `id ASC`, which for the stringified container_id format
        // (`{service}-rep-{replica}`) sorts lexicographically.
        store.insert(make_container("svc-c", 0)).await.unwrap();
        store.insert(make_container("svc-a", 0)).await.unwrap();
        store.insert(make_container("svc-b", 0)).await.unwrap();

        let listed = store.list().await.unwrap();
        let ids: Vec<String> = listed.iter().map(|c| c.container_id.to_string()).collect();
        assert_eq!(
            ids,
            vec![
                "svc-a-rep-0".to_string(),
                "svc-b-rep-0".to_string(),
                "svc-c-rep-0".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn sqlite_delete_existing_then_missing() {
        let store = SqliteStandaloneContainerStorage::in_memory().await.unwrap();
        let container = make_container("svc-d", 1);
        let key = container.container_id.to_string();

        store.insert(container).await.unwrap();
        let removed = store.delete(&key).await.unwrap();
        assert!(removed, "first delete must remove the row");

        // Second delete must return false — proves we are inspecting actual
        // rows_affected and not always reporting success.
        let removed_again = store.delete(&key).await.unwrap();
        assert!(!removed_again, "second delete must report no row");

        let after = store.get(&key).await.unwrap();
        assert!(after.is_none());
    }

    #[tokio::test]
    async fn sqlite_update_replaces_existing_row() {
        let store = SqliteStandaloneContainerStorage::in_memory().await.unwrap();
        let mut container = make_container("svc-e", 0);
        let key = container.container_id.to_string();

        store.insert(container.clone()).await.unwrap();

        container.image = "registry.example.com/svc-e:v2".to_string();
        container.name = Some("svc-e-renamed".to_string());
        container
            .labels
            .insert("rolled".to_string(), "yes".to_string());
        store.update(container.clone()).await.unwrap();

        let got = store.get(&key).await.unwrap().expect("container present");
        assert_eq!(got.image, "registry.example.com/svc-e:v2");
        assert_eq!(got.name, Some("svc-e-renamed".to_string()));
        assert_eq!(got.labels.get("rolled"), Some(&"yes".to_string()));

        // Update must overwrite, not duplicate.
        let all = store.list().await.unwrap();
        assert_eq!(all.len(), 1);
    }

    #[tokio::test]
    async fn sqlite_persists_across_reopen() {
        let temp_dir = ZLayerDirs::system_default()
            .scratch_dir("sqlite-persists-across-reopen-")
            .unwrap();
        let db_path = temp_dir.path().join("standalone-containers.db");

        // Write through one handle...
        {
            let store = SqliteStandaloneContainerStorage::open(&db_path)
                .await
                .unwrap();
            let mut container = make_container("svc-persist", 7);
            container
                .labels
                .insert("persist".to_string(), "true".to_string());
            store.insert(container).await.unwrap();
        }

        // ...and read through a fresh handle. Proves the schema and rows
        // survive a process-style restart, not just an in-memory cache.
        {
            let store = SqliteStandaloneContainerStorage::open(&db_path)
                .await
                .unwrap();
            let got = store
                .get("svc-persist-rep-7")
                .await
                .unwrap()
                .expect("container present after reopen");
            assert_eq!(got.container_id.service, "svc-persist");
            assert_eq!(got.container_id.replica, 7);
            assert_eq!(got.labels.get("persist"), Some(&"true".to_string()));
        }
    }

    /// `delete_on_exit` round-trips through the `SQLite` column. Pinning this
    /// because the auto-remove subscriber consults the cached flag on every
    /// `container.die`; a silent storage truncation would make `--rm`
    /// containers leak across daemon restarts.
    #[tokio::test]
    async fn sqlite_delete_on_exit_round_trips() {
        let store = SqliteStandaloneContainerStorage::in_memory().await.unwrap();
        let mut container = make_container("svc-rm", 0);
        container.delete_on_exit = true;
        let key = container.container_id.to_string();

        store.insert(container).await.unwrap();
        let got = store.get(&key).await.unwrap().expect("container present");
        assert!(
            got.delete_on_exit,
            "delete_on_exit=true must survive insert+get"
        );

        // And the false case must round-trip too — guards against a default
        // that silently flips the flag on the read path.
        let mut container2 = make_container("svc-keep", 0);
        container2.delete_on_exit = false;
        let key2 = container2.container_id.to_string();
        store.insert(container2).await.unwrap();
        let got2 = store.get(&key2).await.unwrap().expect("container present");
        assert!(!got2.delete_on_exit);
    }
}
