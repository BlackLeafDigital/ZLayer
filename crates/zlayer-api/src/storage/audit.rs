//! Audit log storage implementations
//!
//! Provides both an in-memory and a persistent (`SQLite` via sqlx) storage
//! backend for recording and querying audit log entries. `SqlxAuditStore` is
//! append-only with indexed columns on `user_id`, `resource_kind`, and
//! `occurred_at` so the multi-field `list()` filter compiles to indexed range
//! scans rather than a full-table sort.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use tokio::sync::RwLock;

use super::{AuditEntry, StorageError};

/// Filter criteria for querying audit log entries.
pub struct AuditFilter {
    /// Only entries for this user.
    pub user_id: Option<String>,
    /// Only entries for this resource kind.
    pub resource_kind: Option<String>,
    /// Only entries at or after this timestamp.
    pub since: Option<DateTime<Utc>>,
    /// Only entries at or before this timestamp.
    pub until: Option<DateTime<Utc>>,
    /// Maximum number of entries to return.
    pub limit: usize,
}

impl Default for AuditFilter {
    fn default() -> Self {
        Self {
            user_id: None,
            resource_kind: None,
            since: None,
            until: None,
            limit: 100,
        }
    }
}

/// Trait for audit log storage backends.
#[async_trait]
pub trait AuditStorage: Send + Sync {
    /// Record an audit entry.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Database`] if the backing store rejects the write.
    async fn record(&self, entry: &AuditEntry) -> Result<(), StorageError>;

    /// List audit entries matching the given filter, ordered by `created_at`
    /// descending (most recent first).
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the backing store fails.
    async fn list(&self, filter: AuditFilter) -> Result<Vec<AuditEntry>, StorageError>;
}

/// In-memory audit store.
pub struct InMemoryAuditStore {
    entries: Arc<RwLock<Vec<AuditEntry>>>,
}

impl InMemoryAuditStore {
    /// Create a new empty in-memory audit store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: Arc::new(RwLock::new(Vec::new())),
        }
    }
}

impl Default for InMemoryAuditStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AuditStorage for InMemoryAuditStore {
    async fn record(&self, entry: &AuditEntry) -> Result<(), StorageError> {
        let mut entries = self.entries.write().await;
        entries.push(entry.clone());
        Ok(())
    }

    async fn list(&self, filter: AuditFilter) -> Result<Vec<AuditEntry>, StorageError> {
        let entries = self.entries.read().await;

        let mut result: Vec<_> = entries
            .iter()
            .filter(|e| {
                if let Some(ref uid) = filter.user_id {
                    if e.user_id != *uid {
                        return false;
                    }
                }
                if let Some(ref rk) = filter.resource_kind {
                    if e.resource_kind != *rk {
                        return false;
                    }
                }
                if let Some(since) = filter.since {
                    if e.created_at < since {
                        return false;
                    }
                }
                if let Some(until) = filter.until {
                    if e.created_at > until {
                        return false;
                    }
                }
                true
            })
            .cloned()
            .collect();

        // Most recent first.
        result.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        result.truncate(filter.limit);
        Ok(result)
    }
}

/// `SQLite`-backed persistent audit log store.
///
/// Append-only. Each call to [`SqlxAuditStore::record`] inserts one row and
/// [`SqlxAuditStore::list`] issues a single `SELECT` with a dynamically built
/// `WHERE` clause against indexed columns. Designed to match
/// [`InMemoryAuditStore`] semantics exactly: `since`/`until` are both inclusive
/// bounds on `created_at`, the `user_id` and `resource_kind` filters are strict
/// equality, and results are ordered by `created_at DESC` then truncated to
/// `limit`.
pub struct SqlxAuditStore {
    pool: SqlitePool,
}

