//! User account storage implementations
//!
//! Provides both persistent (`SQLite` via sqlx) and in-memory storage backends
//! for user accounts. Password hashes are NOT stored here — they live in
//! `zlayer-secrets::CredentialStore`, keyed by email.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use tokio::sync::RwLock;

use super::{StorageError, StoredUser};

/// Trait for user storage backends
#[async_trait]
pub trait UserStorage: Send + Sync {
    /// Store (create or update) a user by id.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Database`] if the backing store rejects the write,
    /// including when the email uniqueness constraint is violated.
    async fn store(&self, user: &StoredUser) -> Result<(), StorageError>;

    /// Fetch a user by id.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the backing store fails or the record cannot
    /// be deserialized.
    async fn get(&self, id: &str) -> Result<Option<StoredUser>, StorageError>;

    /// Fetch a user by email. Matching is case-insensitive.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the backing store fails or the record cannot
    /// be deserialized.
    async fn get_by_email(&self, email: &str) -> Result<Option<StoredUser>, StorageError>;

    /// List all users sorted by email (case-insensitive ascending).
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the backing store fails or any record cannot
    /// be deserialized.
    async fn list(&self) -> Result<Vec<StoredUser>, StorageError>;

    /// Delete a user by id. Returns `true` if the row existed.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Database`] if the delete operation fails.
    async fn delete(&self, id: &str) -> Result<bool, StorageError>;

    /// Total user count. Used for the first-run / bootstrap check.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Database`] if the count query fails.
    async fn count(&self) -> Result<u64, StorageError>;
}

/// SQLite-based persistent storage for user accounts using sqlx
pub struct SqlxUserStore {
    pool: SqlitePool,
}

impl SqlxUserStore {
    /// Open or create a `SQLite` database at the given path.
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

        // Enable WAL mode for better concurrent access
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
    /// Returns an error if the in-memory database creation or table creation fails.
    pub async fn in_memory() -> Result<Self, StorageError> {
        let pool = SqlitePool::connect(":memory:").await?;
        Self::init_schema(&pool).await?;
        Ok(Self { pool })
    }

