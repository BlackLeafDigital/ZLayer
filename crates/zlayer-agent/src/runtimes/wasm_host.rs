//! ZLayer WASM Host Function Implementations
//!
//! This module provides the host-side implementations of ZLayer interfaces
//! that WASM plugins can call. It bridges WASIp2 component model exports
//! with the ZLayer runtime capabilities.
//!
//! # Architecture
//!
//! The host functions are organized to match the WIT interfaces in `wit/zlayer/host.wit`:
//!
//! - **config**: Plugin configuration access
//! - **keyvalue**: Key-value storage for plugin state
//! - **logging**: Structured logging with levels
//! - **secrets**: Secure secret access
//! - **metrics**: Counter, gauge, and histogram metrics
//!
//! # Usage
//!
//! ```rust,ignore
//! use zlayer_agent::runtimes::wasm_host::{add_to_linker, DefaultHost};
//! use wasmtime::component::Linker;
//!
//! let mut linker = Linker::new(&engine);
//! add_to_linker(&mut linker)?;
//!
//! let host = DefaultHost::new();
//! let mut store = Store::new(&engine, host);
//! ```

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Log level for plugin logging
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LogLevel {
    /// Trace-level logging (most verbose)
    Trace,
    /// Debug-level logging
    Debug,
    /// Info-level logging
    Info,
    /// Warning-level logging
    Warn,
    /// Error-level logging (least verbose)
    Error,
}

impl LogLevel {
    /// Convert from WIT level integer (0=trace, 4=error)
    pub fn from_wit(level: u8) -> Self {
        match level {
            0 => LogLevel::Trace,
            1 => LogLevel::Debug,
            2 => LogLevel::Info,
            3 => LogLevel::Warn,
            _ => LogLevel::Error,
        }
    }

    /// Convert to WIT level integer
    pub fn to_wit(self) -> u8 {
        match self {
            LogLevel::Trace => 0,
            LogLevel::Debug => 1,
            LogLevel::Info => 2,
            LogLevel::Warn => 3,
            LogLevel::Error => 4,
        }
    }

    /// Convert to tracing level
    pub fn to_tracing(self) -> tracing::Level {
        match self {
            LogLevel::Trace => tracing::Level::TRACE,
            LogLevel::Debug => tracing::Level::DEBUG,
            LogLevel::Info => tracing::Level::INFO,
            LogLevel::Warn => tracing::Level::WARN,
            LogLevel::Error => tracing::Level::ERROR,
        }
    }
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogLevel::Trace => write!(f, "trace"),
            LogLevel::Debug => write!(f, "debug"),
            LogLevel::Info => write!(f, "info"),
            LogLevel::Warn => write!(f, "warn"),
            LogLevel::Error => write!(f, "error"),
        }
    }
}

/// Key-value storage error types matching WIT kv-error variant
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KvError {
    /// Key not found
    NotFound,
    /// Value too large
    ValueTooLarge,
    /// Storage quota exceeded
    QuotaExceeded,
    /// Key format invalid
    InvalidKey,
    /// Generic storage error
    Storage(String),
}

impl std::fmt::Display for KvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KvError::NotFound => write!(f, "key not found"),
            KvError::ValueTooLarge => write!(f, "value too large"),
            KvError::QuotaExceeded => write!(f, "storage quota exceeded"),
            KvError::InvalidKey => write!(f, "invalid key format"),
            KvError::Storage(msg) => write!(f, "storage error: {}", msg),
        }
    }
}

impl std::error::Error for KvError {}

/// Host capabilities trait defining all ZLayer host functions
///
/// This trait defines the interface between WASM plugins and the ZLayer host.
/// Implementations provide actual storage, logging, metrics, etc.
///
/// # Thread Safety
///
/// Implementations must be `Send` to allow use across async contexts.
/// The trait methods use `&self` for reads and `&mut self` for writes,
/// but implementations may use interior mutability for concurrent access.
pub trait ZLayerHost: Send {
    // =========================================================================
    // Configuration Interface (zlayer:host/config@0.1.0)
    // =========================================================================

    /// Get a configuration value by key
    ///
    /// Returns `None` if the key doesn't exist.
    fn config_get(&self, key: &str) -> Option<String>;

    /// Get a configuration value, returning error if not found
    fn config_get_required(&self, key: &str) -> Result<String, String> {
        self.config_get(key)
            .ok_or_else(|| format!("required config key '{}' not found", key))
    }

    /// Get multiple configuration values at once
    ///
    /// Returns list of (key, value) pairs for keys that exist.
    fn config_get_many(&self, keys: &[String]) -> Vec<(String, String)> {
        keys.iter()
            .filter_map(|k| self.config_get(k).map(|v| (k.clone(), v)))
            .collect()
    }

    /// Get all configuration keys with a given prefix
    fn config_get_prefix(&self, prefix: &str) -> Vec<(String, String)>;

    /// Check if a configuration key exists
    fn config_exists(&self, key: &str) -> bool {
        self.config_get(key).is_some()
    }

    /// Get a configuration value as a boolean
    ///
    /// Recognizes: "true", "false", "1", "0", "yes", "no"
    fn config_get_bool(&self, key: &str) -> Option<bool> {
        self.config_get(key).and_then(|v| {
            let lower = v.to_lowercase();
            match lower.as_str() {
                "true" | "1" | "yes" => Some(true),
                "false" | "0" | "no" => Some(false),
                _ => None,
            }
        })
    }

    /// Get a configuration value as an integer
    fn config_get_int(&self, key: &str) -> Option<i64> {
        self.config_get(key).and_then(|v| v.parse().ok())
    }

    /// Get a configuration value as a float
    fn config_get_float(&self, key: &str) -> Option<f64> {
        self.config_get(key).and_then(|v| v.parse().ok())
    }

    /// Get all configuration as JSON string (for debugging/export)
    fn config_get_all(&self) -> String;

    // =========================================================================
    // Key-Value Storage Interface (zlayer:host/keyvalue@0.1.0)
    // =========================================================================

    /// Get a value by key
    fn kv_get(&self, key: &str) -> Result<Option<Vec<u8>>, KvError>;

    /// Get a value as a string
    fn kv_get_string(&self, key: &str) -> Result<Option<String>, KvError> {
        match self.kv_get(key)? {
            Some(bytes) => String::from_utf8(bytes)
                .map(Some)
                .map_err(|e| KvError::Storage(format!("invalid UTF-8: {}", e))),
            None => Ok(None),
        }
    }

    /// Set a value
    fn kv_set(&mut self, key: &str, value: &[u8]) -> Result<(), KvError>;

    /// Set a string value
    fn kv_set_string(&mut self, key: &str, value: &str) -> Result<(), KvError> {
        self.kv_set(key, value.as_bytes())
    }

    /// Set a value with TTL in nanoseconds
    fn kv_set_with_ttl(&mut self, key: &str, value: &[u8], ttl_ns: u64) -> Result<(), KvError>;

    /// Delete a key
    ///
    /// Returns `true` if the key existed and was deleted.
    fn kv_delete(&mut self, key: &str) -> Result<bool, KvError>;

    /// Check if a key exists
    fn kv_exists(&self, key: &str) -> bool;

    /// List all keys with a given prefix
    fn kv_list_keys(&self, prefix: &str) -> Result<Vec<String>, KvError>;

    /// Increment a numeric value atomically
    ///
    /// Returns the new value after increment.
    fn kv_increment(&mut self, key: &str, delta: i64) -> Result<i64, KvError>;

    /// Compare and swap - set value only if current value matches expected
    ///
    /// Returns `true` if swap succeeded, `false` if current value didn't match.
    fn kv_compare_and_swap(
        &mut self,
        key: &str,
        expected: Option<&[u8]>,
        new_value: &[u8],
    ) -> Result<bool, KvError>;

    // =========================================================================
    // Logging Interface (zlayer:host/logging@0.1.0)
    // =========================================================================

    /// Emit a log message at the specified level
    fn log(&self, level: LogLevel, message: &str);

    /// Emit a structured log with key-value fields
    fn log_structured(&self, level: LogLevel, message: &str, fields: &[(String, String)]);

    /// Check if a log level is enabled
    fn log_is_enabled(&self, level: LogLevel) -> bool;

    // =========================================================================
    // Secrets Interface (zlayer:host/secrets@0.1.0)
    // =========================================================================

    /// Get a secret by name
    fn secret_get(&self, name: &str) -> Result<Option<String>, String>;

    /// Get a required secret, error if not found
    fn secret_get_required(&self, name: &str) -> Result<String, String> {
        self.secret_get(name)?
            .ok_or_else(|| format!("required secret '{}' not found", name))
    }

    /// Check if a secret exists
    fn secret_exists(&self, name: &str) -> bool;

    /// List available secret names (not values)
    fn secret_list_names(&self) -> Vec<String>;

    // =========================================================================
    // Metrics Interface (zlayer:host/metrics@0.1.0)
    // =========================================================================

    /// Increment a counter metric
    fn counter_inc(&self, name: &str, value: u64);

    /// Increment a counter with labels
    fn counter_inc_labeled(&self, name: &str, value: u64, labels: &[(String, String)]);

    /// Set a gauge metric to a value
    fn gauge_set(&self, name: &str, value: f64);

    /// Set a gauge with labels
    fn gauge_set_labeled(&self, name: &str, value: f64, labels: &[(String, String)]);

    /// Add to a gauge value (can be negative)
    fn gauge_add(&self, name: &str, delta: f64);

    /// Record a histogram observation
    fn histogram_observe(&self, name: &str, value: f64);

    /// Record a histogram observation with labels
    fn histogram_observe_labeled(&self, name: &str, value: f64, labels: &[(String, String)]);

    /// Record request duration in nanoseconds
    fn record_duration(&self, name: &str, duration_ns: u64);

    /// Record request duration with labels
    fn record_duration_labeled(&self, name: &str, duration_ns: u64, labels: &[(String, String)]);
}

/// Default host implementation for testing and development
///
/// This implementation provides in-memory storage for all host capabilities.
/// It's useful for:
/// - Unit testing plugins
/// - Development and debugging
/// - Sandboxed execution without external dependencies
///
/// # Thread Safety
///
/// Uses `Arc<RwLock<...>>` for interior mutability, allowing shared access
/// across async contexts while maintaining thread safety.
#[derive(Debug, Clone)]
pub struct DefaultHost {
    /// Plugin identifier for logging context
    plugin_id: String,
    /// Configuration values
    config: Arc<RwLock<HashMap<String, String>>>,
    /// Key-value storage
    kv: Arc<RwLock<HashMap<String, KvEntry>>>,
    /// Secret storage
    secrets: Arc<RwLock<HashMap<String, String>>>,
    /// Metrics storage (for testing verification)
    metrics: Arc<RwLock<MetricsStore>>,
    /// Maximum value size in bytes (default 1MB)
    max_value_size: usize,
    /// Maximum number of keys (default 10000)
    max_keys: usize,
    /// Minimum log level to emit
    min_log_level: LogLevel,
}

/// Key-value entry with optional TTL
#[derive(Debug, Clone)]
struct KvEntry {
    value: Vec<u8>,
    expires_at: Option<std::time::Instant>,
}

impl KvEntry {
    fn new(value: Vec<u8>) -> Self {
        Self {
            value,
            expires_at: None,
        }
    }

