//! Instance management for `ZLayer` Manager
//!
//! Manages a list of ZLayer API instances that the manager can connect to.
//! Persists to `~/.config/zlayer/instances.json`.

#![allow(clippy::missing_errors_doc, clippy::missing_panics_doc)]

use std::path::PathBuf;
use std::sync::{LazyLock, RwLock};

use serde::{Deserialize, Serialize};

/// A saved ZLayer instance connection
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Instance {
    pub name: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

/// Manages the list of saved instances and the active selection
#[derive(Debug, Serialize, Deserialize)]
pub struct InstanceManager {
    pub instances: Vec<Instance>,
    pub active_index: usize,
}

static INSTANCE_MANAGER: LazyLock<RwLock<InstanceManager>> =
    LazyLock::new(|| RwLock::new(InstanceManager::load_or_default()));

impl Default for InstanceManager {
    fn default() -> Self {
        Self {
            instances: vec![Instance {
                name: "Local".to_string(),
                url: "http://localhost:3669".to_string(),
                token: None,
            }],
            active_index: 0,
        }
    }
}

impl InstanceManager {
    fn config_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        PathBuf::from(home)
            .join(".config")
            .join("zlayer")
            .join("instances.json")
    }

    fn load_or_default() -> Self {
        // Check env var override first — if ZLAYER_API_URL is set, use it as
        // the default instance instead of localhost:3669
        let env_default = std::env::var("ZLAYER_API_URL").ok();
        let env_token = std::env::var("ZLAYER_API_TOKEN").ok();

        let path = Self::config_path();
        let loaded = if path.exists() {
            std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| serde_json::from_str::<InstanceManager>(&s).ok())
                .filter(|mgr| !mgr.instances.is_empty())
        } else {
            None
        };

        loaded.unwrap_or_else(|| {
            let url = env_default.unwrap_or_else(|| "http://localhost:3669".to_string());
            InstanceManager {
                instances: vec![Instance {
                    name: "Local".to_string(),
                    url,
                    token: env_token,
                }],
                active_index: 0,
            }
        })
    }

    fn save(&self) {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&path, json);
        }
    }

    fn active_instance(&self) -> &Instance {
        let idx = self
            .active_index
            .min(self.instances.len().saturating_sub(1));
        &self.instances[idx]
    }
}

/// Get the currently active instance
pub fn get_active() -> Instance {
    let mgr = INSTANCE_MANAGER.read().expect("instance manager poisoned");
    mgr.active_instance().clone()
}

/// List all saved instances and which one is active
pub fn list_all() -> (Vec<Instance>, usize) {
    let mgr = INSTANCE_MANAGER.read().expect("instance manager poisoned");
    (mgr.instances.clone(), mgr.active_index)
}

/// Switch the active instance by index
pub fn set_active(index: usize) -> Result<(), String> {
    let mut mgr = INSTANCE_MANAGER.write().expect("instance manager poisoned");
    if index >= mgr.instances.len() {
        return Err(format!(
            "Index {index} out of range (have {} instances)",
            mgr.instances.len()
        ));
    }
    mgr.active_index = index;
    mgr.save();
    Ok(())
}

/// Add a new instance and persist
pub fn add(instance: Instance) -> Result<(), String> {
    let mut mgr = INSTANCE_MANAGER.write().expect("instance manager poisoned");
    if mgr.instances.iter().any(|i| i.name == instance.name) {
        return Err(format!("Instance '{}' already exists", instance.name));
    }
    mgr.instances.push(instance);
    mgr.save();
    Ok(())
}

/// Update an existing instance by index
pub fn update(index: usize, instance: Instance) -> Result<(), String> {
    let mut mgr = INSTANCE_MANAGER.write().expect("instance manager poisoned");
    if index >= mgr.instances.len() {
        return Err(format!(
            "Index {index} out of range (have {} instances)",
            mgr.instances.len()
        ));
    }
    mgr.instances[index] = instance;
    mgr.save();
    Ok(())
}

/// Remove an instance by index. Cannot remove the last instance.
pub fn remove(index: usize) -> Result<(), String> {
    let mut mgr = INSTANCE_MANAGER.write().expect("instance manager poisoned");
    if mgr.instances.len() <= 1 {
        return Err("Cannot remove the last instance".to_string());
    }
    if index >= mgr.instances.len() {
        return Err(format!(
            "Index {index} out of range (have {} instances)",
            mgr.instances.len()
        ));
    }
    mgr.instances.remove(index);
    // Adjust active index if needed
    if mgr.active_index >= mgr.instances.len() {
        mgr.active_index = mgr.instances.len() - 1;
    }
    mgr.save();
    Ok(())
}

/// Test connection to a URL by hitting its /health endpoint
pub async fn test_connection(url: &str, token: Option<&str>) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?;

    let health_url = format!("{}/health/ready", url.trim_end_matches('/'));
    let mut req = client.get(&health_url);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }

    match req.send().await {
        Ok(resp) if resp.status().is_success() => Ok("Connected".to_string()),
        Ok(resp) => Err(format!("Server returned {}", resp.status())),
        Err(e) => Err(format!("Connection failed: {e}")),
    }
}
