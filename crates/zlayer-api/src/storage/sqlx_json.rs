//! Generic `SQLite`/`sqlx` adapter for blob-shaped stores.
//!
//! `SqlxJsonStore<T>` is the persistence engine shared by the "blob + optional
//! unique indexes" style of `ZLayer` resource — notifiers, variables, tasks,
//! workflows, user groups, and syncs. A record is stored as an opaque
//! `serde_json`-serialised blob in a `data_json` column; zero or more columns
//! are peeled out via caller-supplied extractor functions so that unique
//! constraints and secondary lookups remain first-class SQL rather than
//! whole-table scans.
//!
//! # Design
//!
//! The caller owns the record type `T`. `T` must be `Serialize +
//! DeserializeOwned + Send + Sync + 'static`. The caller describes how `T`
//! maps onto the table via [`JsonTable`]:
//!
//! ```ignore
//! use zlayer_api::storage::{IndexSpec, JsonTable, SqlxJsonStore};
//!
//! # #[derive(serde::Serialize, serde::Deserialize)]
//! # struct MyRecord { id: String, name: String, scope: Option<String> }
//! let table = JsonTable::<MyRecord> {
//!     name: "my_records",
//!     indexes: &[
//!         IndexSpec { column: "name",  extractor: |r| Some(r.name.clone()), unique: true  },
//!         IndexSpec { column: "scope", extractor: |r| r.scope.clone(),      unique: false },
//!     ],
//!     unique_constraints: &[],
//! };
//! # async fn _example() -> Result<(), zlayer_api::storage::StorageError> {
//! let store: SqlxJsonStore<MyRecord> = SqlxJsonStore::in_memory(table).await?;
//! # Ok(()) }
//! ```
//!
//! The generated schema is:
//!
//! ```sql
//! CREATE TABLE IF NOT EXISTS my_records (
//!     id TEXT PRIMARY KEY NOT NULL,
//!     name TEXT,
//!     scope TEXT,
//!     data_json TEXT NOT NULL,
//!     created_at TEXT NOT NULL,
//!     updated_at TEXT NOT NULL,
//!     UNIQUE(name)
//! )
//! CREATE INDEX IF NOT EXISTS idx_my_records_scope ON my_records(scope)
//! ```
//!
//! A table with an empty `indexes` slice produces the minimal 5-column shape
//! with no `UNIQUE` clause — that is what the notifier store uses.
//!
//! # Compound uniqueness
//!
//! For resources that need uniqueness across a combination of columns (e.g.
//! variables with `UNIQUE(name, scope)`), populate
//! [`JsonTable::unique_constraints`] with static slices of column names. Each
//! inner slice becomes one `UNIQUE(col_a, col_b, ...)` clause in the generated
//! DDL. The columns referenced must also appear in [`JsonTable::indexes`] so
//! that they are peeled out at write time; otherwise `SQLite` has no column to
//! enforce the constraint against.
//!
//! Note: `SQLite` treats `NULL`s as distinct in `UNIQUE`, so a compound unique
//! over `(name, scope)` only enforces uniqueness among rows where every
//! referenced column is non-`NULL`. Rows where any referenced column is `NULL`
//! — for example a global variable with `scope = None` — are enforced in
//! application code via a pre-write `get_by_*` check (see
//! `InMemoryVariableStore::store` and `SqlxVariableStore::store` for the
//! pattern).
//!
//! # Operations
//!
//! - [`put`](SqlxJsonStore::put) — upsert by `id`, preserving `created_at` on
//!   conflict.
//! - [`get`](SqlxJsonStore::get), [`list`](SqlxJsonStore::list),
//!   [`delete`](SqlxJsonStore::delete), [`count`](SqlxJsonStore::count) — the
//!   usual CRUD primitives.
//! - [`get_by_unique`](SqlxJsonStore::get_by_unique) — fetch by any indexed
//!   column (not just unique ones).
//!
//! Unique-constraint violations surface as [`StorageError::AlreadyExists`] —
//! not as a raw `Database(...)` string — so callers can distinguish 409-class
//! errors cleanly.
//!
//! # Identifier safety
//!
//! `JsonTable::name` and `IndexSpec::column` are `&'static str`, so they come
//! from compile-time string literals at every call site. The schema DDL is
//! assembled via `format!` — that is safe for `&'static str` table/column
//! identifiers and would be a disaster for user-supplied strings. Do not
//! pass run-time data to either field.

