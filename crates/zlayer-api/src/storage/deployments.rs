//! Deployment storage implementations
//!
//! Provides both persistent (redb) and in-memory storage backends.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use redb::{Database, ReadableTable, TableDefinition};
use thiserror::Error;
use tokio::sync::RwLock;

use super::StoredDeployment;

/// Table definition for deployments in redb
const DEPLOYMENTS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("deployments");

/// Storage errors
#[derive(Debug, Error)]
pub enum StorageError {
    /// Database error
    #[error("Database error: {0}")]
    Database(String),

    /// Serialization error
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// Deployment not found
    #[error("Deployment not found: {0}")]
    NotFound(String),

    /// IO error
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

impl From<redb::Error> for StorageError {
    fn from(err: redb::Error) -> Self {
        StorageError::Database(err.to_string())
    }
}

impl From<redb::DatabaseError> for StorageError {
    fn from(err: redb::DatabaseError) -> Self {
        StorageError::Database(err.to_string())
    }
}

impl From<redb::TableError> for StorageError {
    fn from(err: redb::TableError) -> Self {
        StorageError::Database(err.to_string())
    }
}

impl From<redb::TransactionError> for StorageError {
    fn from(err: redb::TransactionError) -> Self {
        StorageError::Database(err.to_string())
    }
}

impl From<redb::CommitError> for StorageError {
    fn from(err: redb::CommitError) -> Self {
        StorageError::Database(err.to_string())
    }
}

impl From<redb::StorageError> for StorageError {
    fn from(err: redb::StorageError) -> Self {
        StorageError::Database(err.to_string())
    }
}

impl From<serde_json::Error> for StorageError {
    fn from(err: serde_json::Error) -> Self {
        StorageError::Serialization(err.to_string())
    }
}

/// Trait for deployment storage backends
#[async_trait]
pub trait DeploymentStorage: Send + Sync {
    /// Store a deployment (creates or updates)
    async fn store(&self, deployment: &StoredDeployment) -> Result<(), StorageError>;

    /// Get a deployment by name
    async fn get(&self, name: &str) -> Result<Option<StoredDeployment>, StorageError>;

    /// List all deployments
    async fn list(&self) -> Result<Vec<StoredDeployment>, StorageError>;

    /// Delete a deployment by name, returns true if it existed
    async fn delete(&self, name: &str) -> Result<bool, StorageError>;

    /// Check if a deployment exists
    async fn exists(&self, name: &str) -> Result<bool, StorageError> {
        Ok(self.get(name).await?.is_some())
    }
}

/// Redb-based persistent storage for deployments
pub struct RedbStorage {
    db: Database,
}

impl RedbStorage {
    /// Open or create a redb database at the given path
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        let db = Database::create(path)?;

        // Initialize the table
        let write_txn = db.begin_write()?;
        {
            // Create the table if it doesn't exist
            let _table = write_txn.open_table(DEPLOYMENTS_TABLE)?;
        }
        write_txn.commit()?;

        Ok(Self { db })
    }

    /// Create an in-memory redb database (useful for testing)
    pub fn in_memory() -> Result<Self, StorageError> {
        let db = Database::builder().create_with_backend(redb::backends::InMemoryBackend::new())?;

        // Initialize the table
        let write_txn = db.begin_write()?;
        {
            let _table = write_txn.open_table(DEPLOYMENTS_TABLE)?;
        }
        write_txn.commit()?;

        Ok(Self { db })
    }
}

#[async_trait]
impl DeploymentStorage for RedbStorage {
    async fn store(&self, deployment: &StoredDeployment) -> Result<(), StorageError> {
        let name = deployment.name.clone();
        let data = serde_json::to_vec(deployment)?;

        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(DEPLOYMENTS_TABLE)?;
            table.insert(name.as_str(), data.as_slice())?;
        }
        write_txn.commit()?;

        Ok(())
    }

    async fn get(&self, name: &str) -> Result<Option<StoredDeployment>, StorageError> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(DEPLOYMENTS_TABLE)?;

        match table.get(name)? {
            Some(data) => {
                let deployment: StoredDeployment = serde_json::from_slice(data.value())?;
                Ok(Some(deployment))
            }
            None => Ok(None),
        }
    }

    async fn list(&self) -> Result<Vec<StoredDeployment>, StorageError> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(DEPLOYMENTS_TABLE)?;

        let mut deployments = Vec::new();
        for entry in table.iter()? {
            let (_, value) = entry?;
            let deployment: StoredDeployment = serde_json::from_slice(value.value())?;
            deployments.push(deployment);
        }

        // Sort by name for consistent ordering
        deployments.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(deployments)
    }

    async fn delete(&self, name: &str) -> Result<bool, StorageError> {
        let write_txn = self.db.begin_write()?;
        let existed = {
            let mut table = write_txn.open_table(DEPLOYMENTS_TABLE)?;
            let result = table.remove(name)?;
            result.is_some()
        };
        write_txn.commit()?;

        Ok(existed)
    }
}

