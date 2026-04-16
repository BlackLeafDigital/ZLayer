//! Generic ZQL adapter for blob-shaped stores.
//!
//! `ZqlJsonStore<T>` is the persistence engine shared by the "blob + optional
//! unique indexes" style of `ZLayer` resource — notifiers, variables, tasks,
//! workflows, user groups, and syncs — in the `zlayer-api-zql` mirror. A
//! record is stored as a typed value in a ZQL store keyed by `id`; zero or
//! more "columns" are peeled out via caller-supplied extractor functions so
//! that unique constraints and secondary lookups remain cheap O(1) lookups
//! rather than whole-store scans.
//!
//! # Design
//!
//! Unlike the `sqlx` implementation (which uses a single table with peeled
//! columns and SQL `UNIQUE` constraints), ZQL does not have a dedicated
//! unique-index primitive. Uniqueness is enforced by maintaining a parallel
//! ZQL store per unique index:
//!
//! - Primary records live in `table.name`, keyed by `id`.
//! - Each `IndexSpec { unique: true }` with column `col` gets a companion
//!   store `{name}_by_{col}`, keyed by the extracted value (as a string) and
//!   holding the owning primary id.
//! - Each compound `unique_constraints` entry `&["a", "b", ...]` gets a
//!   companion store `{name}_by_a_b_...`, keyed by the joined extracted
//!   values (`"{val_a}|{val_b}|..."`) and holding the owning primary id.
//!
//! Non-unique indexes (`IndexSpec { unique: false }`) do NOT get a companion
//! store — their lookups fall back to filtering after a primary scan.
//!
//! # Caveat: NULL handling in unique indexes
//!
//! `SQLite` treats `NULL`s as distinct under `UNIQUE(...)`. This ZQL adapter
//! mirrors that behaviour: when an extractor returns `None`, no entry is
//! written to the unique companion store, so multiple records with a `None`
//! extracted value coexist peacefully. Callers needing "unique including
//! `None`" must enforce it in application code, exactly as the `sqlx`
//! implementation requires.
//!
//! # Caller contract
//!
//! `T` must be `Serialize + DeserializeOwned + Send + Sync + 'static`.
//! `JsonTable::name` and `IndexSpec::column` are `&'static str`, sourced
//! from compile-time literals at every call site, so store names built by
//! concatenation are always safe.

use std::collections::HashSet;
use std::marker::PhantomData;
use std::path::Path;

use serde::de::DeserializeOwned;
use serde::Serialize;

use super::StorageError;

/// Specification for a single secondary column peeled out of `T` at write
/// time.
pub struct IndexSpec<T> {
    /// Column name. Used to build companion-store names at compile-time, so
    /// this MUST be a stable ASCII identifier.
    pub column: &'static str,

    /// Extractor pulling the column value out of `T`. `None` means the
    /// record has no value for this column (not indexed).
    pub extractor: fn(&T) -> Option<String>,

    /// If `true`, a parallel `{table}_by_{column}` store is maintained so
    /// that `put` rejects duplicate values (for different ids) with
    /// [`StorageError::AlreadyExists`].
    pub unique: bool,
}

/// Full table specification: name + ordered list of indexed columns +
/// compound-unique constraints.
pub struct JsonTable<T: 'static> {
    /// Store name. Used as the primary ZQL store and as the prefix for
    /// companion stores. MUST be a compile-time string literal.
    pub name: &'static str,

    /// Indexed columns. May be empty for pure blob-only tables.
    pub indexes: &'static [IndexSpec<T>],

    /// Compound uniqueness across multiple columns. Each inner slice is a
    /// tuple of column names; uniqueness is enforced only when every
    /// referenced extractor returns `Some` (matching the `NULL`-distinct
    /// behaviour of the `sqlx` variant).
    ///
    /// Every referenced column MUST also appear in `indexes` so its
    /// extractor is known to the adapter.
    pub unique_constraints: &'static [&'static [&'static str]],
}