use std::marker::PhantomData;
use std::path::Path;

use serde::de::DeserializeOwned;
use serde::Serialize;
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use sqlx::Row;

use super::StorageError;

/// Specification for a single secondary column peeled out of `T` at write
/// time. All peeled columns are typed `TEXT` in `SQLite`; `None` from the
/// extractor becomes SQL `NULL`.
pub struct IndexSpec<T> {
    /// Column name. Must be a valid SQL identifier (a-z, A-Z, 0-9, `_`) and
    /// is splat directly into the generated DDL, so this MUST be a
    /// compile-time string literal.
    pub column: &'static str,

    /// Extractor pulling the column value out of `T`. `None` is written as
    /// SQL `NULL`.
    pub extractor: fn(&T) -> Option<String>,

    /// If `true`, `column` is included in a table-level `UNIQUE(...)`
    /// constraint. `SQLite` treats `NULL`s as distinct in `UNIQUE`, so
    /// nullable unique columns only enforce uniqueness among non-`NULL`
    /// values. Callers needing "unique including `NULL`" must enforce it in
    /// application code (see `environments.rs` for the pattern).
    pub unique: bool,
}

/// Full table specification: name + the ordered list of indexed columns.
///
/// Shared across open/reopen so a downstream shim can keep the same layout
/// forever without duplicating strings.
///
/// The `'static` bound on `T` is required because `indexes` is a
/// `&'static [IndexSpec<T>]` — the compiler needs to know any `T` referenced
/// by the static slice is itself valid for `'static`. Every concrete record
/// type we store (e.g. `StoredNotifier`) owns its data, so this bound is
/// always satisfied in practice.
pub struct JsonTable<T: 'static> {
    /// SQL table name. Must be a valid SQL identifier; splat directly into
    /// DDL, so this MUST be a compile-time string literal.
    pub name: &'static str,

    /// Indexed columns. May be empty for pure blob-only tables.
    pub indexes: &'static [IndexSpec<T>],

    /// Compound `UNIQUE(col_a, col_b, ...)` constraints emitted at
    /// `CREATE TABLE` time. Each inner slice becomes one `UNIQUE(...)` clause
    /// in the generated DDL. For single-column uniqueness prefer
    /// `IndexSpec::unique = true` — it's flatter. Use this field when
    /// uniqueness spans two or more columns (e.g. `UNIQUE(name, scope)`).
    ///
    /// Every column name referenced here MUST also appear in `indexes` so
    /// that it's peeled out of `T` at write time; otherwise the `CREATE TABLE`
    /// statement will reference a column that doesn't exist. The column names
    /// are splat directly into the DDL, so they MUST be compile-time string
    /// literals. May be empty when no compound constraints are needed.
    pub unique_constraints: &'static [&'static [&'static str]],
}

/// Generic `SQLite` blob store keyed by a string `id`.
///
/// Use via a narrow wrapper type in each storage module (see
/// `notifiers.rs::SqlxNotifierStore` for the pattern).
pub struct SqlxJsonStore<T>
where
    T: Serialize + DeserializeOwned + Send + Sync + 'static,
{
    pool: SqlitePool,
    table: JsonTable<T>,
    _marker: PhantomData<fn() -> T>,
}

