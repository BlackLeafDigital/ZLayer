//! Compose-project storage trait and backends.
//!
//! Provides persistence for projects launched via `zlayer compose up` so that
//! `compose down` and `compose ls` can later look up:
//!
//! - the working directory the user invoked `up` from,
//! - which compose YAML files were merged together,
//! - which `--profile` filters were active,
//! - which `--env-file` paths were layered in,
//! - the resulting service set, and
//! - the concrete container ids the daemon actually spawned.
//!
//! Two implementations are shipped, mirroring the `standalone_container`
//! module:
//!
//! - [`InMemoryComposeProjectStorage`] — `RwLock<HashMap>`-backed, used for
//!   tests and for daemons running with persistent state disabled.
//! - [`SqliteComposeProjectStorage`] — `sqlx`-backed `SQLite` store that
//!   persists records across daemon restarts. Path-and-list fields are
//!   JSON-encoded into single `TEXT` columns; the project name is the
//!   primary key.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use tokio::sync::RwLock;

use super::StorageError;

/// A compose project record.
///
/// Records everything `compose up` needs to remember so a later
/// `compose down` (potentially from a different cwd) can reconstruct the
/// project's identity and tear down exactly the containers it spawned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposeProject {
    /// Project name. Unique across the daemon — primary key in the SQL
    /// backend, hash key in the in-memory backend.
    pub name: String,

    /// The working directory `compose up` was invoked from. Stored so that
    /// relative paths inside the compose file (volumes, build contexts) can
    /// be resolved on `compose down` even if the caller's cwd has changed.
    pub working_dir: PathBuf,

    /// Compose files that were merged to produce this project, in the order
    /// they were applied (later files override earlier ones).
    pub files: Vec<PathBuf>,

    /// Active `--profile` filters at `compose up` time. Empty means no
    /// profile filter was applied.
    pub profiles: Vec<String>,

    /// `--env-file` paths layered into the compose run.
    pub env_files: Vec<PathBuf>,

    /// Service names this project produced (one entry per service in the
    /// merged compose graph that survived profile filtering).
    pub services: Vec<String>,

    /// Concrete container ids spawned for this project. These are the strings
    /// the runtime returned, suitable for direct use against the agent's
    /// container API.
    pub container_ids: Vec<String>,

    /// Timestamp the project was first recorded.
    pub created_at: DateTime<Utc>,
}

/// Trait for compose-project storage backends.
///
/// The unique key is [`ComposeProject::name`].
#[async_trait]
pub trait ComposeProjectStorage: Send + Sync + 'static {
    /// Insert a new project, or replace any existing record with the same
    /// `name`. The in-memory and SQL backends both implement this as an
    /// unconditional upsert keyed on `name`.
    async fn upsert(&self, project: ComposeProject) -> Result<(), StorageError>;

    /// Fetch a project by name. Returns `None` if no record exists.
    async fn get(&self, name: &str) -> Result<Option<ComposeProject>, StorageError>;

    /// List every recorded compose project.
    async fn list(&self) -> Result<Vec<ComposeProject>, StorageError>;

    /// Delete a project by name. Returns `true` if a record existed and was
    /// removed, `false` otherwise.
    async fn delete(&self, name: &str) -> Result<bool, StorageError>;
}

// =============================================================================
// In-memory backend
// =============================================================================

/// In-memory compose-project storage backed by an `RwLock<HashMap>`.
///
/// Useful for tests and for hosts that disable persistent state.
pub struct InMemoryComposeProjectStorage {
    inner: Arc<RwLock<HashMap<String, ComposeProject>>>,
}

impl InMemoryComposeProjectStorage {
    /// Create a new empty in-memory compose-project store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Default for InMemoryComposeProjectStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ComposeProjectStorage for InMemoryComposeProjectStorage {
    async fn upsert(&self, project: ComposeProject) -> Result<(), StorageError> {
        let mut map = self.inner.write().await;
        map.insert(project.name.clone(), project);
        Ok(())
    }

    async fn get(&self, name: &str) -> Result<Option<ComposeProject>, StorageError> {
        let map = self.inner.read().await;
        Ok(map.get(name).cloned())
    }

    async fn list(&self) -> Result<Vec<ComposeProject>, StorageError> {
        let map = self.inner.read().await;
        Ok(map.values().cloned().collect())
    }

    async fn delete(&self, name: &str) -> Result<bool, StorageError> {
        let mut map = self.inner.write().await;
        Ok(map.remove(name).is_some())
    }
}

// =============================================================================
// SQLite (sqlx) backend
// =============================================================================

