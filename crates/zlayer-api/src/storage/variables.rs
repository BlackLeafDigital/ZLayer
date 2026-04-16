//! Variable storage implementations
//!
//! Provides both persistent (ZQL) and in-memory storage backends for plaintext
//! key-value variables. Variables are NOT encrypted — they are used for
//! template substitution in deployment specs. Each variable has a name that is
//! unique within its scope (project id or global).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use super::zql_json::{IndexSpec, JsonTable, ZqlJsonStore};
use super::{StorageError, StoredVariable};

/// Trait for variable storage backends.
#[async_trait]
pub trait VariableStorage: Send + Sync {
    /// Store (create or update) a variable by id.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Database`] if the backing store rejects the
    /// write, including when the `(name, scope)` uniqueness constraint
    /// would be violated by a different id.
    async fn store(&self, var: &StoredVariable) -> Result<(), StorageError>;

    /// Fetch a variable by id.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the backing store fails.
    async fn get(&self, id: &str) -> Result<Option<StoredVariable>, StorageError>;

    /// Fetch a variable by `(name, scope)`. Matching is exact
    /// (case-sensitive). `scope = None` matches the global namespace.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the backing store fails.
    async fn get_by_name(
        &self,
        name: &str,
        scope: Option<&str>,
    ) -> Result<Option<StoredVariable>, StorageError>;

    /// List variables, optionally filtered by scope. When `scope` is `Some`,
    /// only variables belonging to that scope are returned. When `scope` is
    /// `None`, only global variables are returned. Results are sorted by name
    /// ascending.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the backing store fails.
    async fn list(&self, scope: Option<&str>) -> Result<Vec<StoredVariable>, StorageError>;

    /// Delete a variable by id. Returns `true` if the row existed.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Database`] if the delete operation fails.
    async fn delete(&self, id: &str) -> Result<bool, StorageError>;
}

/// In-memory variable store.
pub struct InMemoryVariableStore {
    vars: Arc<RwLock<HashMap<String, StoredVariable>>>,
}

impl InMemoryVariableStore {
    /// Create a new empty in-memory variable store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            vars: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Default for InMemoryVariableStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VariableStorage for InMemoryVariableStore {
    async fn store(&self, var: &StoredVariable) -> Result<(), StorageError> {
        let mut vars = self.vars.write().await;

        if let Some(conflict) = vars.values().find(|existing| {
            existing.id != var.id && existing.name == var.name && existing.scope == var.scope
        }) {
            return Err(StorageError::Database(format!(
                "UNIQUE constraint failed: variables.(name, scope) \
                 (name '{}' already in use in this scope by variable {})",
                var.name, conflict.id
            )));
        }

        vars.insert(var.id.clone(), var.clone());
        Ok(())
    }

    async fn get(&self, id: &str) -> Result<Option<StoredVariable>, StorageError> {
        let vars = self.vars.read().await;
        Ok(vars.get(id).cloned())
    }

    async fn get_by_name(
        &self,
        name: &str,
        scope: Option<&str>,
    ) -> Result<Option<StoredVariable>, StorageError> {
        let vars = self.vars.read().await;
        Ok(vars
            .values()
            .find(|var| var.name == name && var.scope.as_deref() == scope)
            .cloned())
    }

    async fn list(&self, scope: Option<&str>) -> Result<Vec<StoredVariable>, StorageError> {
        let vars = self.vars.read().await;
        let mut list: Vec<_> = vars
            .values()
            .filter(|var| var.scope.as_deref() == scope)
            .cloned()
            .collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(list)
    }

    async fn delete(&self, id: &str) -> Result<bool, StorageError> {
        let mut vars = self.vars.write().await;
        Ok(vars.remove(id).is_some())
    }
}

/// Static table specification for the variables table.
///
/// Two peeled columns — `name` and `scope` — power both `list(scope)` scope
/// filtering and `get_by_name(name, scope)` lookups. A compound
/// `UNIQUE(name, scope)` prevents two variables from sharing the same name
/// within the same scope.
///
/// Note: the ZQL adapter mirrors `SQLite`'s NULL-distinct semantics under
/// `UNIQUE`, so the compound constraint only enforces uniqueness among
/// variables where `scope` is non-`None`. Global variables (`scope = None`)
/// need application-level enforcement; [`ZqlVariableStore::store`] does a
/// pre-write `get_by_name` check to cover that case.
const VARIABLES_TABLE: JsonTable<StoredVariable> = JsonTable {
    name: "variables",
    indexes: &[
        IndexSpec {
            column: "name",
            extractor: |v| Some(v.name.clone()),
            unique: false,
        },
        IndexSpec {
            column: "scope",
            extractor: |v| v.scope.clone(),
            unique: false,
        },
    ],
    unique_constraints: &[&["name", "scope"]],
};