impl SqlxAuditStore {
    /// Open or create a `SQLite` audit database at the given path.
    ///
    /// # Errors
    ///
    /// Returns an error if the database connection or table creation fails.
    pub async fn open<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        let path_str = path.as_ref().display().to_string();
        let connection_string = format!("sqlite:{path_str}?mode=rwc");

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&connection_string)
            .await?;

        // Enable WAL mode for better concurrent access.
        sqlx::query("PRAGMA journal_mode=WAL")
            .execute(&pool)
            .await?;
        sqlx::query("PRAGMA busy_timeout=5000")
            .execute(&pool)
            .await?;

        Self::init_schema(&pool).await?;

        Ok(Self { pool })
    }

    /// Create an in-memory `SQLite` database (useful for testing).
    ///
    /// # Errors
    ///
    /// Returns an error if the in-memory database creation or table creation
    /// fails.
    pub async fn in_memory() -> Result<Self, StorageError> {
        let pool = SqlitePool::connect(":memory:").await?;
        Self::init_schema(&pool).await?;
        Ok(Self { pool })
    }

    /// Create the `audit_log` table and supporting indexes if they do not
    /// already exist.
    async fn init_schema(pool: &SqlitePool) -> Result<(), StorageError> {
        sqlx::query(
            r"
            CREATE TABLE IF NOT EXISTS audit_log (
                id TEXT PRIMARY KEY NOT NULL,
                user_id TEXT,
                resource_kind TEXT,
                resource_id TEXT,
                action TEXT NOT NULL,
                data_json TEXT NOT NULL,
                occurred_at TEXT NOT NULL
            )
            ",
        )
        .execute(pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_audit_user ON audit_log(user_id, occurred_at DESC)",
        )
        .execute(pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_audit_resource \
             ON audit_log(resource_kind, occurred_at DESC)",
        )
        .execute(pool)
        .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_audit_time ON audit_log(occurred_at DESC)")
            .execute(pool)
            .await?;

        Ok(())
    }
}