/// `SQLite`/`sqlx` compose-project storage.
///
/// Records are keyed by [`ComposeProject::name`]. Path-and-list fields
/// (`files`, `profiles`, `env_files`, `services`, `container_ids`) are
/// JSON-encoded into single `TEXT` columns rather than normalized into
/// child tables — these lists are always read and written as a unit and
/// no compose query needs to filter by, e.g., a single contained file path.
///
/// Schema:
///
/// ```sql
/// CREATE TABLE IF NOT EXISTS compose_projects (
///     name              TEXT PRIMARY KEY NOT NULL,
///     working_dir       TEXT NOT NULL,
///     files_json        TEXT NOT NULL,
///     profiles_json     TEXT NOT NULL,
///     env_files_json    TEXT NOT NULL,
///     services_json     TEXT NOT NULL,
///     container_ids_json TEXT NOT NULL,
///     created_at        TEXT NOT NULL
/// )
/// ```
pub struct SqliteComposeProjectStorage {
    pool: SqlitePool,
}

impl SqliteComposeProjectStorage {
    /// Open or create a `SQLite` database at the given path and ensure the
    /// `compose_projects` table exists.
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

        // Match the durability/concurrency settings used elsewhere in this
        // module so multiple stores running against the same database file
        // behave consistently.
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
            CREATE TABLE IF NOT EXISTS compose_projects (
                name               TEXT PRIMARY KEY NOT NULL,
                working_dir        TEXT NOT NULL,
                files_json         TEXT NOT NULL,
                profiles_json      TEXT NOT NULL,
                env_files_json     TEXT NOT NULL,
                services_json      TEXT NOT NULL,
                container_ids_json TEXT NOT NULL,
                created_at         TEXT NOT NULL
            )
            ",
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Encode a `Vec<PathBuf>` to JSON. Paths are serialized via their
    /// `Serialize` impl, which uses the OS-native string form — round-trips
    /// losslessly on the same platform, which is the only contract we offer.
    fn encode_paths(paths: &[PathBuf]) -> Result<String, StorageError> {
        serde_json::to_string(paths).map_err(StorageError::from)
    }

    /// Decode a JSON `Vec<PathBuf>` column back.
    fn decode_paths(json: &str) -> Result<Vec<PathBuf>, StorageError> {
        serde_json::from_str(json).map_err(StorageError::from)
    }

    /// Encode a `Vec<String>` (profiles, services, `container_ids`) to JSON.
    fn encode_strings(strings: &[String]) -> Result<String, StorageError> {
        serde_json::to_string(strings).map_err(StorageError::from)
    }

    /// Decode a JSON `Vec<String>` column back.
    fn decode_strings(json: &str) -> Result<Vec<String>, StorageError> {
        serde_json::from_str(json).map_err(StorageError::from)
    }

    /// Reconstruct a `ComposeProject` from its row tuple.
    #[allow(clippy::too_many_arguments)]
    fn row_to_project(
        name: String,
        working_dir: String,
        files_json: &str,
        profiles_json: &str,
        env_files_json: &str,
        services_json: &str,
        container_ids_json: &str,
        created_at: &str,
    ) -> Result<ComposeProject, StorageError> {
        let created_at = DateTime::parse_from_rfc3339(created_at)
            .map_err(|e| {
                StorageError::Other(format!(
                    "compose_projects.created_at not RFC3339 ({created_at}): {e}"
                ))
            })?
            .with_timezone(&Utc);

        Ok(ComposeProject {
            name,
            working_dir: PathBuf::from(working_dir),
            files: Self::decode_paths(files_json)?,
            profiles: Self::decode_strings(profiles_json)?,
            env_files: Self::decode_paths(env_files_json)?,
            services: Self::decode_strings(services_json)?,
            container_ids: Self::decode_strings(container_ids_json)?,
            created_at,
        })
    }
}