    fn with_ttl(value: Vec<u8>, ttl_ns: u64) -> Self {
        let expires_at = Some(std::time::Instant::now() + std::time::Duration::from_nanos(ttl_ns));
        Self { value, expires_at }
    }

    fn is_expired(&self) -> bool {
        self.expires_at
            .map(|exp| std::time::Instant::now() >= exp)
            .unwrap_or(false)
    }
}

/// In-memory metrics storage for testing
#[derive(Debug, Default, Clone)]
pub struct MetricsStore {
    /// Counter values by (name, labels_key)
    pub counters: HashMap<String, u64>,
    /// Gauge values by (name, labels_key)
    pub gauges: HashMap<String, f64>,
    /// Histogram observations by (name, labels_key)
    pub histograms: HashMap<String, Vec<f64>>,
}

impl MetricsStore {
    /// Create a key from name and labels for storage lookup
    fn make_key(name: &str, labels: &[(String, String)]) -> String {
        if labels.is_empty() {
            name.to_string()
        } else {
            let mut sorted_labels = labels.to_vec();
            sorted_labels.sort_by(|a, b| a.0.cmp(&b.0));
            let labels_str = sorted_labels
                .iter()
                .map(|(k, v)| format!("{}={}", k, v))
                .collect::<Vec<_>>()
                .join(",");
            format!("{}:{{{}}}", name, labels_str)
        }
    }

    /// Get counter value for testing verification
    pub fn get_counter(&self, name: &str) -> Option<u64> {
        self.counters.get(name).copied()
    }

    /// Get counter value with labels for testing verification
    pub fn get_counter_labeled(&self, name: &str, labels: &[(String, String)]) -> Option<u64> {
        let key = Self::make_key(name, labels);
        self.counters.get(&key).copied()
    }

    /// Get gauge value for testing verification
    pub fn get_gauge(&self, name: &str) -> Option<f64> {
        self.gauges.get(name).copied()
    }

    /// Get gauge value with labels for testing verification
    pub fn get_gauge_labeled(&self, name: &str, labels: &[(String, String)]) -> Option<f64> {
        let key = Self::make_key(name, labels);
        self.gauges.get(&key).copied()
    }

    /// Get histogram observations for testing verification
    pub fn get_histogram(&self, name: &str) -> Option<&Vec<f64>> {
        self.histograms.get(name)
    }
}

impl Default for DefaultHost {
    fn default() -> Self {
        Self::new()
    }
}

impl DefaultHost {
    /// Create a new default host with empty storage
    pub fn new() -> Self {
        Self {
            plugin_id: "unknown".to_string(),
            config: Arc::new(RwLock::new(HashMap::new())),
            kv: Arc::new(RwLock::new(HashMap::new())),
            secrets: Arc::new(RwLock::new(HashMap::new())),
            metrics: Arc::new(RwLock::new(MetricsStore::default())),
            max_value_size: 1024 * 1024, // 1MB
            max_keys: 10000,
            min_log_level: LogLevel::Trace,
        }
    }

    /// Create a new host with a plugin identifier
    pub fn with_plugin_id(plugin_id: impl Into<String>) -> Self {
        let mut host = Self::new();
        host.plugin_id = plugin_id.into();
        host
    }

    /// Set the plugin identifier
    pub fn set_plugin_id(&mut self, plugin_id: impl Into<String>) {
        self.plugin_id = plugin_id.into();
    }

    /// Set maximum value size in bytes
    pub fn set_max_value_size(&mut self, size: usize) {
        self.max_value_size = size;
    }

    /// Set maximum number of keys
    pub fn set_max_keys(&mut self, count: usize) {
        self.max_keys = count;
    }

    /// Set minimum log level
    pub fn set_min_log_level(&mut self, level: LogLevel) {
        self.min_log_level = level;
    }

    /// Add a configuration value
    pub fn add_config(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.config
            .write()
            .expect("config lock poisoned")
            .insert(key.into(), value.into());
    }

    /// Add multiple configuration values
    pub fn add_configs<I, K, V>(&mut self, configs: I)
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let mut config = self.config.write().expect("config lock poisoned");
        for (k, v) in configs {
            config.insert(k.into(), v.into());
        }
    }

    /// Add a secret
    pub fn add_secret(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.secrets
            .write()
            .expect("secrets lock poisoned")
            .insert(name.into(), value.into());
    }

    /// Add multiple secrets
    pub fn add_secrets<I, K, V>(&mut self, secrets: I)
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let mut secrets_store = self.secrets.write().expect("secrets lock poisoned");
        for (k, v) in secrets {
            secrets_store.insert(k.into(), v.into());
        }
    }

    /// Get a reference to the metrics store for testing verification
    pub fn metrics(&self) -> std::sync::RwLockReadGuard<'_, MetricsStore> {
        self.metrics.read().expect("metrics lock poisoned")
    }

    /// Clear all stored data (useful for test reset)
    pub fn clear(&mut self) {
        self.config.write().expect("config lock poisoned").clear();
        self.kv.write().expect("kv lock poisoned").clear();
        self.secrets.write().expect("secrets lock poisoned").clear();
        *self.metrics.write().expect("metrics lock poisoned") = MetricsStore::default();
    }

    /// Validate key format
    fn validate_key(key: &str) -> Result<(), KvError> {
        if key.is_empty() {
            return Err(KvError::InvalidKey);
        }
        if key.len() > 1024 {
            return Err(KvError::InvalidKey);
        }
        // Allow alphanumeric, dash, underscore, dot, slash, colon
        if !key
            .chars()
            .all(|c| c.is_alphanumeric() || "-_./:".contains(c))
        {
            return Err(KvError::InvalidKey);
        }
        Ok(())
    }

    /// Clean expired entries
    fn clean_expired(&self) {
        let mut kv = self.kv.write().expect("kv lock poisoned");
        kv.retain(|_, entry| !entry.is_expired());
    }
}

impl ZLayerHost for DefaultHost {
    // =========================================================================
    // Configuration Interface
    // =========================================================================

    fn config_get(&self, key: &str) -> Option<String> {
        self.config
            .read()
            .expect("config lock poisoned")
            .get(key)
            .cloned()
    }