    /// Create the `users` table and supporting index if they do not already exist.
    async fn init_schema(pool: &SqlitePool) -> Result<(), StorageError> {
        sqlx::query(
            r"
            CREATE TABLE IF NOT EXISTS users (
                id TEXT PRIMARY KEY NOT NULL,
                email TEXT NOT NULL UNIQUE COLLATE NOCASE,
                data_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            ",
        )
        .execute(pool)
        .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_users_email ON users(email)")
            .execute(pool)
            .await?;

        Ok(())
    }
}

#[async_trait]
impl UserStorage for SqlxUserStore {
    async fn store(&self, user: &StoredUser) -> Result<(), StorageError> {
        let data_json = serde_json::to_string(user)?;
        let email = user.email.to_lowercase();
        let created_at = user.created_at.to_rfc3339();
        let updated_at = user.updated_at.to_rfc3339();

        // Upsert by id (the primary key). ON CONFLICT(id) allows a user to
        // update their own email, while the separate UNIQUE constraint on
        // email still rejects a different id trying to claim an email that
        // is already in use.
        sqlx::query(
            r"
            INSERT INTO users (id, email, data_json, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
                email = excluded.email,
                data_json = excluded.data_json,
                created_at = excluded.created_at,
                updated_at = excluded.updated_at
            ",
        )
        .bind(&user.id)
        .bind(&email)
        .bind(&data_json)
        .bind(&created_at)
        .bind(&updated_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn get(&self, id: &str) -> Result<Option<StoredUser>, StorageError> {
        let row: Option<(String,)> = sqlx::query_as("SELECT data_json FROM users WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;

        match row {
            Some((data_json,)) => {
                let user: StoredUser = serde_json::from_str(&data_json)?;
                Ok(Some(user))
            }
            None => Ok(None),
        }
    }

    async fn get_by_email(&self, email: &str) -> Result<Option<StoredUser>, StorageError> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT data_json FROM users WHERE email = LOWER(?) COLLATE NOCASE")
                .bind(email)
                .fetch_optional(&self.pool)
                .await?;

        match row {
            Some((data_json,)) => {
                let user: StoredUser = serde_json::from_str(&data_json)?;
                Ok(Some(user))
            }
            None => Ok(None),
        }
    }

    async fn list(&self) -> Result<Vec<StoredUser>, StorageError> {
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT data_json FROM users ORDER BY email COLLATE NOCASE ASC")
                .fetch_all(&self.pool)
                .await?;

        let mut users = Vec::with_capacity(rows.len());
        for (data_json,) in rows {
            let user: StoredUser = serde_json::from_str(&data_json)?;
            users.push(user);
        }

        Ok(users)
    }

    async fn delete(&self, id: &str) -> Result<bool, StorageError> {
        let result = sqlx::query("DELETE FROM users WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    async fn count(&self) -> Result<u64, StorageError> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
            .fetch_one(&self.pool)
            .await?;

        Ok(u64::try_from(row.0).unwrap_or(0))
    }
}

/// In-memory user store for tests
pub struct InMemoryUserStore {
    users: Arc<RwLock<HashMap<String, StoredUser>>>,
}

impl InMemoryUserStore {
    /// Create a new empty in-memory user store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            users: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Default for InMemoryUserStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl UserStorage for InMemoryUserStore {
    async fn store(&self, user: &StoredUser) -> Result<(), StorageError> {
        let mut users = self.users.write().await;

        // Enforce UNIQUE email constraint: reject if another id already owns
        // this email (case-insensitive).
        let normalized = user.email.to_lowercase();
        if let Some(conflict) = users
            .values()
            .find(|existing| existing.id != user.id && existing.email.to_lowercase() == normalized)
        {
            return Err(StorageError::Database(format!(
                "UNIQUE constraint failed: users.email (email '{}' already in use by user {})",
                user.email, conflict.id
            )));
        }

        let mut to_store = user.clone();
        to_store.email = normalized;
        users.insert(user.id.clone(), to_store);
        Ok(())
    }

    async fn get(&self, id: &str) -> Result<Option<StoredUser>, StorageError> {
        let users = self.users.read().await;
        Ok(users.get(id).cloned())
    }

    async fn get_by_email(&self, email: &str) -> Result<Option<StoredUser>, StorageError> {
        let needle = email.to_lowercase();
        let users = self.users.read().await;
        Ok(users
            .values()
            .find(|user| user.email.to_lowercase() == needle)
            .cloned())
    }

    async fn list(&self) -> Result<Vec<StoredUser>, StorageError> {
        let users = self.users.read().await;
        let mut list: Vec<_> = users.values().cloned().collect();
        list.sort_by(|a, b| a.email.to_lowercase().cmp(&b.email.to_lowercase()));
        Ok(list)
    }

    async fn delete(&self, id: &str) -> Result<bool, StorageError> {
        let mut users = self.users.write().await;
        Ok(users.remove(id).is_some())
    }

    async fn count(&self) -> Result<u64, StorageError> {
        let users = self.users.read().await;
        Ok(users.len() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::UserRole;

    fn make_user(email: &str, display_name: &str) -> StoredUser {
        StoredUser::new(email, display_name, UserRole::User)
    }

    // =========================================================================
    // InMemoryUserStore tests
    // =========================================================================

    #[tokio::test]
    async fn test_inmemory_store_and_get() {
        let store = InMemoryUserStore::new();
        let user = make_user("alice@example.com", "Alice");
        let id = user.id.clone();

        store.store(&user).await.unwrap();

        let retrieved = store.get(&id).await.unwrap().expect("user must exist");
        assert_eq!(retrieved.email, "alice@example.com");
        assert_eq!(retrieved.display_name, "Alice");
        assert_eq!(retrieved.role, UserRole::User);
        assert!(retrieved.is_active);
    }

    #[tokio::test]
    async fn test_inmemory_get_by_email_case_insensitive() {
        let store = InMemoryUserStore::new();
        let user = StoredUser::new("Foo@Bar.com", "Foo", UserRole::Admin);
        store.store(&user).await.unwrap();

        let retrieved = store
            .get_by_email("foo@bar.com")
            .await
            .unwrap()
            .expect("user must exist");
        assert_eq!(retrieved.email, "foo@bar.com");
        assert_eq!(retrieved.role, UserRole::Admin);
    }

    #[tokio::test]
    async fn test_inmemory_get_by_email_nonexistent() {
        let store = InMemoryUserStore::new();
        let missing = store.get_by_email("missing@example.com").await.unwrap();
        assert!(missing.is_none());
    }

    #[tokio::test]
    async fn test_inmemory_list_sorted_by_email() {
        let store = InMemoryUserStore::new();
        store.store(&make_user("c@x.com", "C")).await.unwrap();
        store.store(&make_user("a@x.com", "A")).await.unwrap();
        store.store(&make_user("b@x.com", "B")).await.unwrap();

        let list = store.list().await.unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].email, "a@x.com");
        assert_eq!(list[1].email, "b@x.com");
        assert_eq!(list[2].email, "c@x.com");
    }

    #[tokio::test]
    async fn test_inmemory_delete() {
        let store = InMemoryUserStore::new();
        let user = make_user("del@example.com", "Del");
        let id = user.id.clone();
        store.store(&user).await.unwrap();

        let deleted = store.delete(&id).await.unwrap();
        assert!(deleted);
        assert!(store.get(&id).await.unwrap().is_none());

        let deleted_again = store.delete(&id).await.unwrap();
        assert!(!deleted_again);
    }

    #[tokio::test]
    async fn test_inmemory_count() {
        let store = InMemoryUserStore::new();
        assert_eq!(store.count().await.unwrap(), 0);

        store.store(&make_user("a@x.com", "A")).await.unwrap();
        store.store(&make_user("b@x.com", "B")).await.unwrap();
        let third = make_user("c@x.com", "C");
        let third_id = third.id.clone();
        store.store(&third).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 3);

        store.delete(&third_id).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn test_inmemory_update_advances_updated_at() {
        let store = InMemoryUserStore::new();
        let mut user = make_user("upd@example.com", "Original");
        let id = user.id.clone();
        let original_updated = user.updated_at;
        store.store(&user).await.unwrap();

        std::thread::sleep(std::time::Duration::from_millis(10));

        user.display_name = "Changed".to_string();
        user.email = "changed@example.com".to_string();
        user.touch_login();
        store.store(&user).await.unwrap();

        let retrieved = store.get(&id).await.unwrap().unwrap();
        assert_eq!(retrieved.display_name, "Changed");
        assert_eq!(retrieved.email, "changed@example.com");
        assert!(retrieved.updated_at > original_updated);
        assert!(retrieved.last_login_at.is_some());
    }

    #[tokio::test]
    async fn test_inmemory_unique_email_rejects_different_id() {
        let store = InMemoryUserStore::new();
        let first = make_user("dup@example.com", "First");
        store.store(&first).await.unwrap();

        let second = make_user("dup@example.com", "Second");
        let err = store.store(&second).await.expect_err("should fail");
        match err {
            StorageError::Database(_) => {}
            other => panic!("expected Database error, got {other:?}"),
        }
    }

    // =========================================================================
    // SqlxUserStore tests
    // =========================================================================

    #[tokio::test]
    async fn test_sqlx_store_and_get() {
        let store = SqlxUserStore::in_memory().await.unwrap();
        let user = make_user("alice@example.com", "Alice");
        let id = user.id.clone();

        store.store(&user).await.unwrap();

        let retrieved = store.get(&id).await.unwrap().expect("user must exist");
        assert_eq!(retrieved.email, "alice@example.com");
        assert_eq!(retrieved.display_name, "Alice");
    }

    #[tokio::test]
    async fn test_sqlx_get_by_email_case_insensitive() {
        let store = SqlxUserStore::in_memory().await.unwrap();
        let user = StoredUser::new("Foo@Bar.com", "Foo", UserRole::Admin);
        store.store(&user).await.unwrap();

        let retrieved = store
            .get_by_email("foo@bar.com")
            .await
            .unwrap()
            .expect("user must exist");
        assert_eq!(retrieved.email, "foo@bar.com");
        assert_eq!(retrieved.role, UserRole::Admin);

        let mixed = store
            .get_by_email("FOO@bar.COM")
            .await
            .unwrap()
            .expect("user must exist");
        assert_eq!(mixed.email, "foo@bar.com");
    }

    #[tokio::test]
    async fn test_sqlx_get_by_email_nonexistent() {
        let store = SqlxUserStore::in_memory().await.unwrap();
        let missing = store.get_by_email("missing@example.com").await.unwrap();
        assert!(missing.is_none());
    }

    #[tokio::test]
    async fn test_sqlx_list_sorted_by_email() {
        let store = SqlxUserStore::in_memory().await.unwrap();
        store.store(&make_user("c@x.com", "C")).await.unwrap();
        store.store(&make_user("a@x.com", "A")).await.unwrap();
        store.store(&make_user("b@x.com", "B")).await.unwrap();

        let list = store.list().await.unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].email, "a@x.com");
        assert_eq!(list[1].email, "b@x.com");
        assert_eq!(list[2].email, "c@x.com");
    }

    #[tokio::test]
    async fn test_sqlx_delete() {
        let store = SqlxUserStore::in_memory().await.unwrap();
        let user = make_user("del@example.com", "Del");
        let id = user.id.clone();
        store.store(&user).await.unwrap();

        let deleted = store.delete(&id).await.unwrap();
        assert!(deleted);
        assert!(store.get(&id).await.unwrap().is_none());

        let deleted_again = store.delete(&id).await.unwrap();
        assert!(!deleted_again);
    }

    #[tokio::test]
    async fn test_sqlx_count() {
        let store = SqlxUserStore::in_memory().await.unwrap();
        assert_eq!(store.count().await.unwrap(), 0);

        store.store(&make_user("a@x.com", "A")).await.unwrap();
        store.store(&make_user("b@x.com", "B")).await.unwrap();
        let third = make_user("c@x.com", "C");
        let third_id = third.id.clone();
        store.store(&third).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 3);

        store.delete(&third_id).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn test_sqlx_update_by_id_advances_updated_at() {
        let store = SqlxUserStore::in_memory().await.unwrap();
        let mut user = make_user("upd@example.com", "Original");
        let id = user.id.clone();
        let original_updated = user.updated_at;
        store.store(&user).await.unwrap();

        std::thread::sleep(std::time::Duration::from_millis(10));

        user.display_name = "Changed".to_string();
        user.email = "changed@example.com".to_string();
        user.touch_login();
        store.store(&user).await.unwrap();

        let retrieved = store.get(&id).await.unwrap().unwrap();
        assert_eq!(retrieved.display_name, "Changed");
        assert_eq!(retrieved.email, "changed@example.com");
        assert!(retrieved.updated_at > original_updated);

        // Old email must no longer resolve; new email must.
        assert!(store
            .get_by_email("upd@example.com")
            .await
            .unwrap()
            .is_none());
        assert!(store
            .get_by_email("changed@example.com")
            .await
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn test_sqlx_unique_email_rejects_different_id() {
        let store = SqlxUserStore::in_memory().await.unwrap();
        let first = make_user("dup@example.com", "First");
        store.store(&first).await.unwrap();

        let second = make_user("dup@example.com", "Second");
        let err = store.store(&second).await.expect_err("should fail");
        match err {
            StorageError::Database(_) => {}
            other => panic!("expected Database error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_sqlx_persistent_storage() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("users.db");

        let user = make_user("persist@example.com", "Persist");
        let id = user.id.clone();

        // Create and populate database
        {
            let store = SqlxUserStore::open(&db_path).await.unwrap();
            store.store(&user).await.unwrap();
        }

        // Reopen and verify data persists
        {
            let store = SqlxUserStore::open(&db_path).await.unwrap();
            let retrieved = store.get(&id).await.unwrap().expect("user must persist");
            assert_eq!(retrieved.email, "persist@example.com");
            assert_eq!(retrieved.display_name, "Persist");

            let by_email = store
                .get_by_email("persist@example.com")
                .await
                .unwrap()
                .expect("email lookup must work after reopen");
            assert_eq!(by_email.id, id);
        }
    }
}