#[async_trait]
impl ComposeProjectStorage for SqliteComposeProjectStorage {
    async fn upsert(&self, project: ComposeProject) -> Result<(), StorageError> {
        let working_dir = project.working_dir.display().to_string();
        let files_json = Self::encode_paths(&project.files)?;
        let profiles_json = Self::encode_strings(&project.profiles)?;
        let env_files_json = Self::encode_paths(&project.env_files)?;
        let services_json = Self::encode_strings(&project.services)?;
        let container_ids_json = Self::encode_strings(&project.container_ids)?;
        let created_at = project.created_at.to_rfc3339();

        sqlx::query(
            r"
            INSERT OR REPLACE INTO compose_projects
                (name, working_dir, files_json, profiles_json, env_files_json,
                 services_json, container_ids_json, created_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            ",
        )
        .bind(&project.name)
        .bind(&working_dir)
        .bind(&files_json)
        .bind(&profiles_json)
        .bind(&env_files_json)
        .bind(&services_json)
        .bind(&container_ids_json)
        .bind(&created_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn get(&self, name: &str) -> Result<Option<ComposeProject>, StorageError> {
        let row: Option<(
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
        )> = sqlx::query_as(
            r"
                SELECT name, working_dir, files_json, profiles_json, env_files_json,
                       services_json, container_ids_json, created_at
                FROM compose_projects
                WHERE name = ?
                ",
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some((
                name,
                working_dir,
                files_json,
                profiles_json,
                env_files_json,
                services_json,
                container_ids_json,
                created_at,
            )) => Ok(Some(Self::row_to_project(
                name,
                working_dir,
                &files_json,
                &profiles_json,
                &env_files_json,
                &services_json,
                &container_ids_json,
                &created_at,
            )?)),
            None => Ok(None),
        }
    }

    async fn list(&self) -> Result<Vec<ComposeProject>, StorageError> {
        let rows: Vec<(
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
        )> = sqlx::query_as(
            r"
                SELECT name, working_dir, files_json, profiles_json, env_files_json,
                       services_json, container_ids_json, created_at
                FROM compose_projects
                ORDER BY name ASC
                ",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut out = Vec::with_capacity(rows.len());
        for (
            name,
            working_dir,
            files_json,
            profiles_json,
            env_files_json,
            services_json,
            container_ids_json,
            created_at,
        ) in rows
        {
            out.push(Self::row_to_project(
                name,
                working_dir,
                &files_json,
                &profiles_json,
                &env_files_json,
                &services_json,
                &container_ids_json,
                &created_at,
            )?);
        }
        Ok(out)
    }

    async fn delete(&self, name: &str) -> Result<bool, StorageError> {
        let result = sqlx::query("DELETE FROM compose_projects WHERE name = ?")
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_project(name: &str) -> ComposeProject {
        ComposeProject {
            name: name.to_string(),
            working_dir: PathBuf::from(format!("/home/zach/projects/{name}")),
            files: vec![
                PathBuf::from("compose.yaml"),
                PathBuf::from("compose.override.yaml"),
            ],
            profiles: vec!["dev".to_string(), "debug".to_string()],
            env_files: vec![PathBuf::from(".env"), PathBuf::from(".env.local")],
            services: vec!["api".to_string(), "db".to_string()],
            container_ids: vec![format!("{name}-api-0"), format!("{name}-db-0")],
            // Use a fixed timestamp so equality assertions are stable across runs.
            created_at: DateTime::parse_from_rfc3339("2026-05-03T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        }
    }

    // --------------------------------------------------------------------- //
    // InMemoryComposeProjectStorage
    // --------------------------------------------------------------------- //

    #[tokio::test]
    async fn in_memory_upsert_get_round_trip() {
        let store = InMemoryComposeProjectStorage::new();
        let project = make_project("alpha");

        store.upsert(project.clone()).await.unwrap();

        let got = store.get("alpha").await.unwrap().expect("project present");
        assert_eq!(got, project);
    }

    #[tokio::test]
    async fn in_memory_get_missing_returns_none() {
        let store = InMemoryComposeProjectStorage::new();
        assert!(store.get("ghost").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn in_memory_list_includes_all_inserted() {
        let store = InMemoryComposeProjectStorage::new();
        let a = make_project("alpha");
        let b = make_project("bravo");
        let c = make_project("charlie");

        store.upsert(a.clone()).await.unwrap();
        store.upsert(b.clone()).await.unwrap();
        store.upsert(c.clone()).await.unwrap();

        let mut listed = store.list().await.unwrap();
        listed.sort_by(|x, y| x.name.cmp(&y.name));

        assert_eq!(listed, vec![a, b, c]);
    }

    #[tokio::test]
    async fn in_memory_upsert_overwrites_existing() {
        let store = InMemoryComposeProjectStorage::new();
        let mut project = make_project("alpha");

        store.upsert(project.clone()).await.unwrap();

        // Mutate non-key fields and upsert again.
        project.container_ids = vec!["alpha-api-1".to_string()];
        project.services = vec!["api".to_string()];
        store.upsert(project.clone()).await.unwrap();

        let got = store.get("alpha").await.unwrap().expect("project present");
        assert_eq!(got.container_ids, vec!["alpha-api-1".to_string()]);
        assert_eq!(got.services, vec!["api".to_string()]);

        // No duplicate row.
        assert_eq!(store.list().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn in_memory_delete_returns_correct_bool() {
        let store = InMemoryComposeProjectStorage::new();
        let project = make_project("alpha");

        store.upsert(project).await.unwrap();
        assert!(store.delete("alpha").await.unwrap(), "first delete removes");
        assert!(
            !store.delete("alpha").await.unwrap(),
            "second delete reports absent"
        );
        assert!(!store.delete("never-existed").await.unwrap());
        assert!(store.get("alpha").await.unwrap().is_none());
    }

    // --------------------------------------------------------------------- //
    // SqliteComposeProjectStorage
    // --------------------------------------------------------------------- //

    #[tokio::test]
    async fn sqlite_upsert_get_round_trip_preserves_all_fields() {
        let store = SqliteComposeProjectStorage::in_memory().await.unwrap();
        let project = make_project("alpha");

        store.upsert(project.clone()).await.unwrap();

        let got = store.get("alpha").await.unwrap().expect("project present");
        assert_eq!(got, project);
    }

    #[tokio::test]
    async fn sqlite_list_orders_by_name_ascending() {
        let store = SqliteComposeProjectStorage::in_memory().await.unwrap();

        store.upsert(make_project("charlie")).await.unwrap();
        store.upsert(make_project("alpha")).await.unwrap();
        store.upsert(make_project("bravo")).await.unwrap();

        let listed = store.list().await.unwrap();
        let names: Vec<String> = listed.iter().map(|p| p.name.clone()).collect();
        assert_eq!(
            names,
            vec![
                "alpha".to_string(),
                "bravo".to_string(),
                "charlie".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn sqlite_upsert_overwrites_existing_row() {
        let store = SqliteComposeProjectStorage::in_memory().await.unwrap();
        let mut project = make_project("alpha");

        store.upsert(project.clone()).await.unwrap();

        project.container_ids = vec!["alpha-api-99".to_string()];
        project.profiles = vec!["prod".to_string()];
        store.upsert(project.clone()).await.unwrap();

        let got = store.get("alpha").await.unwrap().expect("project present");
        assert_eq!(got.container_ids, vec!["alpha-api-99".to_string()]);
        assert_eq!(got.profiles, vec!["prod".to_string()]);

        // Update must overwrite, not duplicate.
        let all = store.list().await.unwrap();
        assert_eq!(all.len(), 1);
    }

    #[tokio::test]
    async fn sqlite_delete_returns_correct_bool() {
        let store = SqliteComposeProjectStorage::in_memory().await.unwrap();
        let project = make_project("alpha");

        store.upsert(project).await.unwrap();
        assert!(store.delete("alpha").await.unwrap(), "first delete removes");
        // Second delete must report no row — proves we are inspecting actual
        // rows_affected and not always reporting success.
        assert!(
            !store.delete("alpha").await.unwrap(),
            "second delete reports absent"
        );
        assert!(!store.delete("never-existed").await.unwrap());
        assert!(store.get("alpha").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn sqlite_persists_across_reopen() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("compose-projects.db");
        let project = make_project("persist");

        // Write through one handle...
        {
            let store = SqliteComposeProjectStorage::open(&db_path).await.unwrap();
            store.upsert(project.clone()).await.unwrap();
        }

        // ...and read through a fresh handle. Proves the schema and rows
        // survive a process-style restart, not just an in-memory cache.
        {
            let store = SqliteComposeProjectStorage::open(&db_path).await.unwrap();
            let got = store
                .get("persist")
                .await
                .unwrap()
                .expect("project present after reopen");
            assert_eq!(got, project);
        }
    }

    /// Pin every JSON-encoded list column round-trips through `SQLite`. A
    /// regression in `encode_paths` / `decode_strings` would silently lose
    /// the user's compose-file list or container-id set, making `compose
    /// down` a no-op without surfacing an error.
    #[tokio::test]
    async fn sqlite_json_columns_round_trip() {
        let store = SqliteComposeProjectStorage::in_memory().await.unwrap();

        // Use lists of varied length, including empties, to exercise both
        // the populated and empty `[]` JSON paths.
        let project = ComposeProject {
            name: "json-rt".to_string(),
            working_dir: PathBuf::from("/home/zach/projects/json-rt"),
            files: vec![
                PathBuf::from("a.yaml"),
                PathBuf::from("b.yaml"),
                PathBuf::from("c.yaml"),
            ],
            profiles: vec![],
            env_files: vec![PathBuf::from(".env")],
            services: vec!["s1".to_string(), "s2".to_string(), "s3".to_string()],
            container_ids: vec![],
            created_at: DateTime::parse_from_rfc3339("2026-05-03T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        };

        store.upsert(project.clone()).await.unwrap();
        let got = store
            .get("json-rt")
            .await
            .unwrap()
            .expect("project present");

        assert_eq!(got.files, project.files);
        assert_eq!(got.profiles, project.profiles);
        assert_eq!(got.env_files, project.env_files);
        assert_eq!(got.services, project.services);
        assert_eq!(got.container_ids, project.container_ids);
        assert_eq!(got, project);
    }
}