/// Generic ZQL blob store keyed by a string `id`.
///
/// Use via a narrow wrapper type in each storage module — see
/// `notifiers.rs` / `tasks.rs` for the pattern once they're ported to this
/// adapter.
pub struct ZqlJsonStore<T>
where
    T: Serialize + DeserializeOwned + Send + Sync + 'static,
{
    db: tokio::sync::Mutex<zql::Database>,
    table: JsonTable<T>,
    _marker: PhantomData<fn() -> T>,
}

impl<T> ZqlJsonStore<T>
where
    T: Serialize + DeserializeOwned + Send + Sync + 'static,
{
    /// Open or create a ZQL database at the given path.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the database cannot be opened or the
    /// `spawn_blocking` worker panics.
    pub async fn open<P: AsRef<Path>>(path: P, table: JsonTable<T>) -> Result<Self, StorageError> {
        debug_assert_unique_constraint_columns(&table);
        let path = path.as_ref().to_path_buf();

        let db = tokio::task::spawn_blocking(move || zql::Database::open(&path))
            .await
            .map_err(|e| StorageError::Database(format!("spawn_blocking failed: {e}")))?
            .map_err(StorageError::from)?;

        Ok(Self {
            db: tokio::sync::Mutex::new(db),
            table,
            _marker: PhantomData,
        })
    }

    /// Create a ZQL database in a temporary directory (primarily for tests).
    /// The temp directory is leaked for the lifetime of the returned store —
    /// it is cleaned up by the OS when the process exits.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the temp directory or database cannot be
    /// created.
    #[cfg(test)]
    pub async fn in_memory(table: JsonTable<T>) -> Result<Self, StorageError> {
        debug_assert_unique_constraint_columns(&table);
        let temp_dir = tempfile::tempdir()
            .map_err(|e| StorageError::Database(format!("failed to create temp dir: {e}")))?;
        let path = temp_dir.path().join("zql_json_store");

        let db = tokio::task::spawn_blocking(move || {
            // Keep the temp dir alive inside the worker so its Drop runs on
            // the blocking pool, not on the async runtime.
            let _keep = temp_dir;
            zql::Database::open(&path)
        })
        .await
        .map_err(|e| StorageError::Database(format!("spawn_blocking failed: {e}")))?
        .map_err(StorageError::from)?;

        Ok(Self {
            db: tokio::sync::Mutex::new(db),
            table,
            _marker: PhantomData,
        })
    }

    /// Crate-internal accessor for the underlying ZQL database mutex.
    ///
    /// Used by storage modules that need to maintain secondary link tables
    /// alongside the adapter-managed primary store — e.g. `workflow_runs`
    /// history colocated with the `workflows` primary store, or
    /// `ZqlGroupStore` colocating `group_members` on the same database. The
    /// secondary module locks this same mutex so it shares a single writer
    /// with primary-record operations and cross-store deletes can be ordered
    /// without a second database handle.
    pub(crate) fn db(&self) -> &tokio::sync::Mutex<zql::Database> {
        &self.db
    }

    // ---- Companion-store key helpers --------------------------------------

    /// Build the companion store name for a single unique column.
    fn unique_store_name(&self, column: &str) -> String {
        format!("{table}_by_{column}", table = self.table.name)
    }

    /// Build the companion store name for a compound unique constraint.
    fn compound_store_name(&self, columns: &[&str]) -> String {
        let mut name = String::with_capacity(self.table.name.len() + 4 + 8 * columns.len());
        name.push_str(self.table.name);
        name.push_str("_by");
        for col in columns {
            name.push('_');
            name.push_str(col);
        }
        name
    }

    /// Join extracted values for a compound key. Returns `None` if any of
    /// the referenced columns extracted `None` (so the constraint is not
    /// enforced for this row, matching SQL NULL-distinct semantics).
    fn compound_key(&self, columns: &[&str], value: &T) -> Option<String> {
        let mut parts: Vec<String> = Vec::with_capacity(columns.len());
        for col in columns {
            let extractor = self
                .table
                .indexes
                .iter()
                .find(|i| i.column == *col)
                .expect("unique_constraints column must exist in indexes")
                .extractor;
            let v = extractor(value)?;
            parts.push(v);
        }
        Some(parts.join("|"))
    }

    // ---- CRUD -------------------------------------------------------------

    /// Upsert a record by its primary key `id`.
    ///
    /// # Errors
    ///
    /// - [`StorageError::AlreadyExists`] if the write would collide with a
    ///   unique index or compound unique constraint held by a different id.
    /// - [`StorageError::Database`] for any ZQL failure.
    pub async fn put(&self, id: &str, value: &T) -> Result<(), StorageError> {
        let mut db = self.db.lock().await;

        self.check_unique_conflicts(&mut db, id, value)?;

        let previous = db
            .get_typed::<T>(self.table.name, id)
            .map_err(StorageError::from)?;
        if let Some(prev) = previous.as_ref() {
            self.retire_stale_index_entries(&mut db, prev, value)?;
        }

        db.put_typed(self.table.name, id, value)
            .map_err(StorageError::from)?;

        self.write_index_entries(&mut db, id, value)?;

        Ok(())
    }

    /// Verify that no unique companion slot for any extracted value from
    /// `value` is already owned by a different id.
    fn check_unique_conflicts(
        &self,
        db: &mut zql::Database,
        id: &str,
        value: &T,
    ) -> Result<(), StorageError> {
        for idx in self.table.indexes {
            if !idx.unique {
                continue;
            }
            let Some(new_val) = (idx.extractor)(value) else {
                continue;
            };
            let store = self.unique_store_name(idx.column);
            if let Some(existing_id) = db
                .get_typed::<String>(&store, &new_val)
                .map_err(StorageError::from)?
            {
                if existing_id != id {
                    return Err(StorageError::AlreadyExists(format!(
                        "UNIQUE constraint failed on table '{table}' column '{col}': \
                         value '{new_val}' already owned by id '{existing_id}'",
                        table = self.table.name,
                        col = idx.column,
                    )));
                }
            }
        }

        for constraint in self.table.unique_constraints {
            if constraint.is_empty() {
                continue;
            }
            let Some(new_key) = self.compound_key(constraint, value) else {
                continue;
            };
            let store = self.compound_store_name(constraint);
            if let Some(existing_id) = db
                .get_typed::<String>(&store, &new_key)
                .map_err(StorageError::from)?
            {
                if existing_id != id {
                    return Err(StorageError::AlreadyExists(format!(
                        "UNIQUE constraint failed on table '{table}' columns {cols:?}: \
                         value '{new_key}' already owned by id '{existing_id}'",
                        table = self.table.name,
                        cols = constraint,
                    )));
                }
            }
        }

        Ok(())
    }

    /// For every companion slot whose extracted value changed from
    /// `previous` to `new_value`, delete the old slot so the caller can
    /// write a fresh one (or another id can claim it).
    fn retire_stale_index_entries(
        &self,
        db: &mut zql::Database,
        previous: &T,
        new_value: &T,
    ) -> Result<(), StorageError> {
        for idx in self.table.indexes {
            if !idx.unique {
                continue;
            }
            let old_val = (idx.extractor)(previous);
            let new_val = (idx.extractor)(new_value);
            if old_val != new_val {
                if let Some(v) = old_val {
                    let store = self.unique_store_name(idx.column);
                    db.delete_typed(&store, &v).map_err(StorageError::from)?;
                }
            }
        }

        for constraint in self.table.unique_constraints {
            if constraint.is_empty() {
                continue;
            }
            let old_key = self.compound_key(constraint, previous);
            let new_key = self.compound_key(constraint, new_value);
            if old_key != new_key {
                if let Some(k) = old_key {
                    let store = self.compound_store_name(constraint);
                    db.delete_typed(&store, &k).map_err(StorageError::from)?;
                }
            }
        }

        Ok(())
    }

    /// Write every companion slot for the fresh `value` under primary `id`.
    fn write_index_entries(
        &self,
        db: &mut zql::Database,
        id: &str,
        value: &T,
    ) -> Result<(), StorageError> {
        for idx in self.table.indexes {
            if !idx.unique {
                continue;
            }
            let Some(new_val) = (idx.extractor)(value) else {
                continue;
            };
            let store = self.unique_store_name(idx.column);
            db.put_typed(&store, &new_val, &id.to_string())
                .map_err(StorageError::from)?;
        }

        for constraint in self.table.unique_constraints {
            if constraint.is_empty() {
                continue;
            }
            let Some(new_key) = self.compound_key(constraint, value) else {
                continue;
            };
            let store = self.compound_store_name(constraint);
            db.put_typed(&store, &new_key, &id.to_string())
                .map_err(StorageError::from)?;
        }

        Ok(())
    }

    /// Fetch a record by primary key.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Database`] on ZQL failure or a serialization
    /// error inside ZQL.
    pub async fn get(&self, id: &str) -> Result<Option<T>, StorageError> {
        let mut db = self.db.lock().await;
        db.get_typed(self.table.name, id)
            .map_err(StorageError::from)
    }

    /// List every record, ordered by primary key `id` ascending.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Database`] on ZQL failure.
    pub async fn list(&self) -> Result<Vec<T>, StorageError> {
        let mut db = self.db.lock().await;
        let mut all: Vec<(String, T)> = db
            .scan_typed(self.table.name, "")
            .map_err(StorageError::from)?;
        // `scan_typed` ordering is not part of the stated contract; sort
        // explicitly by key so `list()` is deterministic.
        all.sort_by(|(a, _), (b, _)| a.cmp(b));
        Ok(all.into_iter().map(|(_, v)| v).collect())
    }

    /// Delete a record by primary key. Returns `true` if it existed.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Database`] on ZQL failure.
    pub async fn delete(&self, id: &str) -> Result<bool, StorageError> {
        let mut db = self.db.lock().await;

        // Look up the prior record so we can tear down every companion
        // entry it owned.
        let Some(existing) = db
            .get_typed::<T>(self.table.name, id)
            .map_err(StorageError::from)?
        else {
            return Ok(false);
        };

        for idx in self.table.indexes {
            if !idx.unique {
                continue;
            }
            if let Some(v) = (idx.extractor)(&existing) {
                let store = self.unique_store_name(idx.column);
                db.delete_typed(&store, &v).map_err(StorageError::from)?;
            }
        }

        for constraint in self.table.unique_constraints {
            if constraint.is_empty() {
                continue;
            }
            if let Some(k) = self.compound_key(constraint, &existing) {
                let store = self.compound_store_name(constraint);
                db.delete_typed(&store, &k).map_err(StorageError::from)?;
            }
        }

        db.delete_typed(self.table.name, id)
            .map_err(StorageError::from)
    }

    /// Fetch a record by one of its peeled secondary columns. For a
    /// `unique: true` column the companion store is consulted directly; for
    /// a non-unique column the primary store is scanned and the first match
    /// is returned.
    ///
    /// # Errors
    ///
    /// - [`StorageError::Other`] if `column` is not declared in `indexes`.
    /// - [`StorageError::Database`] on ZQL failure.
    pub async fn get_by_unique(
        &self,
        column: &str,
        value: &str,
    ) -> Result<Option<T>, StorageError> {
        let idx = self
            .table
            .indexes
            .iter()
            .find(|i| i.column == column)
            .ok_or_else(|| {
                StorageError::Other(format!(
                    "unknown column '{column}' for table '{table}'",
                    table = self.table.name,
                ))
            })?;

        let mut db = self.db.lock().await;

        if idx.unique {
            let store = self.unique_store_name(column);
            let Some(owner_id) = db
                .get_typed::<String>(&store, value)
                .map_err(StorageError::from)?
            else {
                return Ok(None);
            };
            return db
                .get_typed::<T>(self.table.name, &owner_id)
                .map_err(StorageError::from);
        }

        // Non-unique column: fall back to a primary scan. This matches the
        // `sqlx` adapter's behaviour of returning the first match.
        let extractor = idx.extractor;
        let all: Vec<(String, T)> = db
            .scan_typed(self.table.name, "")
            .map_err(StorageError::from)?;
        let needle = value.to_string();
        Ok(all.into_iter().find_map(|(_, rec)| match extractor(&rec) {
            Some(v) if v == needle => Some(rec),
            _ => None,
        }))
    }

    /// Return the total number of rows.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Database`] on ZQL failure.
    pub async fn count(&self) -> Result<u64, StorageError> {
        let mut db = self.db.lock().await;
        let all: Vec<(String, T)> = db
            .scan_typed(self.table.name, "")
            .map_err(StorageError::from)?;
        Ok(all.len() as u64)
    }

    /// List every record where a peeled column exactly matches `value`,
    /// ordered by primary key ascending. Use [`list_where_opt`](Self::list_where_opt)
    /// if you need to filter on `None`.
    ///
    /// # Errors
    ///
    /// - [`StorageError::Other`] if `column` is not declared in `indexes`.
    /// - [`StorageError::Database`] on ZQL failure.
    pub async fn list_where(&self, column: &str, value: &str) -> Result<Vec<T>, StorageError> {
        self.list_where_opt(column, Some(value)).await
    }

    /// List every record where a peeled column's extractor returns `None`,
    /// ordered by primary key ascending.
    ///
    /// # Errors
    ///
    /// - [`StorageError::Other`] if `column` is not declared in `indexes`.
    /// - [`StorageError::Database`] on ZQL failure.
    pub async fn list_where_null(&self, column: &str) -> Result<Vec<T>, StorageError> {
        self.list_where_opt(column, None).await
    }

    /// Unified filter primitive. `Some(v)` matches records whose extractor
    /// returns `Some(v.to_string())`; `None` matches records whose
    /// extractor returns `None`. Rows are ordered by primary key ascending.
    ///
    /// # Errors
    ///
    /// - [`StorageError::Other`] if `column` is not declared in `indexes`.
    /// - [`StorageError::Database`] on ZQL failure.
    pub async fn list_where_opt(
        &self,
        column: &str,
        value: Option<&str>,
    ) -> Result<Vec<T>, StorageError> {
        let idx = self
            .table
            .indexes
            .iter()
            .find(|i| i.column == column)
            .ok_or_else(|| {
                StorageError::Other(format!(
                    "unknown column '{column}' for table '{table}'",
                    table = self.table.name,
                ))
            })?;

        let extractor = idx.extractor;
        let needle = value.map(str::to_string);

        let mut db = self.db.lock().await;
        let mut all: Vec<(String, T)> = db
            .scan_typed(self.table.name, "")
            .map_err(StorageError::from)?;
        all.sort_by(|(a, _), (b, _)| a.cmp(b));

        let out: Vec<T> = all
            .into_iter()
            .filter_map(|(_, rec)| {
                let extracted = extractor(&rec);
                match (&needle, &extracted) {
                    (Some(v), Some(e)) if v == e => Some(rec),
                    (None, None) => Some(rec),
                    _ => None,
                }
            })
            .collect();

        Ok(out)
    }
}