#[async_trait]
impl AuditStorage for SqlxAuditStore {
    async fn record(&self, entry: &AuditEntry) -> Result<(), StorageError> {
        let data_json = serde_json::to_string(entry)?;
        let occurred_at = entry.created_at.to_rfc3339();

        sqlx::query(
            r"
            INSERT INTO audit_log (
                id, user_id, resource_kind, resource_id, action, data_json, occurred_at
            )
            VALUES (?, ?, ?, ?, ?, ?, ?)
            ",
        )
        .bind(&entry.id)
        .bind(&entry.user_id)
        .bind(&entry.resource_kind)
        .bind(entry.resource_id.as_ref())
        .bind(&entry.action)
        .bind(&data_json)
        .bind(&occurred_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn list(&self, filter: AuditFilter) -> Result<Vec<AuditEntry>, StorageError> {
        let mut qb: sqlx::QueryBuilder<'_, sqlx::Sqlite> =
            sqlx::QueryBuilder::new("SELECT data_json FROM audit_log WHERE 1=1");

        if let Some(uid) = &filter.user_id {
            qb.push(" AND user_id = ").push_bind(uid.clone());
        }
        if let Some(rk) = &filter.resource_kind {
            qb.push(" AND resource_kind = ").push_bind(rk.clone());
        }
        if let Some(since) = filter.since {
            qb.push(" AND occurred_at >= ")
                .push_bind(since.to_rfc3339());
        }
        if let Some(until) = filter.until {
            qb.push(" AND occurred_at <= ")
                .push_bind(until.to_rfc3339());
        }

        qb.push(" ORDER BY occurred_at DESC LIMIT ")
            .push_bind(i64::try_from(filter.limit).unwrap_or(i64::MAX));

        let rows: Vec<(String,)> = qb
            .build_query_as::<(String,)>()
            .fetch_all(&self.pool)
            .await?;

        let mut out = Vec::with_capacity(rows.len());
        for (data_json,) in rows {
            let entry: AuditEntry = serde_json::from_str(&data_json)?;
            out.push(entry);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::AuditEntry;
    use chrono::Duration;

    fn make_entry(user_id: &str, action: &str, resource_kind: &str) -> AuditEntry {
        AuditEntry::new(user_id, action, resource_kind)
    }

    // =========================================================================
    // InMemoryAuditStore tests
    // =========================================================================

    #[tokio::test]
    async fn test_record_and_list() {
        let store = InMemoryAuditStore::new();
        let e1 = make_entry("u1", "create", "deployment");
        let e2 = make_entry("u1", "update", "deployment");

        store.record(&e1).await.unwrap();
        store.record(&e2).await.unwrap();

        let all = store.list(AuditFilter::default()).await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn test_list_most_recent_first() {
        let store = InMemoryAuditStore::new();

        let mut e1 = make_entry("u1", "create", "deployment");
        e1.created_at = Utc::now() - Duration::seconds(10);
        let mut e2 = make_entry("u1", "update", "deployment");
        e2.created_at = Utc::now();

        store.record(&e1).await.unwrap();
        store.record(&e2).await.unwrap();

        let result = store.list(AuditFilter::default()).await.unwrap();
        assert!(result[0].created_at >= result[1].created_at);
        assert_eq!(result[0].action, "update");
        assert_eq!(result[1].action, "create");
    }

    #[tokio::test]
    async fn test_filter_by_user_id() {
        let store = InMemoryAuditStore::new();
        store
            .record(&make_entry("u1", "create", "deployment"))
            .await
            .unwrap();
        store
            .record(&make_entry("u2", "delete", "project"))
            .await
            .unwrap();
        store
            .record(&make_entry("u1", "update", "deployment"))
            .await
            .unwrap();

        let filter = AuditFilter {
            user_id: Some("u1".to_string()),
            ..AuditFilter::default()
        };
        let result = store.list(filter).await.unwrap();
        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|e| e.user_id == "u1"));
    }

    #[tokio::test]
    async fn test_filter_by_resource_kind() {
        let store = InMemoryAuditStore::new();
        store
            .record(&make_entry("u1", "create", "deployment"))
            .await
            .unwrap();
        store
            .record(&make_entry("u1", "create", "project"))
            .await
            .unwrap();
        store
            .record(&make_entry("u2", "delete", "deployment"))
            .await
            .unwrap();

        let filter = AuditFilter {
            resource_kind: Some("deployment".to_string()),
            ..AuditFilter::default()
        };
        let result = store.list(filter).await.unwrap();
        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|e| e.resource_kind == "deployment"));
    }

    #[tokio::test]
    async fn test_filter_by_since_and_until() {
        let store = InMemoryAuditStore::new();

        let now = Utc::now();
        let mut e_old = make_entry("u1", "create", "deployment");
        e_old.created_at = now - Duration::hours(2);
        let mut e_mid = make_entry("u1", "update", "deployment");
        e_mid.created_at = now - Duration::hours(1);
        let mut e_new = make_entry("u1", "delete", "deployment");
        e_new.created_at = now;

        store.record(&e_old).await.unwrap();
        store.record(&e_mid).await.unwrap();
        store.record(&e_new).await.unwrap();

        // Only the middle entry
        let filter = AuditFilter {
            since: Some(now - Duration::minutes(90)),
            until: Some(now - Duration::minutes(30)),
            ..AuditFilter::default()
        };
        let result = store.list(filter).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].action, "update");
    }

    #[tokio::test]
    async fn test_filter_limit() {
        let store = InMemoryAuditStore::new();
        for i in 0..10 {
            let mut e = make_entry("u1", "create", "deployment");
            e.created_at = Utc::now() + Duration::seconds(i);
            store.record(&e).await.unwrap();
        }

        let filter = AuditFilter {
            limit: 3,
            ..AuditFilter::default()
        };
        let result = store.list(filter).await.unwrap();
        assert_eq!(result.len(), 3);
    }

    #[tokio::test]
    async fn test_empty_store() {
        let store = InMemoryAuditStore::new();
        let result = store.list(AuditFilter::default()).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_combined_filters() {
        let store = InMemoryAuditStore::new();

        let now = Utc::now();

        let mut e1 = make_entry("u1", "create", "deployment");
        e1.created_at = now;
        let mut e2 = make_entry("u2", "create", "deployment");
        e2.created_at = now;
        let mut e3 = make_entry("u1", "create", "project");
        e3.created_at = now;
        let mut e4 = make_entry("u1", "create", "deployment");
        e4.created_at = now - Duration::hours(5);

        store.record(&e1).await.unwrap();
        store.record(&e2).await.unwrap();
        store.record(&e3).await.unwrap();
        store.record(&e4).await.unwrap();

        let filter = AuditFilter {
            user_id: Some("u1".to_string()),
            resource_kind: Some("deployment".to_string()),
            since: Some(now - Duration::hours(1)),
            ..AuditFilter::default()
        };
        let result = store.list(filter).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].user_id, "u1");
        assert_eq!(result[0].resource_kind, "deployment");
    }

    #[tokio::test]
    async fn test_entry_with_details() {
        let store = InMemoryAuditStore::new();
        let mut entry = make_entry("u1", "update", "secret");
        entry.resource_id = Some("secret-123".to_string());
        entry.details = Some(serde_json::json!({"field": "value"}));
        entry.ip = Some("192.168.1.1".to_string());
        entry.user_agent = Some("zlayer-cli/0.1".to_string());

        store.record(&entry).await.unwrap();

        let result = store.list(AuditFilter::default()).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].resource_id.as_deref(), Some("secret-123"));
        assert!(result[0].details.is_some());
        assert_eq!(result[0].ip.as_deref(), Some("192.168.1.1"));
        assert_eq!(result[0].user_agent.as_deref(), Some("zlayer-cli/0.1"));
    }

    // =========================================================================
    // SqlxAuditStore tests
    // =========================================================================

    #[tokio::test]
    async fn test_sqlx_record_and_list_no_filter() {
        let store = SqlxAuditStore::in_memory().await.unwrap();
        let now = Utc::now();

        // Insert 5 entries spaced 1 second apart, oldest first.
        for i in 0..5 {
            let mut e = make_entry("u1", "create", "deployment");
            e.created_at = now - Duration::seconds(5 - i);
            e.action = format!("act-{i}");
            store.record(&e).await.unwrap();
        }

        let all = store.list(AuditFilter::default()).await.unwrap();
        assert_eq!(all.len(), 5);

        // Reverse chronological — act-4 is newest.
        assert_eq!(all[0].action, "act-4");
        assert_eq!(all[1].action, "act-3");
        assert_eq!(all[2].action, "act-2");
        assert_eq!(all[3].action, "act-1");
        assert_eq!(all[4].action, "act-0");

        // Strictly non-increasing timestamps.
        for pair in all.windows(2) {
            assert!(pair[0].created_at >= pair[1].created_at);
        }
    }

    #[tokio::test]
    async fn test_sqlx_list_filtered_by_user_id() {
        let store = SqlxAuditStore::in_memory().await.unwrap();

        store
            .record(&make_entry("u1", "create", "deployment"))
            .await
            .unwrap();
        store
            .record(&make_entry("u2", "delete", "project"))
            .await
            .unwrap();
        store
            .record(&make_entry("u1", "update", "deployment"))
            .await
            .unwrap();
        store
            .record(&make_entry("u3", "create", "deployment"))
            .await
            .unwrap();

        let filter = AuditFilter {
            user_id: Some("u1".to_string()),
            ..AuditFilter::default()
        };
        let result = store.list(filter).await.unwrap();
        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|e| e.user_id == "u1"));
    }

    #[tokio::test]
    async fn test_sqlx_list_filtered_by_resource_kind() {
        let store = SqlxAuditStore::in_memory().await.unwrap();

        store
            .record(&make_entry("u1", "create", "deployment"))
            .await
            .unwrap();
        store
            .record(&make_entry("u1", "create", "project"))
            .await
            .unwrap();
        store
            .record(&make_entry("u2", "delete", "deployment"))
            .await
            .unwrap();
        store
            .record(&make_entry("u2", "delete", "project"))
            .await
            .unwrap();

        let filter = AuditFilter {
            resource_kind: Some("deployment".to_string()),
            ..AuditFilter::default()
        };
        let result = store.list(filter).await.unwrap();
        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|e| e.resource_kind == "deployment"));
    }

    #[tokio::test]
    async fn test_sqlx_list_since_until_window() {
        let store = SqlxAuditStore::in_memory().await.unwrap();
        let now = Utc::now();

        // 10 entries, 1 second apart: offsets 0..10 seconds before `now`.
        // created_at at offset i is (now - 9s + i*1s) -> range [now-9s, now].
        for i in 0..10i64 {
            let mut e = make_entry("u1", "create", "deployment");
            e.created_at = now - Duration::seconds(9 - i);
            e.action = format!("act-{i}");
            store.record(&e).await.unwrap();
        }

        // Window catches entries at offsets 4, 5, 6 (both bounds inclusive).
        let since = now - Duration::seconds(5);
        let until = now - Duration::seconds(3);
        let filter = AuditFilter {
            since: Some(since),
            until: Some(until),
            ..AuditFilter::default()
        };
        let result = store.list(filter).await.unwrap();
        assert_eq!(result.len(), 3, "window should catch exactly 3 entries");

        // All within [since, until], inclusive.
        for e in &result {
            assert!(e.created_at >= since);
            assert!(e.created_at <= until);
        }

        // Newest-first order.
        for pair in result.windows(2) {
            assert!(pair[0].created_at >= pair[1].created_at);
        }
    }

    #[tokio::test]
    async fn test_sqlx_list_limit_respected() {
        let store = SqlxAuditStore::in_memory().await.unwrap();
        let now = Utc::now();

        // Insert 20 entries, each 1 second apart. act-19 is newest.
        for i in 0..20i64 {
            let mut e = make_entry("u1", "create", "deployment");
            e.created_at = now - Duration::seconds(19 - i);
            e.action = format!("act-{i}");
            store.record(&e).await.unwrap();
        }

        let filter = AuditFilter {
            limit: 5,
            ..AuditFilter::default()
        };
        let result = store.list(filter).await.unwrap();
        assert_eq!(result.len(), 5);

        // Must be the 5 newest, in descending order: 19, 18, 17, 16, 15.
        assert_eq!(result[0].action, "act-19");
        assert_eq!(result[1].action, "act-18");
        assert_eq!(result[2].action, "act-17");
        assert_eq!(result[3].action, "act-16");
        assert_eq!(result[4].action, "act-15");
    }

    #[tokio::test]
    async fn test_sqlx_combined_filters() {
        let store = SqlxAuditStore::in_memory().await.unwrap();
        let now = Utc::now();

        // In-window, matching user & kind.
        let mut hit = make_entry("u1", "update", "deployment");
        hit.created_at = now - Duration::seconds(30);
        // In-window, wrong user.
        let mut wrong_user = make_entry("u2", "update", "deployment");
        wrong_user.created_at = now - Duration::seconds(30);
        // In-window, wrong kind.
        let mut wrong_kind = make_entry("u1", "update", "project");
        wrong_kind.created_at = now - Duration::seconds(30);
        // Matching user & kind but outside window (too old).
        let mut too_old = make_entry("u1", "create", "deployment");
        too_old.created_at = now - Duration::hours(2);
        // Matching user & kind but outside window (too new).
        let mut too_new = make_entry("u1", "delete", "deployment");
        too_new.created_at = now + Duration::hours(1);

        store.record(&hit).await.unwrap();
        store.record(&wrong_user).await.unwrap();
        store.record(&wrong_kind).await.unwrap();
        store.record(&too_old).await.unwrap();
        store.record(&too_new).await.unwrap();

        let filter = AuditFilter {
            user_id: Some("u1".to_string()),
            resource_kind: Some("deployment".to_string()),
            since: Some(now - Duration::minutes(5)),
            until: Some(now),
            ..AuditFilter::default()
        };
        let result = store.list(filter).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, hit.id);
        assert_eq!(result[0].user_id, "u1");
        assert_eq!(result[0].resource_kind, "deployment");
        assert_eq!(result[0].action, "update");
    }

    #[tokio::test]
    async fn test_sqlx_persistent_round_trip() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("audit.db");

        let mut entry = make_entry("u1", "create", "secret");
        entry.resource_id = Some("secret-42".to_string());
        entry.details = Some(serde_json::json!({"rotated": true}));
        entry.ip = Some("10.0.0.1".to_string());
        entry.user_agent = Some("zlayer-cli/0.10".to_string());
        entry.created_at = Utc::now() - Duration::seconds(1);
        let expected_id = entry.id.clone();
        let expected_created_at = entry.created_at;

        // Write one entry, drop the store.
        {
            let store = SqlxAuditStore::open(&db_path).await.unwrap();
            store.record(&entry).await.unwrap();
        }

        // Reopen and confirm the entry round-trips byte-for-byte through
        // serde.
        {
            let store = SqlxAuditStore::open(&db_path).await.unwrap();
            let all = store.list(AuditFilter::default()).await.unwrap();
            assert_eq!(all.len(), 1);
            let got = &all[0];
            assert_eq!(got.id, expected_id);
            assert_eq!(got.user_id, "u1");
            assert_eq!(got.action, "create");
            assert_eq!(got.resource_kind, "secret");
            assert_eq!(got.resource_id.as_deref(), Some("secret-42"));
            assert_eq!(got.ip.as_deref(), Some("10.0.0.1"));
            assert_eq!(got.user_agent.as_deref(), Some("zlayer-cli/0.10"));
            assert_eq!(
                got.details.as_ref().and_then(|v| v.get("rotated")),
                Some(&serde_json::json!(true))
            );
            // RFC3339 round-trip should preserve the exact instant.
            assert_eq!(got.created_at, expected_created_at);
        }
    }
}