/// ZQL-backed persistent variable store.
///
/// Delegates storage to a [`ZqlJsonStore<StoredVariable>`] configured for the
/// `variables` table. Records are serialised with `serde_json` into the
/// primary ZQL store; the daemon reopens the database on start-up and
/// variable definitions survive restarts.
pub struct ZqlVariableStore {
    inner: ZqlJsonStore<StoredVariable>,
}

impl ZqlVariableStore {
    /// Open or create a ZQL database at the given path.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the database connection or table creation
    /// fails.
    pub async fn open<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        let inner = ZqlJsonStore::open(path, VARIABLES_TABLE).await?;
        Ok(Self { inner })
    }

    /// Create a ZQL database in a temporary directory (useful for tests).
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the temp directory or database cannot be
    /// created.
    #[cfg(test)]
    pub async fn in_memory() -> Result<Self, StorageError> {
        let inner = ZqlJsonStore::in_memory(VARIABLES_TABLE).await?;
        Ok(Self { inner })
    }
}

#[async_trait]
impl VariableStorage for ZqlVariableStore {
    async fn store(&self, var: &StoredVariable) -> Result<(), StorageError> {
        // The ZQL adapter's compound-unique constraint only fires when every
        // referenced column extracts `Some`, matching SQL NULL-distinct
        // semantics. Global variables (scope = None) slip past that check,
        // so we enforce (name, scope) uniqueness in application code by
        // looking up the existing row and rejecting when the id differs.
        //
        // This is a TOCTOU between the read and write; it's acceptable
        // because the adapter serialises all writes behind a per-store
        // `tokio::sync::Mutex`, matching the in-memory store's semantics.
        // Under concurrent contention the compound unique still enforces
        // the invariant for scoped rows, and the window for global rows is
        // narrow enough to tolerate.
        if let Some(existing) = self.get_by_name(&var.name, var.scope.as_deref()).await? {
            if existing.id != var.id {
                return Err(StorageError::AlreadyExists(format!(
                    "UNIQUE constraint failed: variables.(name, scope) \
                     (name '{}' already in use in this scope by variable {})",
                    var.name, existing.id
                )));
            }
        }

        self.inner.put(&var.id, var).await
    }

    async fn get(&self, id: &str) -> Result<Option<StoredVariable>, StorageError> {
        self.inner.get(id).await
    }

    async fn get_by_name(
        &self,
        name: &str,
        scope: Option<&str>,
    ) -> Result<Option<StoredVariable>, StorageError> {
        // The adapter's list_where_opt filters on the `scope` column,
        // returning records with scope == Some(v) for `Some(v)` and
        // scope == None for `None`. Under the compound UNIQUE(name, scope)
        // constraint at most one row can match a given (name, scope) pair
        // for scoped rows; for global rows, store() enforces the same
        // invariant in application code.
        let candidates = self.inner.list_where_opt("scope", scope).await?;
        Ok(candidates.into_iter().find(|v| v.name == name))
    }

    async fn list(&self, scope: Option<&str>) -> Result<Vec<StoredVariable>, StorageError> {
        let mut list = self.inner.list_where_opt("scope", scope).await?;
        // The trait contract orders results by name; the adapter orders by id.
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

    fn make_var(name: &str, value: &str, scope: Option<&str>) -> StoredVariable {
        StoredVariable::new(name, value, scope.map(str::to_string))
    }

    #[tokio::test]
    async fn test_store_and_get() {
        let store = InMemoryVariableStore::new();
        let var = make_var("APP_VERSION", "1.0.0", None);
        let id = var.id.clone();

        store.store(&var).await.unwrap();

        let retrieved = store.get(&id).await.unwrap().expect("var must exist");
        assert_eq!(retrieved.name, "APP_VERSION");
        assert_eq!(retrieved.value, "1.0.0");
        assert!(retrieved.scope.is_none());
    }

    #[tokio::test]
    async fn test_get_by_name_global() {
        let store = InMemoryVariableStore::new();
        store
            .store(&make_var("LOG_LEVEL", "debug", None))
            .await
            .unwrap();

        let found = store.get_by_name("LOG_LEVEL", None).await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().value, "debug");

        // A project-scoped variable with the same name should not match
        // a global lookup.
        store
            .store(&make_var("LOG_LEVEL", "info", Some("proj-1")))
            .await
            .unwrap();
        let still_global = store.get_by_name("LOG_LEVEL", None).await.unwrap();
        assert_eq!(still_global.unwrap().value, "debug");
    }