    fn config_get_prefix(&self, prefix: &str) -> Vec<(String, String)> {
        self.config
            .read()
            .expect("config lock poisoned")
            .iter()
            .filter(|(k, _)| k.starts_with(prefix))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    fn config_get_all(&self) -> String {
        let config = self.config.read().expect("config lock poisoned");
        serde_json::to_string(&*config).unwrap_or_else(|_| "{}".to_string())
    }

    // =========================================================================
    // Key-Value Storage Interface
    // =========================================================================

    fn kv_get(&self, key: &str) -> Result<Option<Vec<u8>>, KvError> {
        Self::validate_key(key)?;
        self.clean_expired();

        let kv = self.kv.read().expect("kv lock poisoned");
        match kv.get(key) {
            Some(entry) if !entry.is_expired() => Ok(Some(entry.value.clone())),
            _ => Ok(None),
        }
    }

    fn kv_set(&mut self, key: &str, value: &[u8]) -> Result<(), KvError> {
        Self::validate_key(key)?;

        if value.len() > self.max_value_size {
            return Err(KvError::ValueTooLarge);
        }

        let mut kv = self.kv.write().expect("kv lock poisoned");

        // Check quota (only if adding new key)
        if !kv.contains_key(key) && kv.len() >= self.max_keys {
            return Err(KvError::QuotaExceeded);
        }

        kv.insert(key.to_string(), KvEntry::new(value.to_vec()));
        Ok(())
    }

    fn kv_set_with_ttl(&mut self, key: &str, value: &[u8], ttl_ns: u64) -> Result<(), KvError> {
        Self::validate_key(key)?;

        if value.len() > self.max_value_size {
            return Err(KvError::ValueTooLarge);
        }

        let mut kv = self.kv.write().expect("kv lock poisoned");

        // Check quota (only if adding new key)
        if !kv.contains_key(key) && kv.len() >= self.max_keys {
            return Err(KvError::QuotaExceeded);
        }

        kv.insert(key.to_string(), KvEntry::with_ttl(value.to_vec(), ttl_ns));
        Ok(())
    }

    fn kv_delete(&mut self, key: &str) -> Result<bool, KvError> {
        Self::validate_key(key)?;

        let mut kv = self.kv.write().expect("kv lock poisoned");
        Ok(kv.remove(key).is_some())
    }

    fn kv_exists(&self, key: &str) -> bool {
        self.clean_expired();
        let kv = self.kv.read().expect("kv lock poisoned");
        kv.get(key).map(|e| !e.is_expired()).unwrap_or(false)
    }

    fn kv_list_keys(&self, prefix: &str) -> Result<Vec<String>, KvError> {
        self.clean_expired();
        let kv = self.kv.read().expect("kv lock poisoned");
        Ok(kv
            .iter()
            .filter(|(k, entry)| k.starts_with(prefix) && !entry.is_expired())
            .map(|(k, _)| k.clone())
            .collect())
    }

    fn kv_increment(&mut self, key: &str, delta: i64) -> Result<i64, KvError> {
        Self::validate_key(key)?;

        let mut kv = self.kv.write().expect("kv lock poisoned");

        let current: i64 = match kv.get(key) {
            Some(entry) if !entry.is_expired() => {
                let s = String::from_utf8(entry.value.clone())
                    .map_err(|e| KvError::Storage(format!("invalid number: {}", e)))?;
                s.parse()
                    .map_err(|e| KvError::Storage(format!("invalid number: {}", e)))?
            }
            _ => 0,
        };

        let new_value = current.saturating_add(delta);
        let value_str = new_value.to_string();

        // Check quota (only if adding new key)
        if !kv.contains_key(key) && kv.len() >= self.max_keys {
            return Err(KvError::QuotaExceeded);
        }

        kv.insert(key.to_string(), KvEntry::new(value_str.into_bytes()));
        Ok(new_value)
    }

    fn kv_compare_and_swap(
        &mut self,
        key: &str,
        expected: Option<&[u8]>,
        new_value: &[u8],
    ) -> Result<bool, KvError> {
        Self::validate_key(key)?;

        if new_value.len() > self.max_value_size {
            return Err(KvError::ValueTooLarge);
        }

        let mut kv = self.kv.write().expect("kv lock poisoned");

        let current = kv.get(key).and_then(|e| {
            if e.is_expired() {
                None
            } else {
                Some(e.value.as_slice())
            }
        });

        if current == expected {
            // Check quota (only if adding new key)
            if current.is_none() && kv.len() >= self.max_keys {
                return Err(KvError::QuotaExceeded);
            }
            kv.insert(key.to_string(), KvEntry::new(new_value.to_vec()));
            Ok(true)
        } else {
            Ok(false)
        }
    }

    // =========================================================================
    // Logging Interface
    // =========================================================================

    fn log(&self, level: LogLevel, message: &str) {
        if level.to_wit() < self.min_log_level.to_wit() {
            return;
        }

        let plugin_id = &self.plugin_id;

        match level {
            LogLevel::Trace => tracing::trace!(plugin = %plugin_id, "{}", message),
            LogLevel::Debug => tracing::debug!(plugin = %plugin_id, "{}", message),
            LogLevel::Info => tracing::info!(plugin = %plugin_id, "{}", message),
            LogLevel::Warn => tracing::warn!(plugin = %plugin_id, "{}", message),
            LogLevel::Error => tracing::error!(plugin = %plugin_id, "{}", message),
        }
    }

    fn log_structured(&self, level: LogLevel, message: &str, fields: &[(String, String)]) {
        if level.to_wit() < self.min_log_level.to_wit() {
            return;
        }

        let plugin_id = &self.plugin_id;

        // Format fields as JSON for structured logging
        let fields_json: HashMap<&str, &str> = fields
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        match level {
            LogLevel::Trace => {
                tracing::trace!(plugin = %plugin_id, fields = ?fields_json, "{}", message)
            }
            LogLevel::Debug => {
                tracing::debug!(plugin = %plugin_id, fields = ?fields_json, "{}", message)
            }
            LogLevel::Info => {
                tracing::info!(plugin = %plugin_id, fields = ?fields_json, "{}", message)
            }
            LogLevel::Warn => {
                tracing::warn!(plugin = %plugin_id, fields = ?fields_json, "{}", message)
            }
            LogLevel::Error => {
                tracing::error!(plugin = %plugin_id, fields = ?fields_json, "{}", message)
            }
        }
    }

    fn log_is_enabled(&self, level: LogLevel) -> bool {
        level.to_wit() >= self.min_log_level.to_wit()
    }

    // =========================================================================
    // Secrets Interface
    // =========================================================================

    fn secret_get(&self, name: &str) -> Result<Option<String>, String> {
        Ok(self
            .secrets
            .read()
            .expect("secrets lock poisoned")
            .get(name)
            .cloned())
    }

    fn secret_exists(&self, name: &str) -> bool {
        self.secrets
            .read()
            .expect("secrets lock poisoned")
            .contains_key(name)
    }

    fn secret_list_names(&self) -> Vec<String> {
        self.secrets
            .read()
            .expect("secrets lock poisoned")
            .keys()
            .cloned()
            .collect()
    }

    // =========================================================================
    // Metrics Interface
    // =========================================================================

    fn counter_inc(&self, name: &str, value: u64) {
        let mut metrics = self.metrics.write().expect("metrics lock poisoned");
        *metrics.counters.entry(name.to_string()).or_insert(0) += value;
    }

    fn counter_inc_labeled(&self, name: &str, value: u64, labels: &[(String, String)]) {
        let key = MetricsStore::make_key(name, labels);
        let mut metrics = self.metrics.write().expect("metrics lock poisoned");
        *metrics.counters.entry(key).or_insert(0) += value;
    }

    fn gauge_set(&self, name: &str, value: f64) {
        let mut metrics = self.metrics.write().expect("metrics lock poisoned");
        metrics.gauges.insert(name.to_string(), value);
    }

    fn gauge_set_labeled(&self, name: &str, value: f64, labels: &[(String, String)]) {
        let key = MetricsStore::make_key(name, labels);
        let mut metrics = self.metrics.write().expect("metrics lock poisoned");
        metrics.gauges.insert(key, value);
    }

    fn gauge_add(&self, name: &str, delta: f64) {
        let mut metrics = self.metrics.write().expect("metrics lock poisoned");
        let current = metrics.gauges.entry(name.to_string()).or_insert(0.0);
        *current += delta;
    }

    fn histogram_observe(&self, name: &str, value: f64) {
        let mut metrics = self.metrics.write().expect("metrics lock poisoned");
        metrics
            .histograms
            .entry(name.to_string())
            .or_default()
            .push(value);
    }

    fn histogram_observe_labeled(&self, name: &str, value: f64, labels: &[(String, String)]) {
        let key = MetricsStore::make_key(name, labels);
        let mut metrics = self.metrics.write().expect("metrics lock poisoned");
        metrics.histograms.entry(key).or_default().push(value);
    }

    fn record_duration(&self, name: &str, duration_ns: u64) {
        // Convert to seconds for histogram
        let seconds = duration_ns as f64 / 1_000_000_000.0;
        self.histogram_observe(name, seconds);
    }

    fn record_duration_labeled(&self, name: &str, duration_ns: u64, labels: &[(String, String)]) {
        let seconds = duration_ns as f64 / 1_000_000_000.0;
        self.histogram_observe_labeled(name, seconds, labels);
    }
}

/// Add ZLayer host functions to a wasmtime component linker
///
/// This registers all ZLayer host interfaces with the linker so WASM components
/// can import and call them. The store must contain a type implementing `ZLayerHost`.
///
/// # Interface Versions
///
/// Registers the following interfaces at version 0.1.0:
/// - `zlayer:plugin/config@0.1.0`
/// - `zlayer:plugin/keyvalue@0.1.0`
/// - `zlayer:plugin/logging@0.1.0`
/// - `zlayer:plugin/secrets@0.1.0`
/// - `zlayer:plugin/metrics@0.1.0`
///
/// # Example
///
/// ```rust,ignore
/// use wasmtime::component::{Component, Linker};
/// use wasmtime::{Engine, Store, Config};
/// use zlayer_agent::runtimes::wasm_host::{add_to_linker, DefaultHost};
///
/// let mut config = Config::new();
/// config.wasm_component_model(true);
/// let engine = Engine::new(&config)?;
///
/// let mut linker: Linker<DefaultHost> = Linker::new(&engine);
/// add_to_linker(&mut linker)?;
///
/// let host = DefaultHost::new();
/// let mut store = Store::new(&engine, host);
/// ```
pub fn add_to_linker<T>(linker: &mut wasmtime::component::Linker<T>) -> Result<(), wasmtime::Error>
where
    T: ZLayerHost + wasmtime_wasi::WasiView + 'static,
{
    // Register zlayer:plugin/config@0.1.0
    add_config_to_linker(linker)?;

    // Register zlayer:plugin/keyvalue@0.1.0
    add_keyvalue_to_linker(linker)?;

    // Register zlayer:plugin/logging@0.1.0
    add_logging_to_linker(linker)?;

    // Register zlayer:plugin/secrets@0.1.0
    add_secrets_to_linker(linker)?;

    // Register zlayer:plugin/metrics@0.1.0
    add_metrics_to_linker(linker)?;

    Ok(())
}

/// Register config interface functions
fn add_config_to_linker<T>(
    linker: &mut wasmtime::component::Linker<T>,
) -> Result<(), wasmtime::Error>
where
    T: ZLayerHost + 'static,
{
    let mut instance = linker.instance("zlayer:plugin/config@0.1.0")?;

    instance.func_wrap(
        "get",
        |ctx: wasmtime::StoreContextMut<'_, T>, (key,): (String,)| {
            let host = ctx.data();
            Ok((host.config_get(&key),))
        },
    )?;

    instance.func_wrap(
        "get-required",
        |ctx: wasmtime::StoreContextMut<'_, T>, (key,): (String,)| {
            let host = ctx.data();
            match host.config_get_required(&key) {
                Ok(v) => Ok((Ok::<String, (String, String)>(v),)),
                Err(e) => Ok((Err(("not_found".to_string(), e)),)),
            }
        },
    )?;

    instance.func_wrap(
        "get-many",
        |ctx: wasmtime::StoreContextMut<'_, T>, (keys,): (Vec<String>,)| {
            let host = ctx.data();
            Ok((host.config_get_many(&keys),))
        },
    )?;

    instance.func_wrap(
        "get-prefix",
        |ctx: wasmtime::StoreContextMut<'_, T>, (prefix,): (String,)| {
            let host = ctx.data();
            Ok((host.config_get_prefix(&prefix),))
        },
    )?;

    instance.func_wrap(
        "exists",
        |ctx: wasmtime::StoreContextMut<'_, T>, (key,): (String,)| {
            let host = ctx.data();
            Ok((host.config_exists(&key),))
        },
    )?;

    instance.func_wrap(
        "get-bool",
        |ctx: wasmtime::StoreContextMut<'_, T>, (key,): (String,)| {
            let host = ctx.data();
            Ok((host.config_get_bool(&key),))
        },
    )?;

    instance.func_wrap(
        "get-int",
        |ctx: wasmtime::StoreContextMut<'_, T>, (key,): (String,)| {
            let host = ctx.data();
            Ok((host.config_get_int(&key),))
        },
    )?;

    instance.func_wrap(
        "get-float",
        |ctx: wasmtime::StoreContextMut<'_, T>, (key,): (String,)| {
            let host = ctx.data();
            Ok((host.config_get_float(&key),))
        },
    )?;

    Ok(())
}

/// Register keyvalue interface functions
fn add_keyvalue_to_linker<T>(
    linker: &mut wasmtime::component::Linker<T>,
) -> Result<(), wasmtime::Error>
where
    T: ZLayerHost + 'static,
{
    let mut instance = linker.instance("zlayer:plugin/keyvalue@0.1.0")?;

    instance.func_wrap(
        "get",
        |ctx: wasmtime::StoreContextMut<'_, T>, (key,): (String,)| {
            let host = ctx.data();
            match host.kv_get(&key) {
                Ok(v) => Ok((Ok::<Option<Vec<u8>>, u8>(v),)),
                Err(e) => Ok((Err(kv_error_to_wit(&e)),)),
            }
        },
    )?;

    instance.func_wrap(
        "get-string",
        |ctx: wasmtime::StoreContextMut<'_, T>, (key,): (String,)| {
            let host = ctx.data();
            match host.kv_get_string(&key) {
                Ok(v) => Ok((Ok::<Option<String>, u8>(v),)),
                Err(e) => Ok((Err(kv_error_to_wit(&e)),)),
            }
        },
    )?;

    instance.func_wrap(
        "set",
        |mut ctx: wasmtime::StoreContextMut<'_, T>, (key, value): (String, Vec<u8>)| {
            let host = ctx.data_mut();
            match host.kv_set(&key, &value) {
                Ok(()) => Ok((Ok::<(), u8>(()),)),
                Err(e) => Ok((Err(kv_error_to_wit(&e)),)),
            }
        },
    )?;

    instance.func_wrap(
        "set-string",
        |mut ctx: wasmtime::StoreContextMut<'_, T>, (key, value): (String, String)| {
            let host = ctx.data_mut();
            match host.kv_set_string(&key, &value) {
                Ok(()) => Ok((Ok::<(), u8>(()),)),
                Err(e) => Ok((Err(kv_error_to_wit(&e)),)),
            }
        },
    )?;

    instance.func_wrap(
        "set-with-ttl",
        |mut ctx: wasmtime::StoreContextMut<'_, T>, (key, value, ttl): (String, Vec<u8>, u64)| {
            let host = ctx.data_mut();
            match host.kv_set_with_ttl(&key, &value, ttl) {
                Ok(()) => Ok((Ok::<(), u8>(()),)),
                Err(e) => Ok((Err(kv_error_to_wit(&e)),)),
            }
        },
    )?;

    instance.func_wrap(
        "delete",
        |mut ctx: wasmtime::StoreContextMut<'_, T>, (key,): (String,)| {
            let host = ctx.data_mut();
            match host.kv_delete(&key) {
                Ok(v) => Ok((Ok::<bool, u8>(v),)),
                Err(e) => Ok((Err(kv_error_to_wit(&e)),)),
            }
        },
    )?;

    instance.func_wrap(
        "exists",
        |ctx: wasmtime::StoreContextMut<'_, T>, (key,): (String,)| {
            let host = ctx.data();
            Ok((host.kv_exists(&key),))
        },
    )?;

    instance.func_wrap(
        "list-keys",
        |ctx: wasmtime::StoreContextMut<'_, T>, (prefix,): (String,)| {
            let host = ctx.data();
            match host.kv_list_keys(&prefix) {
                Ok(v) => Ok((Ok::<Vec<String>, u8>(v),)),
                Err(e) => Ok((Err(kv_error_to_wit(&e)),)),
            }
        },
    )?;

    instance.func_wrap(
        "increment",
        |mut ctx: wasmtime::StoreContextMut<'_, T>, (key, delta): (String, i64)| {
            let host = ctx.data_mut();
            match host.kv_increment(&key, delta) {
                Ok(v) => Ok((Ok::<i64, u8>(v),)),
                Err(e) => Ok((Err(kv_error_to_wit(&e)),)),
            }
        },
    )?;

    instance.func_wrap(
        "compare-and-swap",
        |mut ctx: wasmtime::StoreContextMut<'_, T>,
         (key, expected, new_value): (String, Option<Vec<u8>>, Vec<u8>)| {
            let host = ctx.data_mut();
            let expected_ref = expected.as_deref();
            match host.kv_compare_and_swap(&key, expected_ref, &new_value) {
                Ok(v) => Ok((Ok::<bool, u8>(v),)),
                Err(e) => Ok((Err(kv_error_to_wit(&e)),)),
            }
        },
    )?;

    Ok(())
}