impl<T> SqlxJsonStore<T>
where
    T: Serialize + DeserializeOwned + Send + Sync + 'static,
{
    /// Open or create a `SQLite` database at the given path and ensure the
    /// table + indexes exist.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the database cannot be opened or if the
    /// schema cannot be created.
    pub async fn open<P: AsRef<Path>>(path: P, table: JsonTable<T>) -> Result<Self, StorageError> {
        let path_str = path.as_ref().display().to_string();
        let connection_string = format!("sqlite:{path_str}?mode=rwc");

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&connection_string)
            .await?;

        sqlx::query("PRAGMA journal_mode=WAL")
            .execute(&pool)
            .await?;
        sqlx::query("PRAGMA busy_timeout=5000")
            .execute(&pool)
            .await?;

        let this = Self {
            pool,
            table,
            _marker: PhantomData,
        };
        this.init_schema().await?;
        Ok(this)
    }

    /// Create an in-memory `SQLite` database (useful for tests) with the
    /// table + indexes in place.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the in-memory database cannot be created.
    pub async fn in_memory(table: JsonTable<T>) -> Result<Self, StorageError> {
        let pool = SqlitePool::connect(":memory:").await?;
        let this = Self {
            pool,
            table,
            _marker: PhantomData,
        };
        this.init_schema().await?;
        Ok(this)
    }

    /// Create the table + per-index secondary indexes if they don't exist.
    async fn init_schema(&self) -> Result<(), StorageError> {
        let table_name = self.table.name;

        // Build the CREATE TABLE statement. Every peeled column is `TEXT`
        // nullable; the blob is `data_json TEXT NOT NULL`; the timestamps
        // are ISO-8601 RFC-3339 strings.
        let mut ddl = String::with_capacity(256);
        ddl.push_str("CREATE TABLE IF NOT EXISTS ");
        ddl.push_str(table_name);
        ddl.push_str(" (\n    id TEXT PRIMARY KEY NOT NULL");

        for idx in self.table.indexes {
            ddl.push_str(",\n    ");
            ddl.push_str(idx.column);
            ddl.push_str(" TEXT");
        }

        ddl.push_str(",\n    data_json TEXT NOT NULL");
        ddl.push_str(",\n    created_at TEXT NOT NULL");
        ddl.push_str(",\n    updated_at TEXT NOT NULL");

        // Compound UNIQUE clause across all `unique: true` columns, matching
        // the `users.rs` / `environments.rs` shape.
        let unique_cols: Vec<&'static str> = self
            .table
            .indexes
            .iter()
            .filter(|i| i.unique)
            .map(|i| i.column)
            .collect();
        if !unique_cols.is_empty() {
            ddl.push_str(",\n    UNIQUE(");
            for (i, col) in unique_cols.iter().enumerate() {
                if i > 0 {
                    ddl.push_str(", ");
                }
                ddl.push_str(col);
            }
            ddl.push(')');
        }

        // Extra compound UNIQUE constraints — one clause per inner slice.
        // Every referenced column must exist in `indexes` so it's actually
        // peeled out of `T` at write time.
        let indexed_columns: std::collections::HashSet<&'static str> =
            self.table.indexes.iter().map(|i| i.column).collect();
        for constraint in self.table.unique_constraints {
            if constraint.is_empty() {
                continue;
            }
            for col in *constraint {
                debug_assert!(
                    indexed_columns.contains(col),
                    "unique_constraints references column '{col}' on table \
                     '{table_name}' that is not declared in JsonTable::indexes"
                );
            }
            ddl.push_str(",\n    UNIQUE(");
            for (i, col) in constraint.iter().enumerate() {
                if i > 0 {
                    ddl.push_str(", ");
                }
                ddl.push_str(col);
            }
            ddl.push(')');
        }
        ddl.push_str("\n)");

        sqlx::query(&ddl).execute(&self.pool).await?;

        // Secondary indexes on every peeled column (including unique ones)
        // for fast `get_by_unique` lookup.
        for idx in self.table.indexes {
            let idx_ddl = format!(
                "CREATE INDEX IF NOT EXISTS idx_{table}_{col} ON {table}({col})",
                table = table_name,
                col = idx.column,
            );
            sqlx::query(&idx_ddl).execute(&self.pool).await?;
        }

        Ok(())
    }

    /// Upsert a record by its primary key `id`.
    ///
    /// - If `id` is new, a row is inserted with `created_at = updated_at =
    ///   now` — both timestamps are taken from `now()` at write time, NOT
    ///   from any field on `T`, because the adapter does not know which
    ///   field (if any) of `T` holds a timestamp.
    /// - If `id` already exists, every column is refreshed from the new
    ///   blob except `created_at`, which is preserved.
    ///
    /// # Errors
    ///
    /// - [`StorageError::AlreadyExists`] if the write violates a `UNIQUE`
    ///   constraint on one of the peeled columns.
    /// - [`StorageError::Serialization`] if `T` cannot be encoded to JSON.
    /// - [`StorageError::Database`] for any other `sqlx` error.
    pub async fn put(&self, id: &str, value: &T) -> Result<(), StorageError> {
        let data_json = serde_json::to_string(value)?;
        let now = chrono::Utc::now().to_rfc3339();
        let table_name = self.table.name;

        // Build column and placeholder lists. `?` for every value; `sqlx`
        // does the binding so there's no injection risk from the blob or
        // extracted columns.
        let mut columns: Vec<&str> = Vec::with_capacity(4 + self.table.indexes.len());
        columns.push("id");
        for idx in self.table.indexes {
            columns.push(idx.column);
        }
        columns.push("data_json");
        columns.push("created_at");
        columns.push("updated_at");

        let placeholders = (0..columns.len())
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(", ");

        // ON CONFLICT(id) update clause — everything except `created_at`.
        let update_assignments = {
            let mut parts: Vec<String> = Vec::new();
            for idx in self.table.indexes {
                parts.push(format!("{col} = excluded.{col}", col = idx.column));
            }
            parts.push("data_json = excluded.data_json".to_string());
            parts.push("updated_at = excluded.updated_at".to_string());
            parts.join(", ")
        };

        let sql = format!(
            "INSERT INTO {table} ({cols}) VALUES ({placeholders}) \
             ON CONFLICT(id) DO UPDATE SET {updates}",
            table = table_name,
            cols = columns.join(", "),
            placeholders = placeholders,
            updates = update_assignments,
        );

        let mut query = sqlx::query(&sql).bind(id);
        for idx in self.table.indexes {
            query = query.bind((idx.extractor)(value));
        }
        query = query.bind(&data_json).bind(&now).bind(&now);

        match query.execute(&self.pool).await {
            Ok(_) => Ok(()),
            Err(err) => Err(map_sqlx_err(err, self.table.name)),
        }
    }

    /// Fetch a record by primary key.
    ///
    /// # Errors
    ///
    /// - [`StorageError::Database`] on `sqlx` failure.
    /// - [`StorageError::Serialization`] if the blob cannot be decoded.
    pub async fn get(&self, id: &str) -> Result<Option<T>, StorageError> {
        let sql = format!(
            "SELECT data_json FROM {table} WHERE id = ?",
            table = self.table.name
        );

        let row: Option<(String,)> = sqlx::query_as(&sql)
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;

        match row {
            Some((data_json,)) => {
                let value: T = serde_json::from_str(&data_json)?;
                Ok(Some(value))
            }
            None => Ok(None),
        }
    }

    /// List every record in the table, ordered by `id` ascending.
    ///
    /// Most callers want a domain-specific ordering (e.g. notifier name),
    /// which they can do by sorting in memory after `list()` returns. Doing
    /// the sort here would require knowing which field of `T` to order by,
    /// which the adapter cannot know generically.
    ///
    /// # Errors
    ///
    /// - [`StorageError::Database`] on `sqlx` failure.
    /// - [`StorageError::Serialization`] if any blob cannot be decoded.
    pub async fn list(&self) -> Result<Vec<T>, StorageError> {
        let sql = format!(
            "SELECT data_json FROM {table} ORDER BY id ASC",
            table = self.table.name
        );

        let rows: Vec<(String,)> = sqlx::query_as(&sql).fetch_all(&self.pool).await?;

        let mut out = Vec::with_capacity(rows.len());
        for (data_json,) in rows {
            let value: T = serde_json::from_str(&data_json)?;
            out.push(value);
        }
        Ok(out)
    }

    /// Delete a record by primary key.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Database`] on `sqlx` failure.
    pub async fn delete(&self, id: &str) -> Result<bool, StorageError> {
        let sql = format!("DELETE FROM {table} WHERE id = ?", table = self.table.name);
        let result = sqlx::query(&sql).bind(id).execute(&self.pool).await?;
        Ok(result.rows_affected() > 0)
    }

    /// Fetch a record by one of its peeled secondary columns.
    ///
    /// The column must appear in the `JsonTable::indexes` list. Works for
    /// any indexed column — unique or non-unique. When the column is
    /// non-unique, the first matching row (by `rowid`) is returned.
    ///
    /// # Errors
    ///
    /// - [`StorageError::Other`] if `column` does not correspond to any
    ///   declared index.
    /// - [`StorageError::Database`] on `sqlx` failure.
    /// - [`StorageError::Serialization`] if the blob cannot be decoded.
    pub async fn get_by_unique(
        &self,
        column: &str,
        value: &str,
    ) -> Result<Option<T>, StorageError> {
        // Validate the column against the static index list — this is what
        // prevents SQL injection via `column`.
        let found = self.table.indexes.iter().any(|i| i.column == column);
        if !found {
            return Err(StorageError::Other(format!(
                "unknown column '{column}' for table '{table}'",
                table = self.table.name
            )));
        }

        let sql = format!(
            "SELECT data_json FROM {table} WHERE {col} = ? LIMIT 1",
            table = self.table.name,
            col = column,
        );

        let row: Option<(String,)> = sqlx::query_as(&sql)
            .bind(value)
            .fetch_optional(&self.pool)
            .await?;

        match row {
            Some((data_json,)) => {
                let value: T = serde_json::from_str(&data_json)?;
                Ok(Some(value))
            }
            None => Ok(None),
        }
    }

    /// List every record where a peeled column matches a specific value.
    /// Rows are ordered by `id` ascending, matching [`list`](Self::list).
    ///
    /// Use [`list_where_opt`](Self::list_where_opt) if you need to filter
    /// on `NULL` — passing `""` to this function binds the empty string, not
    /// SQL `NULL`.
    ///
    /// # Errors
    ///
    /// - [`StorageError::Other`] if `column` is not declared in the table's
    ///   index list.
    /// - [`StorageError::Database`] on `sqlx` failure.
    /// - [`StorageError::Serialization`] if any blob cannot be decoded.
    pub async fn list_where(&self, column: &str, value: &str) -> Result<Vec<T>, StorageError> {
        self.list_where_opt(column, Some(value)).await
    }

    /// List every record where a peeled column is SQL `NULL`. Rows are
    /// ordered by `id` ascending.
    ///
    /// # Errors
    ///
    /// - [`StorageError::Other`] if `column` is not declared in the table's
    ///   index list.
    /// - [`StorageError::Database`] on `sqlx` failure.
    /// - [`StorageError::Serialization`] if any blob cannot be decoded.
    pub async fn list_where_null(&self, column: &str) -> Result<Vec<T>, StorageError> {
        self.list_where_opt(column, None).await
    }

    /// List every record filtered by a peeled column. `Some(v)` matches
    /// `column = ?` with `v` bound; `None` matches `column IS NULL`. Rows are
    /// ordered by `id` ascending.
    ///
    /// This is the unified primitive used by both [`list_where`](Self::list_where)
    /// and [`list_where_null`](Self::list_where_null).
    ///
    /// # Errors
    ///
    /// - [`StorageError::Other`] if `column` is not declared in the table's
    ///   index list.
    /// - [`StorageError::Database`] on `sqlx` failure.
    /// - [`StorageError::Serialization`] if any blob cannot be decoded.
    pub async fn list_where_opt(
        &self,
        column: &str,
        value: Option<&str>,
    ) -> Result<Vec<T>, StorageError> {
        // Validate the column against the static index list — this is what
        // prevents SQL injection via `column`.
        let found = self.table.indexes.iter().any(|i| i.column == column);
        if !found {
            return Err(StorageError::Other(format!(
                "unknown column '{column}' for table '{table}'",
                table = self.table.name
            )));
        }

        let rows: Vec<(String,)> = if let Some(v) = value {
            let sql = format!(
                "SELECT data_json FROM {table} WHERE {col} = ? ORDER BY id ASC",
                table = self.table.name,
                col = column,
            );
            sqlx::query_as(&sql).bind(v).fetch_all(&self.pool).await?
        } else {
            let sql = format!(
                "SELECT data_json FROM {table} WHERE {col} IS NULL ORDER BY id ASC",
                table = self.table.name,
                col = column,
            );
            sqlx::query_as(&sql).fetch_all(&self.pool).await?
        };

        let mut out = Vec::with_capacity(rows.len());
        for (data_json,) in rows {
            let decoded: T = serde_json::from_str(&data_json)?;
            out.push(decoded);
        }
        Ok(out)
    }

    /// Return a reference to the underlying `SqlitePool`.
    ///
    /// Exposed at crate scope so sibling stores in `storage::*` can run
    /// auxiliary queries — creating secondary tables (e.g. `task_runs`),
    /// doing cross-table transactions during cascade deletes — against the
    /// same pool without opening a second connection. Not part of the public
    /// API: no code outside `crates/zlayer-api/src/storage/` should need to
    /// reach the raw pool.
    pub(crate) fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Return the total number of rows in the table.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Database`] on `sqlx` failure.
    pub async fn count(&self) -> Result<u64, StorageError> {
        let sql = format!("SELECT COUNT(*) FROM {table}", table = self.table.name);
        let row = sqlx::query(&sql).fetch_one(&self.pool).await?;
        let raw: i64 = row.try_get(0)?;
        Ok(u64::try_from(raw).unwrap_or(0))
    }
}

/// Map a raw `sqlx::Error` into a `StorageError`, surfacing `SQLite` 2067
/// (`SQLITE_CONSTRAINT_UNIQUE`) / 1555 (`SQLITE_CONSTRAINT_PRIMARYKEY`) as a
/// dedicated `AlreadyExists` variant and everything else as a generic
/// `Database`.
fn map_sqlx_err(err: sqlx::Error, table: &str) -> StorageError {
    if let sqlx::Error::Database(db) = &err {
        // SQLite error codes are returned as strings by `sqlx`. 2067 is the
        // extended code for UNIQUE; 1555 is the extended code for PRIMARY
        // KEY. Both are constraint violations that a caller typically wants
        // to render as HTTP 409.
        if let Some(code) = db.code() {
            if code == "2067" || code == "1555" {
                return StorageError::AlreadyExists(format!(
                    "UNIQUE constraint failed on table '{table}': {db}"
                ));
            }
        }
        // Some SQLite builds don't populate extended codes; fall back to
        // substring matching on the message for those cases.
        let msg = db.message();
        if msg.contains("UNIQUE constraint failed") {
            return StorageError::AlreadyExists(format!(
                "UNIQUE constraint failed on table '{table}': {msg}"
            ));
        }
    }
    StorageError::from(err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    /// Minimal record type exercising both a unique and a non-unique indexed
    /// column plus the blob round-trip.
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    struct TestRecord {
        id: String,
        name: String,
        value: String,
    }

    /// Table spec with a unique `name` column and a non-unique `value`
    /// column — covers both branches of the index machinery.
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

    /// Table spec with no secondary columns — matches the notifier shape.
    fn blob_only_table() -> JsonTable<TestRecord> {
        JsonTable {
            name: "test_blobs",
            indexes: &[],
            unique_constraints: &[],
        }
    }

    /// Record with two peeled columns exercising a compound UNIQUE(name,
    /// scope) constraint — the shape used by the variable store.
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

    fn make(id: &str, name: &str, value: &str) -> TestRecord {
        TestRecord {
            id: id.to_string(),
            name: name.to_string(),
            value: value.to_string(),
        }
    }

    #[tokio::test]
    async fn in_memory_round_trip_with_indexes() {
        let store = SqlxJsonStore::in_memory(indexed_table()).await.unwrap();
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
        let store = SqlxJsonStore::in_memory(blob_only_table()).await.unwrap();
        let rec = make("b-1", "bob", "42");

        store.put(&rec.id, &rec).await.unwrap();
        let got = store.get("b-1").await.unwrap().expect("must exist");
        assert_eq!(got, rec);

        let list = store.list().await.unwrap();
        assert_eq!(list, vec![rec]);
    }

    #[tokio::test]
    async fn unique_conflict_surfaces_as_already_exists() {
        let store = SqlxJsonStore::in_memory(indexed_table()).await.unwrap();
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
            }
            other => panic!("expected AlreadyExists, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn get_by_unique_success_and_unknown_column() {
        let store = SqlxJsonStore::in_memory(indexed_table()).await.unwrap();
        store.put("x", &make("x", "name-x", "val-x")).await.unwrap();
        store.put("y", &make("y", "name-y", "val-y")).await.unwrap();

        // Unique column lookup.
        let found = store
            .get_by_unique("name", "name-x")
            .await
            .unwrap()
            .expect("must exist");
        assert_eq!(found.id, "x");

        // Non-unique column lookup still works.
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
    async fn upsert_preserves_created_at() {
        let store = SqlxJsonStore::in_memory(indexed_table()).await.unwrap();
        let rec = make("z", "z-name", "v1");
        store.put(&rec.id, &rec).await.unwrap();

        // Read back the raw created_at / updated_at columns so we can
        // assert the adapter's timestamp policy (created_at is preserved
        // across upserts, updated_at is refreshed).
        let (created_before, updated_before): (String, String) =
            sqlx::query_as("SELECT created_at, updated_at FROM test_records WHERE id = ?")
                .bind("z")
                .fetch_one(&store.pool)
                .await
                .unwrap();

        // Tiny pause so the RFC-3339 strings differ.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let updated = make("z", "z-name", "v2");
        store.put(&updated.id, &updated).await.unwrap();

        let (created_after, updated_after): (String, String) =
            sqlx::query_as("SELECT created_at, updated_at FROM test_records WHERE id = ?")
                .bind("z")
                .fetch_one(&store.pool)
                .await
                .unwrap();

        assert_eq!(
            created_before, created_after,
            "created_at must be preserved across upsert"
        );
        assert_ne!(
            updated_before, updated_after,
            "updated_at must advance on upsert"
        );

        // And the blob really did update.
        let got = store.get("z").await.unwrap().unwrap();
        assert_eq!(got.value, "v2");
    }

    #[tokio::test]
    async fn count_tracks_inserts_and_deletes() {
        let store = SqlxJsonStore::in_memory(indexed_table()).await.unwrap();
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
    async fn list_returns_every_row() {
        let store = SqlxJsonStore::in_memory(blob_only_table()).await.unwrap();
        store.put("c", &make("c", "cc", "3")).await.unwrap();
        store.put("a", &make("a", "aa", "1")).await.unwrap();
        store.put("b", &make("b", "bb", "2")).await.unwrap();

        let list = store.list().await.unwrap();
        assert_eq!(list.len(), 3);
        // Ordered by id ASC, as documented.
        let ids: Vec<&str> = list.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b", "c"]);
    }

    // =========================================================================
    // Compound UNIQUE + list_where{,_null,_opt} tests
    // =========================================================================

    #[tokio::test]
    async fn compound_unique_rejects_duplicate_pair() {
        let store = SqlxJsonStore::in_memory(scoped_table()).await.unwrap();
        store
            .put("id-1", &make_scoped("id-1", "foo", Some("proj-a")))
            .await
            .unwrap();

        // Same (name, scope) under a different id must fail with AlreadyExists.
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
        let store = SqlxJsonStore::in_memory(scoped_table()).await.unwrap();
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
    async fn list_where_filters_by_column_value() {
        let store = SqlxJsonStore::in_memory(scoped_table()).await.unwrap();
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
        // Ordered by id ASC.
        assert_eq!(ids, vec!["a", "b"]);

        let s2 = store.list_where("scope", "s2").await.unwrap();
        assert_eq!(s2.len(), 1);
        assert_eq!(s2[0].id, "c");

        let missing = store.list_where("scope", "s3").await.unwrap();
        assert!(missing.is_empty());
    }

    #[tokio::test]
    async fn list_where_null_filters_null_rows() {
        let store = SqlxJsonStore::in_memory(scoped_table()).await.unwrap();
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
        let store = SqlxJsonStore::in_memory(scoped_table()).await.unwrap();
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