/// Debug-time guard: every column referenced by `unique_constraints` must
/// also appear in `indexes`, otherwise the compound-key extractor will
/// panic at runtime.
fn debug_assert_unique_constraint_columns<T: 'static>(table: &JsonTable<T>) {
    let indexed: HashSet<&'static str> = table.indexes.iter().map(|i| i.column).collect();
    for constraint in table.unique_constraints {
        for col in *constraint {
            debug_assert!(
                indexed.contains(col),
                "unique_constraints references column '{col}' on table \
                 '{table_name}' that is not declared in JsonTable::indexes",
                table_name = table.name,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    /// Minimal record type exercising both a unique and a non-unique
    /// indexed column plus the blob round-trip.
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    struct TestRecord {
        id: String,
        name: String,
        value: String,
    }

    fn make(id: &str, name: &str, value: &str) -> TestRecord {
        TestRecord {
            id: id.to_string(),
            name: name.to_string(),
            value: value.to_string(),
        }
    }

    /// Table spec with a unique `name` column and a non-unique `value`
    /// column.
    fn indexed_table() -> JsonTable<TestRecord> {
        static INDEXES: &[IndexSpec<TestRecord>] = &[
            IndexSpec {
                column: "name",
                extractor: |r| Some(r.name.clone()),
                unique: true,
            },
            IndexSpec {
                column: "value",
                extractor: |r| Some(r.value.clone()),
                unique: false,
            },
        ];
        JsonTable {
            name: "test_records",
            indexes: INDEXES,
            unique_constraints: &[],
        }
    }

    fn blob_only_table() -> JsonTable<TestRecord> {
        JsonTable {
            name: "test_blobs",
            indexes: &[],
            unique_constraints: &[],
        }
    }

    /// Record with two peeled columns exercising a compound unique
    /// constraint — the shape used by scoped stores.
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    struct ScopedRecord {
        id: String,
        name: String,
        scope: Option<String>,
    }

    fn scoped_table() -> JsonTable<ScopedRecord> {
        static INDEXES: &[IndexSpec<ScopedRecord>] = &[
            IndexSpec {
                column: "name",
                extractor: |r| Some(r.name.clone()),
                unique: false,
            },
            IndexSpec {
                column: "scope",
                extractor: |r| r.scope.clone(),
                unique: false,
            },
        ];
        static UNIQUES: &[&[&str]] = &[&["name", "scope"]];
        JsonTable {
            name: "scoped_records",
            indexes: INDEXES,
            unique_constraints: UNIQUES,
        }
    }

    fn make_scoped(id: &str, name: &str, scope: Option<&str>) -> ScopedRecord {
        ScopedRecord {
            id: id.to_string(),
            name: name.to_string(),
            scope: scope.map(str::to_string),
        }
    }

    #[tokio::test]
    async fn in_memory_round_trip_with_indexes() {
        let store = ZqlJsonStore::in_memory(indexed_table()).await.unwrap();
        let rec = make("id-1", "alpha", "v1");

        store.put(&rec.id, &rec).await.unwrap();

        let got = store.get("id-1").await.unwrap().expect("must exist");
        assert_eq!(got, rec);

        let list = store.list().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0], rec);

        assert!(store.delete("id-1").await.unwrap());
        assert!(store.get("id-1").await.unwrap().is_none());
        assert!(!store.delete("id-1").await.unwrap());
    }

    #[tokio::test]
    async fn in_memory_round_trip_blob_only() {
        let store = ZqlJsonStore::in_memory(blob_only_table()).await.unwrap();
        let rec = make("b-1", "bob", "42");

        store.put(&rec.id, &rec).await.unwrap();
        let got = store.get("b-1").await.unwrap().expect("must exist");
        assert_eq!(got, rec);

        let list = store.list().await.unwrap();
        assert_eq!(list, vec![rec]);
    }

    #[tokio::test]
    async fn unique_conflict_surfaces_as_already_exists() {
        let store = ZqlJsonStore::in_memory(indexed_table()).await.unwrap();
        store.put("a", &make("a", "dup", "v1")).await.unwrap();

        let err = store
            .put("b", &make("b", "dup", "v2"))
            .await
            .expect_err("unique violation must error");

        match err {
            StorageError::AlreadyExists(msg) => {
                assert!(
                    msg.contains("test_records"),
                    "message should mention the table name, got: {msg}"
                );
                assert!(
                    msg.contains("name"),
                    "message should mention the column name, got: {msg}"
                );
            }
            other => panic!("expected AlreadyExists, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unique_upsert_same_id_is_ok() {
        let store = ZqlJsonStore::in_memory(indexed_table()).await.unwrap();
        // Same id, same unique name — must succeed as an idempotent update.
        store.put("x", &make("x", "alpha", "v1")).await.unwrap();
        store.put("x", &make("x", "alpha", "v2")).await.unwrap();
        let got = store.get("x").await.unwrap().unwrap();
        assert_eq!(got.value, "v2");
    }

    #[tokio::test]
    async fn rename_frees_old_unique_slot() {
        let store = ZqlJsonStore::in_memory(indexed_table()).await.unwrap();
        store.put("a", &make("a", "old", "v1")).await.unwrap();
        store.put("a", &make("a", "new", "v1")).await.unwrap();

        // The old name slot must now be reusable by a different id.
        store
            .put("b", &make("b", "old", "v2"))
            .await
            .expect("old unique slot must be free after rename");

        let by_new = store.get_by_unique("name", "new").await.unwrap().unwrap();
        assert_eq!(by_new.id, "a");
        let by_old = store.get_by_unique("name", "old").await.unwrap().unwrap();
        assert_eq!(by_old.id, "b");
    }

    #[tokio::test]
    async fn delete_frees_unique_slot() {
        let store = ZqlJsonStore::in_memory(indexed_table()).await.unwrap();
        store.put("a", &make("a", "dup", "v1")).await.unwrap();
        assert!(store.delete("a").await.unwrap());

        // After delete the name must be available again.
        store
            .put("b", &make("b", "dup", "v2"))
            .await
            .expect("unique slot must be free after delete");
    }

    #[tokio::test]
    async fn get_by_unique_success_and_unknown_column() {
        let store = ZqlJsonStore::in_memory(indexed_table()).await.unwrap();
        store.put("x", &make("x", "name-x", "val-x")).await.unwrap();
        store.put("y", &make("y", "name-y", "val-y")).await.unwrap();

        // Unique column lookup.
        let found = store
            .get_by_unique("name", "name-x")
            .await
            .unwrap()
            .expect("must exist");
        assert_eq!(found.id, "x");

        // Non-unique column lookup still works via primary scan.
        let by_value = store
            .get_by_unique("value", "val-y")
            .await
            .unwrap()
            .expect("must exist");
        assert_eq!(by_value.id, "y");

        // Missing value returns None, not an error.
        let missing = store.get_by_unique("name", "nope").await.unwrap();
        assert!(missing.is_none());

        // Unknown column returns Other, not Database.
        let err = store
            .get_by_unique("not_a_column", "whatever")
            .await
            .expect_err("unknown column must fail");
        match err {
            StorageError::Other(msg) => {
                assert!(msg.contains("not_a_column"));
                assert!(msg.contains("test_records"));
            }
            other => panic!("expected Other, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn count_tracks_inserts_and_deletes() {
        let store = ZqlJsonStore::in_memory(indexed_table()).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 0);

        store.put("1", &make("1", "one", "v")).await.unwrap();
        store.put("2", &make("2", "two", "v")).await.unwrap();
        store.put("3", &make("3", "three", "v")).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 3);

        // Upsert of an existing id must NOT bump the count.
        store.put("1", &make("1", "one", "v2")).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 3);

        assert!(store.delete("2").await.unwrap());
        assert_eq!(store.count().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn list_returns_every_row_sorted_by_id_ascending() {
        let store = ZqlJsonStore::in_memory(blob_only_table()).await.unwrap();
        store.put("c", &make("c", "cc", "3")).await.unwrap();
        store.put("a", &make("a", "aa", "1")).await.unwrap();
        store.put("b", &make("b", "bb", "2")).await.unwrap();

        let list = store.list().await.unwrap();
        assert_eq!(list.len(), 3);
        let ids: Vec<&str> = list.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b", "c"]);
    }

    // =========================================================================
    // Compound UNIQUE + list_where{,_null,_opt} tests
    // =========================================================================

    #[tokio::test]
    async fn compound_unique_rejects_duplicate_pair() {
        let store = ZqlJsonStore::in_memory(scoped_table()).await.unwrap();
        store
            .put("id-1", &make_scoped("id-1", "foo", Some("proj-a")))
            .await
            .unwrap();

        // Same (name, scope) under a different id must fail.
        let err = store
            .put("id-2", &make_scoped("id-2", "foo", Some("proj-a")))
            .await
            .expect_err("compound unique violation must error");
        match err {
            StorageError::AlreadyExists(msg) => {
                assert!(
                    msg.contains("scoped_records"),
                    "message should mention the table name, got: {msg}"
                );
            }
            other => panic!("expected AlreadyExists, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn compound_unique_permits_distinct_combinations() {
        let store = ZqlJsonStore::in_memory(scoped_table()).await.unwrap();
        // Same name in different scopes — allowed.
        store
            .put("id-1", &make_scoped("id-1", "foo", Some("proj-a")))
            .await
            .unwrap();
        store
            .put("id-2", &make_scoped("id-2", "foo", Some("proj-b")))
            .await
            .unwrap();
        // Different name in the same scope — allowed.
        store
            .put("id-3", &make_scoped("id-3", "bar", Some("proj-a")))
            .await
            .unwrap();

        assert_eq!(store.count().await.unwrap(), 3);
    }

    #[tokio::test]
    async fn compound_unique_treats_none_as_distinct() {
        // NULL/None is distinct: two rows with scope = None are allowed even
        // with the same name, matching the SQL NULL-distinct semantics of
        // the sqlx adapter.
        let store = ZqlJsonStore::in_memory(scoped_table()).await.unwrap();
        store
            .put("id-1", &make_scoped("id-1", "foo", None))
            .await
            .unwrap();
        store
            .put("id-2", &make_scoped("id-2", "foo", None))
            .await
            .expect("two rows with NULL scope must coexist");
        assert_eq!(store.count().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn compound_unique_rename_frees_old_key() {
        let store = ZqlJsonStore::in_memory(scoped_table()).await.unwrap();
        store
            .put("id-1", &make_scoped("id-1", "foo", Some("proj-a")))
            .await
            .unwrap();
        // Move id-1 to a different scope — the (foo, proj-a) slot must free up.
        store
            .put("id-1", &make_scoped("id-1", "foo", Some("proj-b")))
            .await
            .unwrap();
        // Another id may now claim (foo, proj-a).
        store
            .put("id-2", &make_scoped("id-2", "foo", Some("proj-a")))
            .await
            .expect("old compound key must free up after rename");
    }

    #[tokio::test]
    async fn list_where_filters_by_column_value() {
        let store = ZqlJsonStore::in_memory(scoped_table()).await.unwrap();
        store
            .put("a", &make_scoped("a", "x", Some("s1")))
            .await
            .unwrap();
        store
            .put("b", &make_scoped("b", "y", Some("s1")))
            .await
            .unwrap();
        store
            .put("c", &make_scoped("c", "z", Some("s2")))
            .await
            .unwrap();

        let s1 = store.list_where("scope", "s1").await.unwrap();
        assert_eq!(s1.len(), 2);
        let ids: Vec<&str> = s1.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b"]);

        let s2 = store.list_where("scope", "s2").await.unwrap();
        assert_eq!(s2.len(), 1);
        assert_eq!(s2[0].id, "c");

        let missing = store.list_where("scope", "s3").await.unwrap();
        assert!(missing.is_empty());
    }

    #[tokio::test]
    async fn list_where_null_filters_none_rows() {
        let store = ZqlJsonStore::in_memory(scoped_table()).await.unwrap();
        store.put("a", &make_scoped("a", "x", None)).await.unwrap();
        store
            .put("b", &make_scoped("b", "y", Some("s1")))
            .await
            .unwrap();
        store.put("c", &make_scoped("c", "z", None)).await.unwrap();

        let nulls = store.list_where_null("scope").await.unwrap();
        assert_eq!(nulls.len(), 2);
        let ids: Vec<&str> = nulls.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "c"]);
    }

    #[tokio::test]
    async fn list_where_opt_unknown_column_returns_other() {
        let store = ZqlJsonStore::in_memory(scoped_table()).await.unwrap();
        let err = store
            .list_where_opt("not_a_column", Some("x"))
            .await
            .expect_err("unknown column must fail");
        match err {
            StorageError::Other(msg) => {
                assert!(msg.contains("not_a_column"));
                assert!(msg.contains("scoped_records"));
            }
            other => panic!("expected Other, got {other:?}"),
        }

        let err2 = store
            .list_where_opt("not_a_column", None)
            .await
            .expect_err("unknown column must fail");
        match err2 {
            StorageError::Other(_) => {}
            other => panic!("expected Other, got {other:?}"),
        }
    }
}