/// In-memory storage for testing
pub struct InMemoryStorage {
    deployments: Arc<RwLock<HashMap<String, StoredDeployment>>>,
}

impl InMemoryStorage {
    /// Create a new empty in-memory storage
    pub fn new() -> Self {
        Self {
            deployments: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Default for InMemoryStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DeploymentStorage for InMemoryStorage {
    async fn store(&self, deployment: &StoredDeployment) -> Result<(), StorageError> {
        let mut deployments = self.deployments.write().await;
        deployments.insert(deployment.name.clone(), deployment.clone());
        Ok(())
    }

    async fn get(&self, name: &str) -> Result<Option<StoredDeployment>, StorageError> {
        let deployments = self.deployments.read().await;
        Ok(deployments.get(name).cloned())
    }

    async fn list(&self) -> Result<Vec<StoredDeployment>, StorageError> {
        let deployments = self.deployments.read().await;
        let mut list: Vec<_> = deployments.values().cloned().collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(list)
    }

    async fn delete(&self, name: &str) -> Result<bool, StorageError> {
        let mut deployments = self.deployments.write().await;
        Ok(deployments.remove(name).is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::DeploymentStatus;
    use std::collections::HashMap;
    use zlayer_spec::{DeploymentSpec, ImageSpec, ServiceSpec};

    fn create_test_spec(name: &str) -> DeploymentSpec {
        let mut services = HashMap::new();
        services.insert(
            "test-service".to_string(),
            ServiceSpec {
                rtype: zlayer_spec::ResourceType::Service,
                schedule: None,
                image: ImageSpec {
                    name: "test:latest".to_string(),
                    pull_policy: zlayer_spec::PullPolicy::IfNotPresent,
                },
                resources: Default::default(),
                env: Default::default(),
                command: Default::default(),
                network: Default::default(),
                endpoints: vec![],
                scale: Default::default(),
                depends: vec![],
                health: zlayer_spec::HealthSpec {
                    start_grace: None,
                    interval: None,
                    timeout: None,
                    retries: 3,
                    check: zlayer_spec::HealthCheck::Tcp { port: 8080 },
                },
                init: Default::default(),
                errors: Default::default(),
                devices: vec![],
                storage: vec![],
                capabilities: vec![],
                privileged: false,
                node_mode: Default::default(),
                node_selector: None,
            },
        );

        DeploymentSpec {
            version: "v1".to_string(),
            deployment: name.to_string(),
            services,
        }
    }

    fn create_test_deployment(name: &str) -> StoredDeployment {
        StoredDeployment::new(create_test_spec(name))
    }

    // =========================================================================
    // InMemoryStorage tests
    // =========================================================================

    #[tokio::test]
    async fn test_inmemory_store_and_get() {
        let storage = InMemoryStorage::new();
        let deployment = create_test_deployment("test-app");

        storage.store(&deployment).await.unwrap();

        let retrieved = storage.get("test-app").await.unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.name, "test-app");
        assert_eq!(retrieved.status, DeploymentStatus::Pending);
    }

    #[tokio::test]
    async fn test_inmemory_get_nonexistent() {
        let storage = InMemoryStorage::new();

        let result = storage.get("nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_inmemory_list() {
        let storage = InMemoryStorage::new();

        storage
            .store(&create_test_deployment("app-c"))
            .await
            .unwrap();
        storage
            .store(&create_test_deployment("app-a"))
            .await
            .unwrap();
        storage
            .store(&create_test_deployment("app-b"))
            .await
            .unwrap();

        let list = storage.list().await.unwrap();
        assert_eq!(list.len(), 3);
        // Should be sorted by name
        assert_eq!(list[0].name, "app-a");
        assert_eq!(list[1].name, "app-b");
        assert_eq!(list[2].name, "app-c");
    }

    #[tokio::test]
    async fn test_inmemory_delete() {
        let storage = InMemoryStorage::new();
        let deployment = create_test_deployment("test-app");

        storage.store(&deployment).await.unwrap();

        let deleted = storage.delete("test-app").await.unwrap();
        assert!(deleted);

        let retrieved = storage.get("test-app").await.unwrap();
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_inmemory_delete_nonexistent() {
        let storage = InMemoryStorage::new();

        let deleted = storage.delete("nonexistent").await.unwrap();
        assert!(!deleted);
    }

    #[tokio::test]
    async fn test_inmemory_exists() {
        let storage = InMemoryStorage::new();
        let deployment = create_test_deployment("test-app");

        assert!(!storage.exists("test-app").await.unwrap());

        storage.store(&deployment).await.unwrap();

        assert!(storage.exists("test-app").await.unwrap());
    }

    #[tokio::test]
    async fn test_inmemory_update() {
        let storage = InMemoryStorage::new();
        let mut deployment = create_test_deployment("test-app");

        storage.store(&deployment).await.unwrap();

        // Update the deployment
        deployment.update_status(DeploymentStatus::Running);
        storage.store(&deployment).await.unwrap();

        let retrieved = storage.get("test-app").await.unwrap().unwrap();
        assert_eq!(retrieved.status, DeploymentStatus::Running);
    }

    // =========================================================================
    // RedbStorage tests
    // =========================================================================

    #[tokio::test]
    async fn test_redb_store_and_get() {
        let storage = RedbStorage::in_memory().unwrap();
        let deployment = create_test_deployment("test-app");

        storage.store(&deployment).await.unwrap();

        let retrieved = storage.get("test-app").await.unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.name, "test-app");
        assert_eq!(retrieved.status, DeploymentStatus::Pending);
    }

    #[tokio::test]
    async fn test_redb_get_nonexistent() {
        let storage = RedbStorage::in_memory().unwrap();

        let result = storage.get("nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_redb_list() {
        let storage = RedbStorage::in_memory().unwrap();

        storage
            .store(&create_test_deployment("app-c"))
            .await
            .unwrap();
        storage
            .store(&create_test_deployment("app-a"))
            .await
            .unwrap();
        storage
            .store(&create_test_deployment("app-b"))
            .await
            .unwrap();

        let list = storage.list().await.unwrap();
        assert_eq!(list.len(), 3);
        // Should be sorted by name
        assert_eq!(list[0].name, "app-a");
        assert_eq!(list[1].name, "app-b");
        assert_eq!(list[2].name, "app-c");
    }

    #[tokio::test]
    async fn test_redb_delete() {
        let storage = RedbStorage::in_memory().unwrap();
        let deployment = create_test_deployment("test-app");

        storage.store(&deployment).await.unwrap();

        let deleted = storage.delete("test-app").await.unwrap();
        assert!(deleted);

        let retrieved = storage.get("test-app").await.unwrap();
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_redb_delete_nonexistent() {
        let storage = RedbStorage::in_memory().unwrap();

        let deleted = storage.delete("nonexistent").await.unwrap();
        assert!(!deleted);
    }

    #[tokio::test]
    async fn test_redb_exists() {
        let storage = RedbStorage::in_memory().unwrap();
        let deployment = create_test_deployment("test-app");

        assert!(!storage.exists("test-app").await.unwrap());

        storage.store(&deployment).await.unwrap();

        assert!(storage.exists("test-app").await.unwrap());
    }

    #[tokio::test]
    async fn test_redb_update() {
        let storage = RedbStorage::in_memory().unwrap();
        let mut deployment = create_test_deployment("test-app");

        storage.store(&deployment).await.unwrap();

        // Update the deployment
        deployment.update_status(DeploymentStatus::Running);
        storage.store(&deployment).await.unwrap();

        let retrieved = storage.get("test-app").await.unwrap().unwrap();
        assert_eq!(retrieved.status, DeploymentStatus::Running);
    }

    #[tokio::test]
    async fn test_redb_persistent_storage() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test.redb");

        // Create and populate database
        {
            let storage = RedbStorage::open(&db_path).unwrap();
            storage
                .store(&create_test_deployment("persistent-app"))
                .await
                .unwrap();
        }

        // Reopen and verify data persists
        {
            let storage = RedbStorage::open(&db_path).unwrap();
            let deployment = storage.get("persistent-app").await.unwrap();
            assert!(deployment.is_some());
            assert_eq!(deployment.unwrap().name, "persistent-app");
        }
    }

    #[tokio::test]
    async fn test_redb_failed_status_serialization() {
        let storage = RedbStorage::in_memory().unwrap();
        let mut deployment = create_test_deployment("test-app");
        deployment.update_status(DeploymentStatus::Failed {
            message: "Container OOM killed".to_string(),
        });

        storage.store(&deployment).await.unwrap();

        let retrieved = storage.get("test-app").await.unwrap().unwrap();
        match retrieved.status {
            DeploymentStatus::Failed { message } => {
                assert_eq!(message, "Container OOM killed");
            }
            _ => panic!("Expected Failed status"),
        }
    }
}