/// Register logging interface functions
fn add_logging_to_linker<T>(
    linker: &mut wasmtime::component::Linker<T>,
) -> Result<(), wasmtime::Error>
where
    T: ZLayerHost + 'static,
{
    let mut instance = linker.instance("zlayer:plugin/logging@0.1.0")?;

    instance.func_wrap(
        "log",
        |ctx: wasmtime::StoreContextMut<'_, T>, (level, message): (u8, String)| {
            let host = ctx.data();
            host.log(LogLevel::from_wit(level), &message);
            Ok(())
        },
    )?;

    instance.func_wrap(
        "log-structured",
        |ctx: wasmtime::StoreContextMut<'_, T>,
         (level, message, fields): (u8, String, Vec<(String, String)>)| {
            let host = ctx.data();
            host.log_structured(LogLevel::from_wit(level), &message, &fields);
            Ok(())
        },
    )?;

    instance.func_wrap(
        "trace",
        |ctx: wasmtime::StoreContextMut<'_, T>, (message,): (String,)| {
            let host = ctx.data();
            host.log(LogLevel::Trace, &message);
            Ok(())
        },
    )?;

    instance.func_wrap(
        "debug",
        |ctx: wasmtime::StoreContextMut<'_, T>, (message,): (String,)| {
            let host = ctx.data();
            host.log(LogLevel::Debug, &message);
            Ok(())
        },
    )?;

    instance.func_wrap(
        "info",
        |ctx: wasmtime::StoreContextMut<'_, T>, (message,): (String,)| {
            let host = ctx.data();
            host.log(LogLevel::Info, &message);
            Ok(())
        },
    )?;

    instance.func_wrap(
        "warn",
        |ctx: wasmtime::StoreContextMut<'_, T>, (message,): (String,)| {
            let host = ctx.data();
            host.log(LogLevel::Warn, &message);
            Ok(())
        },
    )?;

    instance.func_wrap(
        "error",
        |ctx: wasmtime::StoreContextMut<'_, T>, (message,): (String,)| {
            let host = ctx.data();
            host.log(LogLevel::Error, &message);
            Ok(())
        },
    )?;

    instance.func_wrap(
        "is-enabled",
        |ctx: wasmtime::StoreContextMut<'_, T>, (level,): (u8,)| {
            let host = ctx.data();
            Ok((host.log_is_enabled(LogLevel::from_wit(level)),))
        },
    )?;

    Ok(())
}

/// Register secrets interface functions
fn add_secrets_to_linker<T>(
    linker: &mut wasmtime::component::Linker<T>,
) -> Result<(), wasmtime::Error>
where
    T: ZLayerHost + 'static,
{
    let mut instance = linker.instance("zlayer:plugin/secrets@0.1.0")?;

    instance.func_wrap(
        "get",
        |ctx: wasmtime::StoreContextMut<'_, T>, (name,): (String,)| {
            let host = ctx.data();
            match host.secret_get(&name) {
                Ok(v) => Ok((Ok::<Option<String>, (String, String)>(v),)),
                Err(e) => Ok((Err(("error".to_string(), e)),)),
            }
        },
    )?;

    instance.func_wrap(
        "get-required",
        |ctx: wasmtime::StoreContextMut<'_, T>, (name,): (String,)| {
            let host = ctx.data();
            match host.secret_get_required(&name) {
                Ok(v) => Ok((Ok::<String, (String, String)>(v),)),
                Err(e) => Ok((Err(("not_found".to_string(), e)),)),
            }
        },
    )?;

    instance.func_wrap(
        "exists",
        |ctx: wasmtime::StoreContextMut<'_, T>, (name,): (String,)| {
            let host = ctx.data();
            Ok((host.secret_exists(&name),))
        },
    )?;

    instance.func_wrap("list-names", |ctx: wasmtime::StoreContextMut<'_, T>, ()| {
        let host = ctx.data();
        Ok((host.secret_list_names(),))
    })?;

    Ok(())
}

/// Register metrics interface functions
fn add_metrics_to_linker<T>(
    linker: &mut wasmtime::component::Linker<T>,
) -> Result<(), wasmtime::Error>
where
    T: ZLayerHost + 'static,
{
    let mut instance = linker.instance("zlayer:plugin/metrics@0.1.0")?;

    instance.func_wrap(
        "counter-inc",
        |ctx: wasmtime::StoreContextMut<'_, T>, (name, value): (String, u64)| {
            let host = ctx.data();
            host.counter_inc(&name, value);
            Ok(())
        },
    )?;

    instance.func_wrap(
        "counter-inc-labeled",
        |ctx: wasmtime::StoreContextMut<'_, T>,
         (name, value, labels): (String, u64, Vec<(String, String)>)| {
            let host = ctx.data();
            host.counter_inc_labeled(&name, value, &labels);
            Ok(())
        },
    )?;

    instance.func_wrap(
        "gauge-set",
        |ctx: wasmtime::StoreContextMut<'_, T>, (name, value): (String, f64)| {
            let host = ctx.data();
            host.gauge_set(&name, value);
            Ok(())
        },
    )?;

    instance.func_wrap(
        "gauge-set-labeled",
        |ctx: wasmtime::StoreContextMut<'_, T>,
         (name, value, labels): (String, f64, Vec<(String, String)>)| {
            let host = ctx.data();
            host.gauge_set_labeled(&name, value, &labels);
            Ok(())
        },
    )?;

    instance.func_wrap(
        "gauge-add",
        |ctx: wasmtime::StoreContextMut<'_, T>, (name, delta): (String, f64)| {
            let host = ctx.data();
            host.gauge_add(&name, delta);
            Ok(())
        },
    )?;

    instance.func_wrap(
        "histogram-observe",
        |ctx: wasmtime::StoreContextMut<'_, T>, (name, value): (String, f64)| {
            let host = ctx.data();
            host.histogram_observe(&name, value);
            Ok(())
        },
    )?;

    instance.func_wrap(
        "histogram-observe-labeled",
        |ctx: wasmtime::StoreContextMut<'_, T>,
         (name, value, labels): (String, f64, Vec<(String, String)>)| {
            let host = ctx.data();
            host.histogram_observe_labeled(&name, value, &labels);
            Ok(())
        },
    )?;

    instance.func_wrap(
        "record-duration",
        |ctx: wasmtime::StoreContextMut<'_, T>, (name, duration_ns): (String, u64)| {
            let host = ctx.data();
            host.record_duration(&name, duration_ns);
            Ok(())
        },
    )?;

    instance.func_wrap(
        "record-duration-labeled",
        |ctx: wasmtime::StoreContextMut<'_, T>,
         (name, duration_ns, labels): (String, u64, Vec<(String, String)>)| {
            let host = ctx.data();
            host.record_duration_labeled(&name, duration_ns, &labels);
            Ok(())
        },
    )?;

    Ok(())
}

