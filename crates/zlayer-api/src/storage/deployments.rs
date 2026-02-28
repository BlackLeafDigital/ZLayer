//! Deployment storage implementations
//!
//! Provides both persistent (ZQL) and in-memory storage backends.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;
use tokio::sync::RwLock;

use super::StoredDeployment;

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

impl From<serde_json::Error> for StorageError {
    fn from(err: serde_json::Error) -> Self {
        StorageError::Serialization(err.to_string())
    }
}

impl From<zql::database::DatabaseError> for StorageError {
    fn from(err: zql::database::DatabaseError) -> Self {
        StorageError::Database(err.to_string())
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

/// ZQL-based persistent storage for deployments
pub struct ZqlStorage {
    db: tokio::sync::Mutex<zql::Database>,
}

impl ZqlStorage {
    /// Open or create a ZQL database at the given path
    pub async fn open<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        let path = path.as_ref().to_path_buf();

        // ZQL Database::open is synchronous, run on blocking thread
        let db = tokio::task::spawn_blocking(move || {
            zql::Database::open(&path)
        })
        .await
        .map_err(|e| StorageError::Database(format!("spawn_blocking failed: {e}")))?
        .map_err(StorageError::from)?;

        Ok(Self {
            db: tokio::sync::Mutex::new(db),
        })
    }

    /// Create a ZQL database in a temporary directory (useful for testing)
    #[cfg(test)]
    pub async fn in_memory() -> Result<Self, StorageError> {
        let temp_dir = tempfile::tempdir()
            .map_err(|e| StorageError::Database(format!("failed to create temp dir: {e}")))?;
        let path = temp_dir.path().join("deployments_zql");

        let db = tokio::task::spawn_blocking(move || {
            // Keep temp_dir alive by moving it into the closure (leaked)
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
impl DeploymentStorage for ZqlStorage {
    async fn store(&self, deployment: &StoredDeployment) -> Result<(), StorageError> {
        let data_json = serde_json::to_string(deployment)?;
        let name = deployment.name.clone();

        // Delete existing entry first (upsert semantics)
        let mut db = self.db.lock().await;
        let _ = db.query(&format!(
            "DELETE FROM deployments WHERE name = '{}'",
            name.replace('\'', "''")
        ));

        // Insert new entry
        db.query(&format!(
            "INSERT INTO deployments (name, data_json) VALUES ('{}', '{}')",
            name.replace('\'', "''"),
            data_json.replace('\'', "''")
        ))
        .map_err(StorageError::from)?;

        Ok(())
    }

    async fn get(&self, name: &str) -> Result<Option<StoredDeployment>, StorageError> {
        let mut db = self.db.lock().await;
        let result = db.query(&format!(
            "SELECT * FROM deployments WHERE name = '{}'",
            name.replace('\'', "''")
        ));

        match result {
            Ok(zql::query::executor::ExecResult::Retrieved(records)) => {
                if records.is_empty() {
                    return Ok(None);
                }
                let record = &records[0];
                if let Some(data_json) = record.fields.get("data_json") {
                    let deployment: StoredDeployment = serde_json::from_str(data_json)?;
                    Ok(Some(deployment))
                } else {
                    Ok(None)
                }
            }
            Ok(_) => Ok(None),
            Err(zql::database::DatabaseError::Exec(
                zql::query::executor::ExecError::UnknownStore(_),
            )) => {
                // Store doesn't exist yet, no deployments
                Ok(None)
            }
            Err(e) => Err(StorageError::from(e)),
        }
    }

    async fn list(&self) -> Result<Vec<StoredDeployment>, StorageError> {
        let mut db = self.db.lock().await;
        let result = db.query("SELECT * FROM deployments");

        match result {
            Ok(zql::query::executor::ExecResult::Retrieved(records)) => {
                let mut deployments = Vec::with_capacity(records.len());
                for record in &records {
                    if let Some(data_json) = record.fields.get("data_json") {
                        let deployment: StoredDeployment = serde_json::from_str(data_json)?;
                        deployments.push(deployment);
                    }
                }
                deployments.sort_by(|a, b| a.name.cmp(&b.name));
                Ok(deployments)
            }
            Ok(_) => Ok(Vec::new()),
            Err(zql::database::DatabaseError::Exec(
                zql::query::executor::ExecError::UnknownStore(_),
            )) => {
                // Store doesn't exist yet
                Ok(Vec::new())
            }
            Err(e) => Err(StorageError::from(e)),
        }
    }

    async fn delete(&self, name: &str) -> Result<bool, StorageError> {
        // Check existence first
        let exists = self.get(name).await?.is_some();
        if !exists {
            return Ok(false);
        }

        let mut db = self.db.lock().await;
        db.query(&format!(
            "DELETE FROM deployments WHERE name = '{}'",
            name.replace('\'', "''")
        ))
        .map_err(StorageError::from)?;

        Ok(true)
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
                service_type: Default::default(),
                wasm_http: None,
                host_network: false,
            },
        );

        DeploymentSpec {
            version: "v1".to_string(),
            deployment: name.to_string(),
            services,
            tunnels: HashMap::new(),
            api: Default::default(),
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
    // ZqlStorage tests
    // =========================================================================

    #[tokio::test]
    async fn test_zql_store_and_get() {
        let storage = ZqlStorage::in_memory().await.unwrap();
        let deployment = create_test_deployment("test-app");

        storage.store(&deployment).await.unwrap();

        let retrieved = storage.get("test-app").await.unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.name, "test-app");
        assert_eq!(retrieved.status, DeploymentStatus::Pending);
    }

    #[tokio::test]
    async fn test_zql_get_nonexistent() {
        let storage = ZqlStorage::in_memory().await.unwrap();

        let result = storage.get("nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_zql_list() {
        let storage = ZqlStorage::in_memory().await.unwrap();

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
    async fn test_zql_delete() {
        let storage = ZqlStorage::in_memory().await.unwrap();
        let deployment = create_test_deployment("test-app");

        storage.store(&deployment).await.unwrap();

        let deleted = storage.delete("test-app").await.unwrap();
        assert!(deleted);

        let retrieved = storage.get("test-app").await.unwrap();
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_zql_delete_nonexistent() {
        let storage = ZqlStorage::in_memory().await.unwrap();

        let deleted = storage.delete("nonexistent").await.unwrap();
        assert!(!deleted);
    }

    #[tokio::test]
    async fn test_zql_exists() {
        let storage = ZqlStorage::in_memory().await.unwrap();
        let deployment = create_test_deployment("test-app");

        assert!(!storage.exists("test-app").await.unwrap());

        storage.store(&deployment).await.unwrap();

        assert!(storage.exists("test-app").await.unwrap());
    }

    #[tokio::test]
    async fn test_zql_update() {
        let storage = ZqlStorage::in_memory().await.unwrap();
        let mut deployment = create_test_deployment("test-app");

        storage.store(&deployment).await.unwrap();

        // Update the deployment
        deployment.update_status(DeploymentStatus::Running);
        storage.store(&deployment).await.unwrap();

        let retrieved = storage.get("test-app").await.unwrap().unwrap();
        assert_eq!(retrieved.status, DeploymentStatus::Running);
    }

    #[tokio::test]
    async fn test_zql_persistent_storage() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test_zql_db");

        // Create and populate database
        {
            let storage = ZqlStorage::open(&db_path).await.unwrap();
            storage
                .store(&create_test_deployment("persistent-app"))
                .await
                .unwrap();
        }

        // Reopen and verify data persists
        {
            let storage = ZqlStorage::open(&db_path).await.unwrap();
            let deployment = storage.get("persistent-app").await.unwrap();
            assert!(deployment.is_some());
            assert_eq!(deployment.unwrap().name, "persistent-app");
        }
    }

    #[tokio::test]
    async fn test_zql_failed_status_serialization() {
        let storage = ZqlStorage::in_memory().await.unwrap();
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
