//! User account storage implementations
//!
//! Provides both persistent (ZQL) and in-memory storage backends for user
//! accounts. Password hashes are NOT stored here — they live in
//! `zlayer-secrets::CredentialStore`, keyed by email.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use super::{StorageError, StoredUser};

/// Name of the ZQL store holding user records (keyed by user id).
const USERS_STORE: &str = "users";

/// Name of the ZQL store holding the email -> user-id secondary index.
/// Keys are lower-cased emails; values are the owning user id.
const USERS_BY_EMAIL_STORE: &str = "users_by_email";

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

/// ZQL-based persistent storage for user accounts.
///
/// Primary records are keyed by user id in the `users` store. A parallel
/// `users_by_email` store maps lower-cased email -> user id so that the
/// email-uniqueness invariant can be enforced and `get_by_email` stays cheap.
pub struct ZqlUserStore {
    db: tokio::sync::Mutex<zql::Database>,
}

impl ZqlUserStore {
    /// Open or create a ZQL database at the given path.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the database cannot be opened or created.
    pub async fn open<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        let path = path.as_ref().to_path_buf();

        let db = tokio::task::spawn_blocking(move || zql::Database::open(&path))
            .await
            .map_err(|e| StorageError::Database(format!("spawn_blocking failed: {e}")))?
            .map_err(StorageError::from)?;

        Ok(Self {
            db: tokio::sync::Mutex::new(db),
        })
    }

    /// Create a ZQL database in a temporary directory (useful for testing).
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the temporary directory cannot be created
    /// or the database fails to open.
    #[cfg(test)]
    pub async fn in_memory() -> Result<Self, StorageError> {
        let temp_dir = tempfile::tempdir()
            .map_err(|e| StorageError::Database(format!("failed to create temp dir: {e}")))?;
        let path = temp_dir.path().join("users_zql");

        let db = tokio::task::spawn_blocking(move || {
            let _keep = temp_dir;
            zql::Database::open(&path)
        })
        .await
        .map_err(|e| StorageError::Database(format!("spawn_blocking failed: {e}")))?
        .map_err(StorageError::from)?;

        Ok(Self {
            db: tokio::sync::Mutex::new(db),
        })
    }
}

#[async_trait]
impl UserStorage for ZqlUserStore {
    async fn store(&self, user: &StoredUser) -> Result<(), StorageError> {
        let mut db = self.db.lock().await;

        let new_email = user.email.to_lowercase();

        // 1) Enforce UNIQUE email: reject if a different id already owns it.
        if let Some(existing_id) = db
            .get_typed::<String>(USERS_BY_EMAIL_STORE, &new_email)
            .map_err(StorageError::from)?
        {
            if existing_id != user.id {
                return Err(StorageError::Database(format!(
                    "UNIQUE constraint failed: users.email (email '{}' already in use by user {})",
                    user.email, existing_id
                )));
            }
        }

        // 2) If this is an update and the email changed, tear down the old
        //    email -> id mapping before writing the new one.
        if let Some(previous) = db
            .get_typed::<StoredUser>(USERS_STORE, &user.id)
            .map_err(StorageError::from)?
        {
            let previous_email = previous.email.to_lowercase();
            if previous_email != new_email {
                db.delete_typed(USERS_BY_EMAIL_STORE, &previous_email)
                    .map_err(StorageError::from)?;
            }
        }

        // 3) Normalise the persisted record and write it.
        let mut to_store = user.clone();
        to_store.email.clone_from(&new_email);

        db.put_typed(USERS_STORE, &user.id, &to_store)
            .map_err(StorageError::from)?;
        db.put_typed(USERS_BY_EMAIL_STORE, &new_email, &user.id)
            .map_err(StorageError::from)?;

        Ok(())
    }

    async fn get(&self, id: &str) -> Result<Option<StoredUser>, StorageError> {
        let mut db = self.db.lock().await;
        db.get_typed(USERS_STORE, id).map_err(StorageError::from)
    }

    async fn get_by_email(&self, email: &str) -> Result<Option<StoredUser>, StorageError> {
        let needle = email.to_lowercase();
        let mut db = self.db.lock().await;
        let Some(user_id) = db
            .get_typed::<String>(USERS_BY_EMAIL_STORE, &needle)
            .map_err(StorageError::from)?
        else {
            return Ok(None);
        };

        db.get_typed(USERS_STORE, &user_id)
            .map_err(StorageError::from)
    }

    async fn list(&self) -> Result<Vec<StoredUser>, StorageError> {
        let mut db = self.db.lock().await;
        let all: Vec<(String, StoredUser)> =
            db.scan_typed(USERS_STORE, "").map_err(StorageError::from)?;

        let mut users: Vec<StoredUser> = all.into_iter().map(|(_, u)| u).collect();
        users.sort_by(|a, b| a.email.to_lowercase().cmp(&b.email.to_lowercase()));
        Ok(users)
    }

    async fn delete(&self, id: &str) -> Result<bool, StorageError> {
        let mut db = self.db.lock().await;

        // Look up the email so we can clean up the secondary index.
        let Some(existing) = db
            .get_typed::<StoredUser>(USERS_STORE, id)
            .map_err(StorageError::from)?
        else {
            return Ok(false);
        };

        let email_key = existing.email.to_lowercase();
        db.delete_typed(USERS_BY_EMAIL_STORE, &email_key)
            .map_err(StorageError::from)?;
        db.delete_typed(USERS_STORE, id).map_err(StorageError::from)
    }

    async fn count(&self) -> Result<u64, StorageError> {
        let mut db = self.db.lock().await;
        let all: Vec<(String, StoredUser)> =
            db.scan_typed(USERS_STORE, "").map_err(StorageError::from)?;
        Ok(all.len() as u64)
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
    // ZqlUserStore tests
    // =========================================================================

    #[tokio::test]
    async fn test_zql_store_and_get() {
        let store = ZqlUserStore::in_memory().await.unwrap();
        let user = make_user("alice@example.com", "Alice");
        let id = user.id.clone();

        store.store(&user).await.unwrap();

        let retrieved = store.get(&id).await.unwrap().expect("user must exist");
        assert_eq!(retrieved.email, "alice@example.com");
        assert_eq!(retrieved.display_name, "Alice");
    }

    #[tokio::test]
    async fn test_zql_get_by_email_case_insensitive() {
        let store = ZqlUserStore::in_memory().await.unwrap();
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
    async fn test_zql_get_by_email_nonexistent() {
        let store = ZqlUserStore::in_memory().await.unwrap();
        let missing = store.get_by_email("missing@example.com").await.unwrap();
        assert!(missing.is_none());
    }

    #[tokio::test]
    async fn test_zql_list_sorted_by_email() {
        let store = ZqlUserStore::in_memory().await.unwrap();
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
    async fn test_zql_delete() {
        let store = ZqlUserStore::in_memory().await.unwrap();
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
    async fn test_zql_count() {
        let store = ZqlUserStore::in_memory().await.unwrap();
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
    async fn test_zql_update_by_id_advances_updated_at() {
        let store = ZqlUserStore::in_memory().await.unwrap();
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
    async fn test_zql_unique_email_rejects_different_id() {
        let store = ZqlUserStore::in_memory().await.unwrap();
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
    async fn test_zql_persistent_storage() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("users_zql_db");

        let user = make_user("persist@example.com", "Persist");
        let id = user.id.clone();

        // Create and populate database
        {
            let store = ZqlUserStore::open(&db_path).await.unwrap();
            store.store(&user).await.unwrap();
        }

        // Reopen and verify data persists
        {
            let store = ZqlUserStore::open(&db_path).await.unwrap();
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