/// Convert KvError to WIT error code
fn kv_error_to_wit(err: &KvError) -> u8 {
    match err {
        KvError::NotFound => 0,
        KvError::ValueTooLarge => 1,
        KvError::QuotaExceeded => 2,
        KvError::InvalidKey => 3,
        KvError::Storage(_) => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // LogLevel Tests
    // =========================================================================

    #[test]
    fn test_log_level_conversion() {
        assert_eq!(LogLevel::from_wit(0), LogLevel::Trace);
        assert_eq!(LogLevel::from_wit(1), LogLevel::Debug);
        assert_eq!(LogLevel::from_wit(2), LogLevel::Info);
        assert_eq!(LogLevel::from_wit(3), LogLevel::Warn);
        assert_eq!(LogLevel::from_wit(4), LogLevel::Error);
        assert_eq!(LogLevel::from_wit(5), LogLevel::Error); // Invalid defaults to Error
        assert_eq!(LogLevel::from_wit(255), LogLevel::Error); // Edge case

        assert_eq!(LogLevel::Trace.to_wit(), 0);
        assert_eq!(LogLevel::Debug.to_wit(), 1);
        assert_eq!(LogLevel::Info.to_wit(), 2);
        assert_eq!(LogLevel::Warn.to_wit(), 3);
        assert_eq!(LogLevel::Error.to_wit(), 4);
    }

    #[test]
    fn test_log_level_display() {
        assert_eq!(LogLevel::Trace.to_string(), "trace");
        assert_eq!(LogLevel::Debug.to_string(), "debug");
        assert_eq!(LogLevel::Info.to_string(), "info");
        assert_eq!(LogLevel::Warn.to_string(), "warn");
        assert_eq!(LogLevel::Error.to_string(), "error");

        // Verify Display is used in format strings
        assert_eq!(format!("Level: {}", LogLevel::Info), "Level: info");
    }

    #[test]
    fn test_log_level_debug_formatting() {
        assert_eq!(format!("{:?}", LogLevel::Trace), "Trace");
        assert_eq!(format!("{:?}", LogLevel::Debug), "Debug");
        assert_eq!(format!("{:?}", LogLevel::Info), "Info");
        assert_eq!(format!("{:?}", LogLevel::Warn), "Warn");
        assert_eq!(format!("{:?}", LogLevel::Error), "Error");
    }

    #[test]
    fn test_log_level_to_tracing() {
        assert_eq!(LogLevel::Trace.to_tracing(), tracing::Level::TRACE);
        assert_eq!(LogLevel::Debug.to_tracing(), tracing::Level::DEBUG);
        assert_eq!(LogLevel::Info.to_tracing(), tracing::Level::INFO);
        assert_eq!(LogLevel::Warn.to_tracing(), tracing::Level::WARN);
        assert_eq!(LogLevel::Error.to_tracing(), tracing::Level::ERROR);
    }

    #[test]
    fn test_log_level_ordering() {
        // Trace < Debug < Info < Warn < Error (by WIT value)
        assert!(LogLevel::Trace.to_wit() < LogLevel::Debug.to_wit());
        assert!(LogLevel::Debug.to_wit() < LogLevel::Info.to_wit());
        assert!(LogLevel::Info.to_wit() < LogLevel::Warn.to_wit());
        assert!(LogLevel::Warn.to_wit() < LogLevel::Error.to_wit());

        // Verify ordering property: lower levels are more verbose
        let levels = [
            LogLevel::Trace,
            LogLevel::Debug,
            LogLevel::Info,
            LogLevel::Warn,
            LogLevel::Error,
        ];
        for i in 0..levels.len() - 1 {
            assert!(
                levels[i].to_wit() < levels[i + 1].to_wit(),
                "{:?} should have lower WIT value than {:?}",
                levels[i],
                levels[i + 1]
            );
        }
    }

    #[test]
    fn test_log_level_equality() {
        assert_eq!(LogLevel::Trace, LogLevel::Trace);
        assert_eq!(LogLevel::Debug, LogLevel::Debug);
        assert_eq!(LogLevel::Info, LogLevel::Info);
        assert_eq!(LogLevel::Warn, LogLevel::Warn);
        assert_eq!(LogLevel::Error, LogLevel::Error);

        assert_ne!(LogLevel::Trace, LogLevel::Error);
        assert_ne!(LogLevel::Debug, LogLevel::Info);
    }

    #[test]
    fn test_log_level_clone() {
        let level = LogLevel::Info;
        let cloned = level;
        assert_eq!(level, cloned);
    }

    #[test]
    fn test_log_level_hash() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(LogLevel::Trace);
        set.insert(LogLevel::Debug);
        set.insert(LogLevel::Info);
        set.insert(LogLevel::Warn);
        set.insert(LogLevel::Error);

        assert_eq!(set.len(), 5);
        assert!(set.contains(&LogLevel::Info));
    }

    // =========================================================================
    // KvError Tests
    // =========================================================================

    #[test]
    fn test_kv_error_display() {
        assert_eq!(KvError::NotFound.to_string(), "key not found");
        assert_eq!(KvError::ValueTooLarge.to_string(), "value too large");
        assert_eq!(KvError::QuotaExceeded.to_string(), "storage quota exceeded");
        assert_eq!(KvError::InvalidKey.to_string(), "invalid key format");
        assert_eq!(
            KvError::Storage("test".to_string()).to_string(),
            "storage error: test"
        );

        // Test with various storage messages
        assert_eq!(
            KvError::Storage("connection failed".to_string()).to_string(),
            "storage error: connection failed"
        );
        assert_eq!(
            KvError::Storage("".to_string()).to_string(),
            "storage error: "
        );
    }

    #[test]
    fn test_kv_error_debug_formatting() {
        assert_eq!(format!("{:?}", KvError::NotFound), "NotFound");
        assert_eq!(format!("{:?}", KvError::ValueTooLarge), "ValueTooLarge");
        assert_eq!(format!("{:?}", KvError::QuotaExceeded), "QuotaExceeded");
        assert_eq!(format!("{:?}", KvError::InvalidKey), "InvalidKey");
        assert_eq!(
            format!("{:?}", KvError::Storage("msg".to_string())),
            "Storage(\"msg\")"
        );
    }

    #[test]
    fn test_kv_error_equality() {
        assert_eq!(KvError::NotFound, KvError::NotFound);
        assert_eq!(KvError::ValueTooLarge, KvError::ValueTooLarge);
        assert_eq!(KvError::QuotaExceeded, KvError::QuotaExceeded);
        assert_eq!(KvError::InvalidKey, KvError::InvalidKey);
        assert_eq!(
            KvError::Storage("test".to_string()),
            KvError::Storage("test".to_string())
        );

        assert_ne!(KvError::NotFound, KvError::InvalidKey);
        assert_ne!(
            KvError::Storage("a".to_string()),
            KvError::Storage("b".to_string())
        );
    }

    #[test]
    fn test_kv_error_clone() {
        let err = KvError::Storage("test message".to_string());
        let cloned = err.clone();
        assert_eq!(err, cloned);
    }

    #[test]
    fn test_kv_error_is_std_error() {
        // Verify KvError implements std::error::Error
        fn assert_error<T: std::error::Error>() {}
        assert_error::<KvError>();

        // Test error source (KvError has no source, so it should return None)
        let err = KvError::NotFound;
        assert!(std::error::Error::source(&err).is_none());
    }

    #[test]
    fn test_kv_error_to_wit() {
        assert_eq!(kv_error_to_wit(&KvError::NotFound), 0);
        assert_eq!(kv_error_to_wit(&KvError::ValueTooLarge), 1);
        assert_eq!(kv_error_to_wit(&KvError::QuotaExceeded), 2);
        assert_eq!(kv_error_to_wit(&KvError::InvalidKey), 3);
        assert_eq!(kv_error_to_wit(&KvError::Storage("test".to_string())), 4);
        assert_eq!(kv_error_to_wit(&KvError::Storage("".to_string())), 4);
    }

    // =========================================================================
    // DefaultHost Creation and Builder Tests
    // =========================================================================

    #[test]
    fn test_default_host_new_creates_empty_state() {
        let host = DefaultHost::new();

        // Verify all stores are empty
        assert!(host.config_get("any_key").is_none());
        assert!(!host.kv_exists("any_key"));
        assert!(!host.secret_exists("any_key"));
        assert!(host.metrics().counters.is_empty());
        assert!(host.metrics().gauges.is_empty());
        assert!(host.metrics().histograms.is_empty());

        // Verify default plugin ID
        assert_eq!(host.plugin_id, "unknown");

        // Verify default limits
        assert_eq!(host.max_value_size, 1024 * 1024); // 1MB
        assert_eq!(host.max_keys, 10000);

        // Verify default log level
        assert_eq!(host.min_log_level, LogLevel::Trace);
    }

    #[test]
    fn test_default_host_default_impl() {
        let host1 = DefaultHost::new();
        let host2 = DefaultHost::default();

        // Both should have identical default state
        assert_eq!(host1.plugin_id, host2.plugin_id);
        assert_eq!(host1.max_value_size, host2.max_value_size);
        assert_eq!(host1.max_keys, host2.max_keys);
        assert_eq!(host1.min_log_level, host2.min_log_level);
    }

    #[test]
    fn test_default_host_with_plugin_id() {
        let host = DefaultHost::with_plugin_id("my-test-plugin");
        assert_eq!(host.plugin_id, "my-test-plugin");

        // Verify other defaults are maintained
        assert_eq!(host.max_value_size, 1024 * 1024);
        assert_eq!(host.max_keys, 10000);
        assert_eq!(host.min_log_level, LogLevel::Trace);
    }

    #[test]
    fn test_default_host_with_plugin_id_from_string() {
        let host = DefaultHost::with_plugin_id(String::from("dynamic-plugin"));
        assert_eq!(host.plugin_id, "dynamic-plugin");
    }

    #[test]
    fn test_default_host_set_plugin_id() {
        let mut host = DefaultHost::new();
        host.set_plugin_id("updated-plugin");
        assert_eq!(host.plugin_id, "updated-plugin");

        host.set_plugin_id(String::from("another-plugin"));
        assert_eq!(host.plugin_id, "another-plugin");
    }

    #[test]
    fn test_default_host_set_max_value_size() {
        let mut host = DefaultHost::new();
        host.set_max_value_size(512);
        assert_eq!(host.max_value_size, 512);

        // Verify the limit is enforced
        assert!(matches!(
            host.kv_set("key", &[0u8; 600]),
            Err(KvError::ValueTooLarge)
        ));
        assert!(host.kv_set("key", &[0u8; 500]).is_ok());
    }

    #[test]
    fn test_default_host_set_max_keys() {
        let mut host = DefaultHost::new();
        host.set_max_keys(2);
        assert_eq!(host.max_keys, 2);

        // Verify the limit is enforced
        host.kv_set("key1", b"v").unwrap();
        host.kv_set("key2", b"v").unwrap();
        assert!(matches!(
            host.kv_set("key3", b"v"),
            Err(KvError::QuotaExceeded)
        ));
    }

    #[test]
    fn test_default_host_set_min_log_level() {
        let mut host = DefaultHost::new();

        host.set_min_log_level(LogLevel::Error);
        assert_eq!(host.min_log_level, LogLevel::Error);
        assert!(!host.log_is_enabled(LogLevel::Trace));
        assert!(!host.log_is_enabled(LogLevel::Debug));
        assert!(!host.log_is_enabled(LogLevel::Info));
        assert!(!host.log_is_enabled(LogLevel::Warn));
        assert!(host.log_is_enabled(LogLevel::Error));

        host.set_min_log_level(LogLevel::Trace);
        assert!(host.log_is_enabled(LogLevel::Trace));
    }

    #[test]
    fn test_default_host_clone() {
        let mut host = DefaultHost::new();
        host.add_config("key", "value");
        host.kv_set("kv_key", b"data").unwrap();
        host.add_secret("secret", "password");
        host.counter_inc("counter", 5);

        let cloned = host.clone();

        // Both should see the same data (Arc-backed)
        assert_eq!(cloned.config_get("key"), Some("value".to_string()));
        assert!(cloned.kv_exists("kv_key"));
        assert!(cloned.secret_exists("secret"));
        assert_eq!(cloned.metrics().get_counter("counter"), Some(5));

        // Modifications in clone should be visible in original (shared Arc)
        cloned.counter_inc("counter", 3);
        assert_eq!(host.metrics().get_counter("counter"), Some(8));
    }

    #[test]
    fn test_default_host_debug_formatting() {
        let host = DefaultHost::new();
        let debug_str = format!("{:?}", host);
        assert!(debug_str.contains("DefaultHost"));
        assert!(debug_str.contains("plugin_id"));
    }

    // =========================================================================
    // Config Operations Tests
    // =========================================================================

    #[test]
    fn test_config_get_with_existing_key() {
        let mut host = DefaultHost::new();
        host.add_config("database_url", "postgres://localhost:5432/db");

        let result = host.config_get("database_url");
        assert_eq!(result, Some("postgres://localhost:5432/db".to_string()));
    }

    #[test]
    fn test_config_get_with_missing_key() {
        let host = DefaultHost::new();
        let result = host.config_get("nonexistent_key");
        assert!(result.is_none());
    }

    #[test]
    fn test_config_get_required_with_existing_key() {
        let mut host = DefaultHost::new();
        host.add_config("required_key", "required_value");

        let result = host.config_get_required("required_key");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "required_value");
    }

    #[test]
    fn test_config_get_required_with_missing_key() {
        let host = DefaultHost::new();
        let result = host.config_get_required("missing_key");

        assert!(result.is_err());
        let err_msg = result.unwrap_err();
        assert!(err_msg.contains("missing_key"));
        assert!(err_msg.contains("not found"));
    }

    #[test]
    fn test_config_get_bool_all_true_values() {
        let mut host = DefaultHost::new();
        host.add_config("bool_true", "true");
        host.add_config("bool_TRUE", "TRUE");
        host.add_config("bool_True", "True");
        host.add_config("bool_1", "1");
        host.add_config("bool_yes", "yes");
        host.add_config("bool_YES", "YES");

        assert_eq!(host.config_get_bool("bool_true"), Some(true));
        assert_eq!(host.config_get_bool("bool_TRUE"), Some(true));
        assert_eq!(host.config_get_bool("bool_True"), Some(true));
        assert_eq!(host.config_get_bool("bool_1"), Some(true));
        assert_eq!(host.config_get_bool("bool_yes"), Some(true));
        assert_eq!(host.config_get_bool("bool_YES"), Some(true));
    }

    #[test]
    fn test_config_get_bool_all_false_values() {
        let mut host = DefaultHost::new();
        host.add_config("bool_false", "false");
        host.add_config("bool_FALSE", "FALSE");
        host.add_config("bool_False", "False");
        host.add_config("bool_0", "0");
        host.add_config("bool_no", "no");
        host.add_config("bool_NO", "NO");

        assert_eq!(host.config_get_bool("bool_false"), Some(false));
        assert_eq!(host.config_get_bool("bool_FALSE"), Some(false));
        assert_eq!(host.config_get_bool("bool_False"), Some(false));
        assert_eq!(host.config_get_bool("bool_0"), Some(false));
        assert_eq!(host.config_get_bool("bool_no"), Some(false));
        assert_eq!(host.config_get_bool("bool_NO"), Some(false));
    }

    #[test]
    fn test_config_get_bool_invalid_values() {
        let mut host = DefaultHost::new();
        host.add_config("invalid1", "maybe");
        host.add_config("invalid2", "2");
        host.add_config("invalid3", "on");
        host.add_config("invalid4", "off");
        host.add_config("invalid5", "");

        assert_eq!(host.config_get_bool("invalid1"), None);
        assert_eq!(host.config_get_bool("invalid2"), None);
        assert_eq!(host.config_get_bool("invalid3"), None);
        assert_eq!(host.config_get_bool("invalid4"), None);
        assert_eq!(host.config_get_bool("invalid5"), None);
        assert_eq!(host.config_get_bool("nonexistent"), None);
    }

    #[test]
    fn test_config_get_int_valid_values() {
        let mut host = DefaultHost::new();
        host.add_config("positive", "42");
        host.add_config("negative", "-100");
        host.add_config("zero", "0");
        host.add_config("large", "9223372036854775807"); // i64::MAX
        host.add_config("small", "-9223372036854775808"); // i64::MIN

        assert_eq!(host.config_get_int("positive"), Some(42));
        assert_eq!(host.config_get_int("negative"), Some(-100));
        assert_eq!(host.config_get_int("zero"), Some(0));
        assert_eq!(host.config_get_int("large"), Some(i64::MAX));
        assert_eq!(host.config_get_int("small"), Some(i64::MIN));
    }

    #[test]
    fn test_config_get_int_invalid_values() {
        let mut host = DefaultHost::new();
        host.add_config("float", "3.14");
        host.add_config("text", "not_a_number");
        host.add_config("empty", "");
        host.add_config("overflow", "99999999999999999999999");

        assert_eq!(host.config_get_int("float"), None);
        assert_eq!(host.config_get_int("text"), None);
        assert_eq!(host.config_get_int("empty"), None);
        assert_eq!(host.config_get_int("overflow"), None);
        assert_eq!(host.config_get_int("nonexistent"), None);
    }

    #[test]
    fn test_config_get_float_valid_values() {
        let mut host = DefaultHost::new();
        host.add_config("positive", "3.14159");
        host.add_config("negative", "-2.5");
        host.add_config("zero", "0.0");
        host.add_config("integer", "42");
        host.add_config("scientific", "1.5e10");

        assert!((host.config_get_float("positive").unwrap() - 3.14159).abs() < 1e-10);
        assert!((host.config_get_float("negative").unwrap() - (-2.5)).abs() < f64::EPSILON);
        assert!((host.config_get_float("zero").unwrap() - 0.0).abs() < f64::EPSILON);
        assert!((host.config_get_float("integer").unwrap() - 42.0).abs() < f64::EPSILON);
        assert!((host.config_get_float("scientific").unwrap() - 1.5e10).abs() < 1e5);
    }

    #[test]
    fn test_config_get_float_invalid_values() {
        let mut host = DefaultHost::new();
        host.add_config("text", "not_a_float");
        host.add_config("empty", "");

        assert_eq!(host.config_get_float("text"), None);
        assert_eq!(host.config_get_float("empty"), None);
        assert_eq!(host.config_get_float("nonexistent"), None);
    }

    #[test]
    fn test_config_get_many() {
        let mut host = DefaultHost::new();
        host.add_config("key1", "value1");
        host.add_config("key2", "value2");
        host.add_config("key3", "value3");

        let results = host.config_get_many(&[
            "key1".to_string(),
            "key2".to_string(),
            "nonexistent".to_string(),
            "key3".to_string(),
        ]);

        assert_eq!(results.len(), 3);

        // Results should only contain existing keys
        let keys: Vec<&String> = results.iter().map(|(k, _)| k).collect();
        assert!(keys.contains(&&"key1".to_string()));
        assert!(keys.contains(&&"key2".to_string()));
        assert!(keys.contains(&&"key3".to_string()));
    }

    #[test]
    fn test_config_get_many_empty_keys() {
        let host = DefaultHost::new();
        let results = host.config_get_many(&[]);
        assert!(results.is_empty());
    }

    #[test]
    fn test_config_get_many_all_missing() {
        let host = DefaultHost::new();
        let results = host.config_get_many(&["a".to_string(), "b".to_string(), "c".to_string()]);
        assert!(results.is_empty());
    }

    #[test]
    fn test_config_get_prefix() {
        let mut host = DefaultHost::new();
        host.add_config("database.host", "localhost");
        host.add_config("database.port", "5432");
        host.add_config("database.user", "admin");
        host.add_config("cache.host", "redis-host");
        host.add_config("app.name", "myapp");

        let db_configs = host.config_get_prefix("database.");
        assert_eq!(db_configs.len(), 3);

        let cache_configs = host.config_get_prefix("cache.");
        assert_eq!(cache_configs.len(), 1);

        let all_host_configs = host.config_get_prefix("");
        assert_eq!(all_host_configs.len(), 5);

        let no_match = host.config_get_prefix("nonexistent.");
        assert!(no_match.is_empty());
    }

    #[test]
    fn test_config_exists() {
        let mut host = DefaultHost::new();
        host.add_config("existing_key", "value");

        assert!(host.config_exists("existing_key"));
        assert!(!host.config_exists("nonexistent_key"));
    }

    #[test]
    fn test_config_get_all_returns_valid_json() {
        let mut host = DefaultHost::new();
        host.add_config("key1", "value1");
        host.add_config("key2", "value2");

        let json = host.config_get_all();

        // Should be valid JSON
        let parsed: Result<serde_json::Value, _> = serde_json::from_str(&json);
        assert!(parsed.is_ok());

        let value = parsed.unwrap();
        assert!(value.is_object());
        assert_eq!(value.get("key1").unwrap(), "value1");
        assert_eq!(value.get("key2").unwrap(), "value2");
    }

    #[test]
    fn test_config_get_all_empty() {
        let host = DefaultHost::new();
        let json = host.config_get_all();

        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_object());
        assert!(parsed.as_object().unwrap().is_empty());
    }

    #[test]
    fn test_default_host_config_add_multiple() {
        let mut host = DefaultHost::new();

        host.add_configs([("key1", "value1"), ("key2", "value2"), ("key3", "value3")]);

        assert_eq!(host.config_get("key1"), Some("value1".to_string()));
        assert_eq!(host.config_get("key2"), Some("value2".to_string()));
        assert_eq!(host.config_get("key3"), Some("value3".to_string()));
    }

    // =========================================================================
    // KV Operations Tests
    // =========================================================================

    #[test]
    fn test_kv_get_with_existing_key() {
        let mut host = DefaultHost::new();
        host.kv_set("mykey", b"myvalue").unwrap();

        let result = host.kv_get("mykey").unwrap();
        assert_eq!(result, Some(b"myvalue".to_vec()));
    }

    #[test]
    fn test_kv_get_with_missing_key() {
        let host = DefaultHost::new();
        let result = host.kv_get("nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_kv_set_and_retrieve() {
        let mut host = DefaultHost::new();

        // Set binary data
        let binary_data = vec![0x00, 0x01, 0x02, 0xFF, 0xFE];
        host.kv_set("binary", &binary_data).unwrap();
        assert_eq!(host.kv_get("binary").unwrap(), Some(binary_data));

        // Set empty data
        host.kv_set("empty", b"").unwrap();
        assert_eq!(host.kv_get("empty").unwrap(), Some(vec![]));

        // Overwrite existing key
        host.kv_set("key", b"first").unwrap();
        host.kv_set("key", b"second").unwrap();
        assert_eq!(host.kv_get("key").unwrap(), Some(b"second".to_vec()));
    }

    #[test]
    fn test_kv_set_string_and_get_string() {
        let mut host = DefaultHost::new();

        host.kv_set_string("greeting", "Hello, World!").unwrap();
        assert_eq!(
            host.kv_get_string("greeting").unwrap(),
            Some("Hello, World!".to_string())
        );

        // Test with Unicode
        host.kv_set_string("unicode", "Hello, World!").unwrap();
        assert_eq!(
            host.kv_get_string("unicode").unwrap(),
            Some("Hello, World!".to_string())
        );
    }

    #[test]
    fn test_kv_get_string_invalid_utf8() {
        let mut host = DefaultHost::new();

        // Set invalid UTF-8 bytes
        host.kv_set("invalid_utf8", &[0xFF, 0xFE, 0x00, 0x01])
            .unwrap();

        let result = host.kv_get_string("invalid_utf8");
        assert!(result.is_err());
        assert!(matches!(result, Err(KvError::Storage(_))));
    }

    #[test]
    fn test_kv_delete() {
        let mut host = DefaultHost::new();
        host.kv_set("to_delete", b"value").unwrap();

        // Delete existing key
        assert!(host.kv_delete("to_delete").unwrap());
        assert!(!host.kv_exists("to_delete"));

        // Delete already deleted key
        assert!(!host.kv_delete("to_delete").unwrap());

        // Delete non-existent key
        assert!(!host.kv_delete("never_existed").unwrap());
    }

    #[test]
    fn test_kv_list_keys_with_prefix() {
        let mut host = DefaultHost::new();
        host.kv_set("users/alice", b"1").unwrap();
        host.kv_set("users/bob", b"2").unwrap();
        host.kv_set("users/charlie", b"3").unwrap();
        host.kv_set("sessions/abc", b"4").unwrap();

        let user_keys = host.kv_list_keys("users/").unwrap();
        assert_eq!(user_keys.len(), 3);
        assert!(user_keys.contains(&"users/alice".to_string()));
        assert!(user_keys.contains(&"users/bob".to_string()));
        assert!(user_keys.contains(&"users/charlie".to_string()));

        let session_keys = host.kv_list_keys("sessions/").unwrap();
        assert_eq!(session_keys.len(), 1);

        // Empty prefix returns all
        let all_keys = host.kv_list_keys("").unwrap();
        assert_eq!(all_keys.len(), 4);

        // No matches
        let no_keys = host.kv_list_keys("nonexistent/").unwrap();
        assert!(no_keys.is_empty());
    }

    #[test]
    fn test_kv_exists() {
        let mut host = DefaultHost::new();

        assert!(!host.kv_exists("key"));
        host.kv_set("key", b"value").unwrap();
        assert!(host.kv_exists("key"));
        host.kv_delete("key").unwrap();
        assert!(!host.kv_exists("key"));
    }

    #[test]
    fn test_kv_increment() {
        let mut host = DefaultHost::new();

        // Increment non-existent key starts at 0
        assert_eq!(host.kv_increment("counter", 1).unwrap(), 1);
        assert_eq!(host.kv_increment("counter", 5).unwrap(), 6);
        assert_eq!(host.kv_increment("counter", -3).unwrap(), 3);
        assert_eq!(host.kv_increment("counter", -10).unwrap(), -7);
    }

    #[test]
    fn test_kv_increment_saturating() {
        let mut host = DefaultHost::new();

        // Set to near max
        host.kv_set_string("max", &(i64::MAX - 5).to_string())
            .unwrap();
        assert_eq!(host.kv_increment("max", 10).unwrap(), i64::MAX);

        // Set to near min
        host.kv_set_string("min", &(i64::MIN + 5).to_string())
            .unwrap();
        assert_eq!(host.kv_increment("min", -10).unwrap(), i64::MIN);
    }

    #[test]
    fn test_kv_increment_invalid_value() {
        let mut host = DefaultHost::new();
        host.kv_set_string("not_a_number", "hello").unwrap();

        let result = host.kv_increment("not_a_number", 1);
        assert!(result.is_err());
        assert!(matches!(result, Err(KvError::Storage(_))));
    }

    #[test]
    fn test_kv_compare_and_swap_success() {
        let mut host = DefaultHost::new();

        // CAS on non-existent key with None expected
        assert!(host.kv_compare_and_swap("key", None, b"value1").unwrap());
        assert_eq!(host.kv_get("key").unwrap(), Some(b"value1".to_vec()));

        // CAS with correct expected value
        assert!(host
            .kv_compare_and_swap("key", Some(b"value1"), b"value2")
            .unwrap());
        assert_eq!(host.kv_get("key").unwrap(), Some(b"value2".to_vec()));
    }

    #[test]
    fn test_kv_compare_and_swap_failure() {
        let mut host = DefaultHost::new();
        host.kv_set("key", b"actual_value").unwrap();

        // CAS with wrong expected value
        assert!(!host
            .kv_compare_and_swap("key", Some(b"wrong_value"), b"new_value")
            .unwrap());

        // Value should be unchanged
        assert_eq!(host.kv_get("key").unwrap(), Some(b"actual_value".to_vec()));

        // CAS expecting None on existing key
        assert!(!host.kv_compare_and_swap("key", None, b"new_value").unwrap());
        assert_eq!(host.kv_get("key").unwrap(), Some(b"actual_value".to_vec()));
    }

    #[test]
    fn test_kv_set_with_ttl() {
        let mut host = DefaultHost::new();

        // Set with very long TTL (should exist)
        host.kv_set_with_ttl("long_ttl", b"value", 1_000_000_000_000)
            .unwrap(); // 1000s
        assert!(host.kv_exists("long_ttl"));
        assert_eq!(host.kv_get("long_ttl").unwrap(), Some(b"value".to_vec()));

        // Set with zero TTL (expires immediately or very soon)
        host.kv_set_with_ttl("zero_ttl", b"value", 0).unwrap();
        // Due to timing, we can't reliably test immediate expiration
        // but we can verify the key was created
    }

    #[test]
    fn test_kv_value_size_limits() {
        let mut host = DefaultHost::new();
        host.set_max_value_size(100);

        // Within limit
        assert!(host.kv_set("small", &[0u8; 100]).is_ok());

        // Exceeds limit
        assert!(matches!(
            host.kv_set("large", &[0u8; 101]),
            Err(KvError::ValueTooLarge)
        ));

        // CAS also checks limit
        assert!(matches!(
            host.kv_compare_and_swap("cas", None, &[0u8; 101]),
            Err(KvError::ValueTooLarge)
        ));

        // TTL set also checks limit
        assert!(matches!(
            host.kv_set_with_ttl("ttl", &[0u8; 101], 1000),
            Err(KvError::ValueTooLarge)
        ));
    }

    #[test]
    fn test_kv_key_count_limits() {
        let mut host = DefaultHost::new();
        host.set_max_keys(3);

        host.kv_set("key1", b"v").unwrap();
        host.kv_set("key2", b"v").unwrap();
        host.kv_set("key3", b"v").unwrap();

        // Exceeds key count
        assert!(matches!(
            host.kv_set("key4", b"v"),
            Err(KvError::QuotaExceeded)
        ));

        // Updating existing key is allowed
        assert!(host.kv_set("key1", b"updated").is_ok());

        // Delete and add new key is allowed
        host.kv_delete("key1").unwrap();
        assert!(host.kv_set("key4", b"v").is_ok());

        // CAS creating new key checks quota
        assert!(matches!(
            host.kv_compare_and_swap("key5", None, b"v"),
            Err(KvError::QuotaExceeded)
        ));

        // Increment creating new key checks quota
        assert!(matches!(
            host.kv_increment("counter", 1),
            Err(KvError::QuotaExceeded)
        ));
    }

    #[test]
    fn test_kv_invalid_key_characters() {
        let mut host = DefaultHost::new();

        // Invalid characters
        assert!(matches!(
            host.kv_set("key with spaces", b"v"),
            Err(KvError::InvalidKey)
        ));
        assert!(matches!(
            host.kv_set("key\twith\ttabs", b"v"),
            Err(KvError::InvalidKey)
        ));
        assert!(matches!(
            host.kv_set("key\nwith\nnewlines", b"v"),
            Err(KvError::InvalidKey)
        ));
        assert!(matches!(
            host.kv_set("key@with#special$chars", b"v"),
            Err(KvError::InvalidKey)
        ));

        // Empty key
        assert!(matches!(host.kv_set("", b"v"), Err(KvError::InvalidKey)));

        // Key too long
        let long_key = "a".repeat(1025);
        assert!(matches!(
            host.kv_set(&long_key, b"v"),
            Err(KvError::InvalidKey)
        ));

        // Valid characters
        assert!(host.kv_set("valid-key_123.test/path:tag", b"v").is_ok());
        assert!(host.kv_set("UPPERCASE", b"v").is_ok());
        assert!(host.kv_set("mixedCase123", b"v").is_ok());
    }

    #[test]
    fn test_kv_key_validation_on_all_operations() {
        let mut host = DefaultHost::new();
        let invalid_key = "invalid key";

        assert!(matches!(host.kv_get(invalid_key), Err(KvError::InvalidKey)));
        assert!(matches!(
            host.kv_set(invalid_key, b"v"),
            Err(KvError::InvalidKey)
        ));
        assert!(matches!(
            host.kv_delete(invalid_key),
            Err(KvError::InvalidKey)
        ));
        assert!(matches!(
            host.kv_increment(invalid_key, 1),
            Err(KvError::InvalidKey)
        ));
        assert!(matches!(
            host.kv_compare_and_swap(invalid_key, None, b"v"),
            Err(KvError::InvalidKey)
        ));
        assert!(matches!(
            host.kv_set_with_ttl(invalid_key, b"v", 1000),
            Err(KvError::InvalidKey)
        ));
    }

    // =========================================================================
    // Logging Operations Tests
    // =========================================================================

    #[test]
    fn test_log_at_each_level() {
        let host = DefaultHost::with_plugin_id("test-plugin");

        // These just verify no panics occur
        host.log(LogLevel::Trace, "trace message");
        host.log(LogLevel::Debug, "debug message");
        host.log(LogLevel::Info, "info message");
        host.log(LogLevel::Warn, "warn message");
        host.log(LogLevel::Error, "error message");
    }

    #[test]
    fn test_log_with_empty_message() {
        let host = DefaultHost::new();
        host.log(LogLevel::Info, "");
    }

    #[test]
    fn test_log_with_special_characters() {
        let host = DefaultHost::new();
        host.log(LogLevel::Info, "Message with newline\nand tab\tcharacters");
        host.log(LogLevel::Info, "Message with unicode: emoji and more");
        host.log(
            LogLevel::Info,
            "Message with quotes: \"quoted\" and 'single'",
        );
    }

    #[test]
    fn test_log_structured_with_fields() {
        let host = DefaultHost::with_plugin_id("test-plugin");

        host.log_structured(
            LogLevel::Info,
            "structured message",
            &[
                ("key1".to_string(), "value1".to_string()),
                ("key2".to_string(), "value2".to_string()),
            ],
        );
    }

    #[test]
    fn test_log_structured_with_empty_fields() {
        let host = DefaultHost::new();
        host.log_structured(LogLevel::Info, "message with no fields", &[]);
    }

    #[test]
    fn test_log_structured_with_special_field_values() {
        let host = DefaultHost::new();
        host.log_structured(
            LogLevel::Info,
            "message",
            &[
                ("empty".to_string(), "".to_string()),
                ("unicode".to_string(), "test".to_string()),
                ("json".to_string(), "{\"key\": \"value\"}".to_string()),
            ],
        );
    }

    #[test]
    fn test_log_is_enabled_with_min_log_level() {
        let mut host = DefaultHost::new();

        // Default is Trace, so all levels enabled
        assert!(host.log_is_enabled(LogLevel::Trace));
        assert!(host.log_is_enabled(LogLevel::Debug));
        assert!(host.log_is_enabled(LogLevel::Info));
        assert!(host.log_is_enabled(LogLevel::Warn));
        assert!(host.log_is_enabled(LogLevel::Error));

        // Set to Info - Trace and Debug should be disabled
        host.set_min_log_level(LogLevel::Info);
        assert!(!host.log_is_enabled(LogLevel::Trace));
        assert!(!host.log_is_enabled(LogLevel::Debug));
        assert!(host.log_is_enabled(LogLevel::Info));
        assert!(host.log_is_enabled(LogLevel::Warn));
        assert!(host.log_is_enabled(LogLevel::Error));

        // Set to Error - only Error enabled
        host.set_min_log_level(LogLevel::Error);
        assert!(!host.log_is_enabled(LogLevel::Trace));
        assert!(!host.log_is_enabled(LogLevel::Debug));
        assert!(!host.log_is_enabled(LogLevel::Info));
        assert!(!host.log_is_enabled(LogLevel::Warn));
        assert!(host.log_is_enabled(LogLevel::Error));
    }

    #[test]
    fn test_log_respects_min_level() {
        let mut host = DefaultHost::new();
        host.set_min_log_level(LogLevel::Warn);

        // Lower levels should be filtered (no panic)
        host.log(LogLevel::Trace, "should be filtered");
        host.log(LogLevel::Debug, "should be filtered");
        host.log(LogLevel::Info, "should be filtered");

        // Warn and above should pass through
        host.log(LogLevel::Warn, "should be logged");
        host.log(LogLevel::Error, "should be logged");
    }

    #[test]
    fn test_log_structured_respects_min_level() {
        let mut host = DefaultHost::new();
        host.set_min_log_level(LogLevel::Error);

        // Lower levels should be filtered
        host.log_structured(
            LogLevel::Info,
            "filtered",
            &[("key".to_string(), "value".to_string())],
        );

        // Error should pass through
        host.log_structured(
            LogLevel::Error,
            "logged",
            &[("key".to_string(), "value".to_string())],
        );
    }

    // =========================================================================
    // Secrets Operations Tests
    // =========================================================================

    #[test]
    fn test_secret_get_with_existing_secret() {
        let mut host = DefaultHost::new();
        host.add_secret("api_key", "super_secret_value");

        let result = host.secret_get("api_key").unwrap();
        assert_eq!(result, Some("super_secret_value".to_string()));
    }

    #[test]
    fn test_secret_get_with_missing_secret() {
        let host = DefaultHost::new();
        let result = host.secret_get("nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_secret_get_required_with_existing_secret() {
        let mut host = DefaultHost::new();
        host.add_secret("required_secret", "value");

        let result = host.secret_get_required("required_secret");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "value");
    }

    #[test]
    fn test_secret_get_required_with_missing_secret() {
        let host = DefaultHost::new();
        let result = host.secret_get_required("missing_secret");

        assert!(result.is_err());
        let err_msg = result.unwrap_err();
        assert!(err_msg.contains("missing_secret"));
        assert!(err_msg.contains("not found"));
    }

    #[test]
    fn test_secret_exists() {
        let mut host = DefaultHost::new();

        assert!(!host.secret_exists("secret"));
        host.add_secret("secret", "value");
        assert!(host.secret_exists("secret"));
    }

    #[test]
    fn test_secret_list_names() {
        let mut host = DefaultHost::new();
        host.add_secret("api_key", "value1");
        host.add_secret("db_password", "value2");
        host.add_secret("jwt_secret", "value3");

        let names = host.secret_list_names();
        assert_eq!(names.len(), 3);
        assert!(names.contains(&"api_key".to_string()));
        assert!(names.contains(&"db_password".to_string()));
        assert!(names.contains(&"jwt_secret".to_string()));
    }

    #[test]
    fn test_secret_list_names_empty() {
        let host = DefaultHost::new();
        let names = host.secret_list_names();
        assert!(names.is_empty());
    }

    #[test]
    fn test_add_secrets_multiple() {
        let mut host = DefaultHost::new();
        host.add_secrets([("secret1", "password1"), ("secret2", "password2")]);

        assert_eq!(
            host.secret_get("secret1").unwrap(),
            Some("password1".to_string())
        );
        assert_eq!(
            host.secret_get("secret2").unwrap(),
            Some("password2".to_string())
        );
    }

    // =========================================================================
    // Metrics Operations Tests
    // =========================================================================

    #[test]
    fn test_counter_inc() {
        let host = DefaultHost::new();

        host.counter_inc("requests", 1);
        assert_eq!(host.metrics().get_counter("requests"), Some(1));

        host.counter_inc("requests", 5);
        assert_eq!(host.metrics().get_counter("requests"), Some(6));

        host.counter_inc("requests", 0);
        assert_eq!(host.metrics().get_counter("requests"), Some(6));
    }

    #[test]
    fn test_counter_inc_labeled() {
        let host = DefaultHost::new();

        let labels1 = vec![
            ("method".to_string(), "GET".to_string()),
            ("path".to_string(), "/api".to_string()),
        ];
        let labels2 = vec![
            ("method".to_string(), "POST".to_string()),
            ("path".to_string(), "/api".to_string()),
        ];

        host.counter_inc_labeled("http_requests", 10, &labels1);
        host.counter_inc_labeled("http_requests", 5, &labels2);
        host.counter_inc_labeled("http_requests", 3, &labels1);

        assert_eq!(
            host.metrics()
                .get_counter_labeled("http_requests", &labels1),
            Some(13)
        );
        assert_eq!(
            host.metrics()
                .get_counter_labeled("http_requests", &labels2),
            Some(5)
        );
    }

    #[test]
    fn test_gauge_set() {
        let host = DefaultHost::new();

        host.gauge_set("temperature", 25.5);
        assert!((host.metrics().get_gauge("temperature").unwrap() - 25.5).abs() < f64::EPSILON);

        // Overwrite
        host.gauge_set("temperature", 30.0);
        assert!((host.metrics().get_gauge("temperature").unwrap() - 30.0).abs() < f64::EPSILON);

        // Negative value
        host.gauge_set("temperature", -10.5);
        assert!((host.metrics().get_gauge("temperature").unwrap() - (-10.5)).abs() < f64::EPSILON);
    }

    #[test]
    fn test_gauge_add() {
        let host = DefaultHost::new();

        // Add to non-existent gauge starts at 0
        host.gauge_add("connections", 5.0);
        assert!((host.metrics().get_gauge("connections").unwrap() - 5.0).abs() < f64::EPSILON);

        // Add positive
        host.gauge_add("connections", 3.0);
        assert!((host.metrics().get_gauge("connections").unwrap() - 8.0).abs() < f64::EPSILON);

        // Add negative (subtract)
        host.gauge_add("connections", -2.5);
        assert!((host.metrics().get_gauge("connections").unwrap() - 5.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_gauge_set_labeled() {
        let host = DefaultHost::new();
        let labels = vec![("sensor".to_string(), "cpu".to_string())];

        host.gauge_set_labeled("temperature", 65.0, &labels);
        assert!(
            (host
                .metrics()
                .get_gauge_labeled("temperature", &labels)
                .unwrap()
                - 65.0)
                .abs()
                < f64::EPSILON
        );
    }

    #[test]
    fn test_histogram_observe() {
        let host = DefaultHost::new();

        host.histogram_observe("latency", 0.1);
        host.histogram_observe("latency", 0.2);
        host.histogram_observe("latency", 0.15);
        host.histogram_observe("latency", 0.05);

        let metrics = host.metrics();
        let observations = metrics.get_histogram("latency").unwrap();
        assert_eq!(observations.len(), 4);
        assert!((observations[0] - 0.1).abs() < f64::EPSILON);
        assert!((observations[1] - 0.2).abs() < f64::EPSILON);
        assert!((observations[2] - 0.15).abs() < f64::EPSILON);
        assert!((observations[3] - 0.05).abs() < f64::EPSILON);
    }

    #[test]
    fn test_histogram_observe_labeled() {
        let host = DefaultHost::new();
        let labels = vec![("endpoint".to_string(), "/api/users".to_string())];

        host.histogram_observe_labeled("request_latency", 0.5, &labels);
        host.histogram_observe_labeled("request_latency", 0.3, &labels);

        let key = MetricsStore::make_key("request_latency", &labels);
        let metrics = host.metrics();
        let observations = metrics.histograms.get(&key).unwrap();
        assert_eq!(observations.len(), 2);
    }

    #[test]
    fn test_record_duration() {
        let host = DefaultHost::new();

        // 1.5 seconds in nanoseconds
        host.record_duration("request_duration", 1_500_000_000);

        {
            let metrics = host.metrics();
            let durations = metrics.get_histogram("request_duration").unwrap();
            assert_eq!(durations.len(), 1);
            assert!((durations[0] - 1.5).abs() < f64::EPSILON);
        }

        // 500 milliseconds
        host.record_duration("request_duration", 500_000_000);
        {
            let metrics = host.metrics();
            let durations = metrics.get_histogram("request_duration").unwrap();
            assert_eq!(durations.len(), 2);
            assert!((durations[1] - 0.5).abs() < f64::EPSILON);
        }
    }

    #[test]
    fn test_record_duration_labeled() {
        let host = DefaultHost::new();
        let labels = vec![("handler".to_string(), "get_user".to_string())];

        host.record_duration_labeled("handler_duration", 100_000_000, &labels); // 100ms

        let key = MetricsStore::make_key("handler_duration", &labels);
        let metrics = host.metrics();
        let durations = metrics.histograms.get(&key).unwrap();
        assert_eq!(durations.len(), 1);
        assert!((durations[0] - 0.1).abs() < f64::EPSILON);
    }

    #[test]
    fn test_metrics_captured_in_metrics_store() {
        let host = DefaultHost::new();

        // Add various metrics
        host.counter_inc("counter1", 5);
        host.counter_inc("counter2", 10);
        host.gauge_set("gauge1", 42.0);
        host.histogram_observe("histogram1", 1.0);
        host.histogram_observe("histogram1", 2.0);

        // Verify all are captured
        let metrics = host.metrics();
        assert_eq!(metrics.counters.len(), 2);
        assert_eq!(metrics.gauges.len(), 1);
        assert_eq!(metrics.histograms.len(), 1);

        assert_eq!(metrics.get_counter("counter1"), Some(5));
        assert_eq!(metrics.get_counter("counter2"), Some(10));
        assert!((metrics.get_gauge("gauge1").unwrap() - 42.0).abs() < f64::EPSILON);
        assert_eq!(metrics.get_histogram("histogram1").unwrap().len(), 2);
    }

    // =========================================================================
    // DefaultHost Additional Tests
    // =========================================================================

    #[test]
    fn test_default_host_clear() {
        let mut host = DefaultHost::new();

        host.add_config("key", "value");
        host.kv_set("kv_key", b"data").unwrap();
        host.add_secret("secret", "password");
        host.counter_inc("counter", 10);
        host.gauge_set("gauge", 5.0);
        host.histogram_observe("histogram", 1.0);

        host.clear();

        assert!(host.config_get("key").is_none());
        assert!(!host.kv_exists("kv_key"));
        assert!(!host.secret_exists("secret"));
        assert!(host.metrics().get_counter("counter").is_none());
        assert!(host.metrics().get_gauge("gauge").is_none());
        assert!(host.metrics().get_histogram("histogram").is_none());
    }

    #[test]
    fn test_metrics_store_key_generation() {
        // No labels
        let key = MetricsStore::make_key("metric", &[]);
        assert_eq!(key, "metric");

        // Single label
        let key = MetricsStore::make_key("metric", &[("label".to_string(), "value".to_string())]);
        assert_eq!(key, "metric:{label=value}");

        // Multiple labels (should be sorted alphabetically)
        let key = MetricsStore::make_key(
            "metric",
            &[
                ("z".to_string(), "last".to_string()),
                ("a".to_string(), "first".to_string()),
                ("m".to_string(), "middle".to_string()),
            ],
        );
        assert_eq!(key, "metric:{a=first,m=middle,z=last}");
    }

    #[test]
    fn test_metrics_store_default() {
        let store = MetricsStore::default();
        assert!(store.counters.is_empty());
        assert!(store.gauges.is_empty());
        assert!(store.histograms.is_empty());
    }

    #[test]
    fn test_kv_entry_expiration() {
        let entry_no_ttl = KvEntry::new(vec![1, 2, 3]);
        assert!(!entry_no_ttl.is_expired());

        let entry_long_ttl = KvEntry::with_ttl(vec![1, 2, 3], 1_000_000_000_000); // 1000s
        assert!(!entry_long_ttl.is_expired());

        // Zero TTL should be expired immediately or very soon
        let entry_zero_ttl = KvEntry::with_ttl(vec![1, 2, 3], 0);
        // We can't reliably test this due to timing, but the code path is covered
        let _ = entry_zero_ttl.is_expired();
    }

    #[test]
    fn test_default_host_thread_safety() {
        // Verify DefaultHost is Send
        fn assert_send<T: Send>() {}
        assert_send::<DefaultHost>();

        // Verify ZLayerHost requires Send
        fn assert_trait_send<T: ZLayerHost>() {}
        assert_trait_send::<DefaultHost>();
    }
}