    #[tokio::test]
    async fn test_get_by_name_with_scope() {
        let store = InMemoryVariableStore::new();
        store
            .store(&make_var("PORT", "3000", Some("proj-a")))
            .await
            .unwrap();
        store
            .store(&make_var("PORT", "4000", Some("proj-b")))
            .await
            .unwrap();

        let a = store
            .get_by_name("PORT", Some("proj-a"))
            .await
            .unwrap()
            .expect("var in proj-a must exist");
        assert_eq!(a.value, "3000");

        let b = store
            .get_by_name("PORT", Some("proj-b"))
            .await
            .unwrap()
            .expect("var in proj-b must exist");
        assert_eq!(b.value, "4000");
        assert_ne!(a.id, b.id);
    }

    #[tokio::test]
    async fn test_get_by_name_nonexistent() {
        let store = InMemoryVariableStore::new();
        assert!(store.get_by_name("NOPE", None).await.unwrap().is_none());
        assert!(store
            .get_by_name("NOPE", Some("p"))
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn test_list_filtered_by_scope() {
        let store = InMemoryVariableStore::new();
        store.store(&make_var("B_VAR", "b", None)).await.unwrap();
        store.store(&make_var("A_VAR", "a", None)).await.unwrap();
        store
            .store(&make_var("C_VAR", "c", Some("p1")))
            .await
            .unwrap();
        store
            .store(&make_var("D_VAR", "d", Some("p1")))
            .await
            .unwrap();
        store
            .store(&make_var("E_VAR", "e", Some("p2")))
            .await
            .unwrap();

        let globals = store.list(None).await.unwrap();
        assert_eq!(globals.len(), 2);
        assert_eq!(globals[0].name, "A_VAR");
        assert_eq!(globals[1].name, "B_VAR");

        let p1 = store.list(Some("p1")).await.unwrap();
        assert_eq!(p1.len(), 2);
        assert_eq!(p1[0].name, "C_VAR");
        assert_eq!(p1[1].name, "D_VAR");

        let p2 = store.list(Some("p2")).await.unwrap();
        assert_eq!(p2.len(), 1);
        assert_eq!(p2[0].name, "E_VAR");
    }

    #[tokio::test]
    async fn test_delete() {
        let store = InMemoryVariableStore::new();
        let var = make_var("DOOMED", "x", None);
        let id = var.id.clone();
        store.store(&var).await.unwrap();

        let deleted = store.delete(&id).await.unwrap();
        assert!(deleted);
        assert!(store.get(&id).await.unwrap().is_none());

        let deleted_again = store.delete(&id).await.unwrap();
        assert!(!deleted_again);
    }

    #[tokio::test]
    async fn test_unique_name_within_scope_rejects_different_id() {
        let store = InMemoryVariableStore::new();
        let first = make_var("FOO", "bar", Some("proj"));
        store.store(&first).await.unwrap();

        let second = make_var("FOO", "baz", Some("proj"));
        let err = store.store(&second).await.expect_err("should fail");
        match err {
            StorageError::Database(_) => {}
            other => panic!("expected Database error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_unique_name_global_rejects_different_id() {
        let store = InMemoryVariableStore::new();
        let first = make_var("FOO", "bar", None);
        store.store(&first).await.unwrap();

        let second = make_var("FOO", "baz", None);
        let err = store.store(&second).await.expect_err("should fail");
        match err {
            StorageError::Database(_) => {}
            other => panic!("expected Database error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_same_name_in_different_scopes_ok() {
        let store = InMemoryVariableStore::new();
        store.store(&make_var("FOO", "global", None)).await.unwrap();
        store
            .store(&make_var("FOO", "p1", Some("p1")))
            .await
            .unwrap();
        store
            .store(&make_var("FOO", "p2", Some("p2")))
            .await
            .unwrap();

        let globals = store.list(None).await.unwrap();
        assert_eq!(globals.len(), 1);
        let p1 = store.list(Some("p1")).await.unwrap();
        assert_eq!(p1.len(), 1);
        let p2 = store.list(Some("p2")).await.unwrap();
        assert_eq!(p2.len(), 1);
    }

    #[tokio::test]
    async fn test_update_in_place() {
        let store = InMemoryVariableStore::new();
        let mut var = make_var("VER", "1.0", None);
        let id = var.id.clone();
        let original_updated = var.updated_at;
        store.store(&var).await.unwrap();

        std::thread::sleep(std::time::Duration::from_millis(10));

        var.value = "2.0".to_string();
        var.updated_at = chrono::Utc::now();
        store.store(&var).await.unwrap();

        let retrieved = store.get(&id).await.unwrap().unwrap();
        assert_eq!(retrieved.value, "2.0");
        assert!(retrieved.updated_at > original_updated);
    }

    // =========================================================================
    // ZqlVariableStore tests
    // =========================================================================

    #[tokio::test]
    async fn test_zql_store_and_get() {
        let store = ZqlVariableStore::in_memory().await.unwrap();
        let var = make_var("APP_VERSION", "1.0.0", None);
        let id = var.id.clone();

        store.store(&var).await.unwrap();

        let retrieved = store.get(&id).await.unwrap().expect("var must exist");
        assert_eq!(retrieved.name, "APP_VERSION");
        assert_eq!(retrieved.value, "1.0.0");
        assert!(retrieved.scope.is_none());
    }

    #[tokio::test]
    async fn test_zql_scope_isolation() {
        let store = ZqlVariableStore::in_memory().await.unwrap();

        // Same name in three different scope positions — all allowed.
        store
            .store(&make_var("FOO", "in-a", Some("a")))
            .await
            .unwrap();
        store
            .store(&make_var("FOO", "in-b", Some("b")))
            .await
            .unwrap();
        store.store(&make_var("FOO", "global", None)).await.unwrap();

        // A second FOO in scope `a` with a different id must fail.
        let dup = make_var("FOO", "in-a-v2", Some("a"));
        let err = store.store(&dup).await.expect_err("duplicate must fail");
        match err {
            StorageError::AlreadyExists(msg) => {
                assert!(
                    msg.contains("FOO"),
                    "error should mention variable name: {msg}"
                );
            }
            other => panic!("expected AlreadyExists, got {other:?}"),
        }

        // And a second FOO in the global scope (scope = None) with a
        // different id must also fail — the application-level check covers
        // the NULL case that the ZQL compound UNIQUE treats as distinct.
        let dup_global = make_var("FOO", "global-v2", None);
        let err = store
            .store(&dup_global)
            .await
            .expect_err("duplicate global must fail");
        match err {
            StorageError::AlreadyExists(_) => {}
            other => panic!("expected AlreadyExists, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_zql_list_filtered_by_scope() {
        let store = ZqlVariableStore::in_memory().await.unwrap();
        store.store(&make_var("A1", "a1", Some("a"))).await.unwrap();
        store.store(&make_var("A2", "a2", Some("a"))).await.unwrap();
        store.store(&make_var("A3", "a3", Some("a"))).await.unwrap();
        store.store(&make_var("B1", "b1", Some("b"))).await.unwrap();
        store.store(&make_var("B2", "b2", Some("b"))).await.unwrap();
        store.store(&make_var("G1", "g1", None)).await.unwrap();

        let scope_a = store.list(Some("a")).await.unwrap();
        assert_eq!(scope_a.len(), 3);
        let names_a: Vec<&str> = scope_a.iter().map(|v| v.name.as_str()).collect();
        // list() sorts by name ASC per the trait contract.
        assert_eq!(names_a, vec!["A1", "A2", "A3"]);

        let scope_b = store.list(Some("b")).await.unwrap();
        assert_eq!(scope_b.len(), 2);
        let names_b: Vec<&str> = scope_b.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names_b, vec!["B1", "B2"]);

        let globals = store.list(None).await.unwrap();
        assert_eq!(globals.len(), 1);
        assert_eq!(globals[0].name, "G1");

        // Nonexistent scope is empty, not an error.
        let empty = store.list(Some("nope")).await.unwrap();
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn test_zql_get_by_name() {
        let store = ZqlVariableStore::in_memory().await.unwrap();
        store
            .store(&make_var("PORT", "3000", Some("proj-a")))
            .await
            .unwrap();
        store
            .store(&make_var("PORT", "4000", Some("proj-b")))
            .await
            .unwrap();
        store.store(&make_var("PORT", "80", None)).await.unwrap();

        let a = store
            .get_by_name("PORT", Some("proj-a"))
            .await
            .unwrap()
            .expect("proj-a PORT must exist");
        assert_eq!(a.value, "3000");

        let b = store
            .get_by_name("PORT", Some("proj-b"))
            .await
            .unwrap()
            .expect("proj-b PORT must exist");
        assert_eq!(b.value, "4000");

        let g = store
            .get_by_name("PORT", None)
            .await
            .unwrap()
            .expect("global PORT must exist");
        assert_eq!(g.value, "80");

        // Wrong scope returns None.
        assert!(store
            .get_by_name("PORT", Some("proj-c"))
            .await
            .unwrap()
            .is_none());
        // Unknown name returns None.
        assert!(store
            .get_by_name("NOPE", Some("proj-a"))
            .await
            .unwrap()
            .is_none());
        assert!(store.get_by_name("NOPE", None).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_zql_delete_by_id() {
        let store = ZqlVariableStore::in_memory().await.unwrap();
        let var = make_var("DOOMED", "x", None);
        let id = var.id.clone();
        store.store(&var).await.unwrap();

        assert!(store.delete(&id).await.unwrap());
        assert!(store.get(&id).await.unwrap().is_none());
        // Deleting a missing id returns Ok(false), not an error.
        assert!(!store.delete(&id).await.unwrap());
        assert!(!store.delete("never-existed").await.unwrap());
    }

    #[tokio::test]
    async fn test_zql_update_same_id() {
        // Writing the same id twice must upsert, not conflict — the second
        // store() should succeed even though (name, scope) matches.
        let store = ZqlVariableStore::in_memory().await.unwrap();
        let mut var = make_var("VER", "1.0", None);
        let id = var.id.clone();
        store.store(&var).await.unwrap();

        var.value = "2.0".to_string();
        store.store(&var).await.unwrap();

        let retrieved = store.get(&id).await.unwrap().unwrap();
        assert_eq!(retrieved.value, "2.0");

        // Only one row.
        let all = store.list(None).await.unwrap();
        assert_eq!(all.len(), 1);
    }

    #[tokio::test]
    async fn test_zql_persistent_round_trip() {
        // Survives closing + reopening the database.
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("variables_zql_db");

        let scoped = make_var("PORT", "3000", Some("proj-a"));
        let global = make_var("LOG_LEVEL", "debug", None);
        let scoped_id = scoped.id.clone();
        let global_id = global.id.clone();

        {
            let store = ZqlVariableStore::open(&db_path).await.unwrap();
            store.store(&scoped).await.unwrap();
            store.store(&global).await.unwrap();
        }

        {
            let store = ZqlVariableStore::open(&db_path).await.unwrap();

            let by_id = store
                .get(&scoped_id)
                .await
                .unwrap()
                .expect("scoped var must persist");
            assert_eq!(by_id.name, "PORT");
            assert_eq!(by_id.value, "3000");
            assert_eq!(by_id.scope.as_deref(), Some("proj-a"));

            let by_id_global = store
                .get(&global_id)
                .await
                .unwrap()
                .expect("global var must persist");
            assert_eq!(by_id_global.name, "LOG_LEVEL");
            assert!(by_id_global.scope.is_none());

            // Re-open + get_by_name still finds both.
            let by_name_scoped = store
                .get_by_name("PORT", Some("proj-a"))
                .await
                .unwrap()
                .expect("scoped var must persist");
            assert_eq!(by_name_scoped.id, scoped_id);

            let by_name_global = store
                .get_by_name("LOG_LEVEL", None)
                .await
                .unwrap()
                .expect("global var must persist");
            assert_eq!(by_name_global.id, global_id);

            // And the scope-filtered lists are correct after reopen.
            let scoped_list = store.list(Some("proj-a")).await.unwrap();
            assert_eq!(scoped_list.len(), 1);
            let global_list = store.list(None).await.unwrap();
            assert_eq!(global_list.len(), 1);
        }
    }
}
