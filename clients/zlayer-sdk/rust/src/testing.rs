//! Testing utilities for ZLayer plugins.
//!
//! This module provides mock implementations of ZLayer host functions
//! for unit testing plugins without running them in the actual runtime.
//!
//! # Example
//!
//! ```rust,ignore
//! use zlayer_sdk::testing::{MockHost, LogLevel};
//! use zlayer_sdk::zlayer_test;
//!
//! zlayer_test!(test_my_plugin, |host| {
//!     host.set_config("api_key", "test-key")
//!         .set_secret("db_password", "secret123");
//! }, |host| {
//!     // Test your plugin logic here
//!     assert!(host.has_log(LogLevel::Info, "initialized"));
//! });
//! ```

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Log level for mock logging.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LogLevel {
    /// Trace level logging (most verbose).
    Trace,
    /// Debug level logging.
    Debug,
    /// Info level logging.
    Info,
    /// Warning level logging.
    Warn,
    /// Error level logging (least verbose).
    Error,
}

impl LogLevel {
    /// Returns the string representation of the log level.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Trace => "TRACE",
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
        }
    }
}

/// Mock host for testing `ZLayer` plugins.
///
/// This struct simulates the `ZLayer` host environment, allowing you to:
/// - Set mock configuration values
/// - Set mock KV storage values
/// - Set mock secrets
/// - Capture and inspect log messages
///
/// # Example
///
/// ```rust,ignore
/// use zlayer_sdk::testing::{MockHost, LogLevel};
///
/// let mut host = MockHost::new();
/// host.set_config("timeout", "30")
///     .set_kv_string("cache", "user:1", "{\"name\": \"Alice\"}");
///
/// // Run your plugin logic...
///
/// assert!(host.has_log(LogLevel::Info, "request processed"));
/// ```
pub struct MockHost {
    config: BTreeMap<String, String>,
    kv: BTreeMap<(String, String), Vec<u8>>,
    secrets: BTreeMap<String, String>,
    logs: Vec<(LogLevel, String)>,
    http_responses: BTreeMap<String, MockHttpResponse>,
}

/// A mock HTTP response for testing outbound HTTP calls.
#[derive(Debug, Clone)]
pub struct MockHttpResponse {
    /// HTTP status code.
    pub status: u16,
    /// Response headers.
    pub headers: BTreeMap<String, String>,
    /// Response body.
    pub body: Vec<u8>,
}

impl MockHttpResponse {
    /// Create a new mock HTTP response with the given status code.
    #[must_use]
    pub fn new(status: u16) -> Self {
        Self {
            status,
            headers: BTreeMap::new(),
            body: Vec::new(),
        }
    }

    /// Create a successful (200 OK) response with the given body.
    #[must_use]
    pub fn ok(body: impl AsRef<[u8]>) -> Self {
        Self {
            status: 200,
            headers: BTreeMap::new(),
            body: body.as_ref().to_vec(),
        }
    }

    /// Create a JSON response with the given body.
    #[must_use]
    pub fn json(body: impl AsRef<[u8]>) -> Self {
        let mut headers = BTreeMap::new();
        headers.insert("content-type".to_string(), "application/json".to_string());
        Self {
            status: 200,
            headers,
            body: body.as_ref().to_vec(),
        }
    }

    /// Set a header on the response.
    #[must_use]
    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }

    /// Set the response body.
    #[must_use]
    pub fn with_body(mut self, body: impl AsRef<[u8]>) -> Self {
        self.body = body.as_ref().to_vec();
        self
    }

    /// Set the status code.
    #[must_use]
    pub fn with_status(mut self, status: u16) -> Self {
        self.status = status;
        self
    }
}

impl MockHost {
    /// Create a new mock host with empty state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: BTreeMap::new(),
            kv: BTreeMap::new(),
            secrets: BTreeMap::new(),
            logs: Vec::new(),
            http_responses: BTreeMap::new(),
        }
    }

    /// Set a mock config value.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let mut host = MockHost::new();
    /// host.set_config("timeout", "30");
    /// ```
    pub fn set_config(&mut self, key: &str, value: &str) -> &mut Self {
        self.config.insert(key.to_string(), value.to_string());
        self
    }

    /// Set multiple mock config values at once.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let mut host = MockHost::new();
    /// host.set_configs(&[
    ///     ("timeout", "30"),
    ///     ("retries", "3"),
    ///     ("debug", "true"),
    /// ]);
    /// ```
    pub fn set_configs(&mut self, configs: &[(&str, &str)]) -> &mut Self {
        for (key, value) in configs {
            self.config.insert((*key).to_string(), (*value).to_string());
        }
        self
    }

    /// Get a config value.
    #[must_use]
    pub fn get_config(&self, key: &str) -> Option<&String> {
        self.config.get(key)
    }

    /// Set a mock KV value.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let mut host = MockHost::new();
    /// host.set_kv("cache", "data:1", &[1, 2, 3, 4]);
    /// ```
    pub fn set_kv(&mut self, bucket: &str, key: &str, value: &[u8]) -> &mut Self {
        self.kv
            .insert((bucket.to_string(), key.to_string()), value.to_vec());
        self
    }

    /// Set a mock KV string value (convenience method).
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let mut host = MockHost::new();
    /// host.set_kv_string("cache", "user:1", "{\"name\": \"Alice\"}");
    /// ```
    pub fn set_kv_string(&mut self, bucket: &str, key: &str, value: &str) -> &mut Self {
        self.set_kv(bucket, key, value.as_bytes())
    }

    /// Get a KV value.
    #[must_use]
    pub fn get_kv(&self, bucket: &str, key: &str) -> Option<&Vec<u8>> {
        self.kv.get(&(bucket.to_string(), key.to_string()))
    }

    /// Get a KV value as a string.
    #[must_use]
    pub fn get_kv_string(&self, bucket: &str, key: &str) -> Option<String> {
        self.get_kv(bucket, key)
            .and_then(|v| core::str::from_utf8(v).ok())
            .map(ToString::to_string)
    }

    /// Delete a KV value.
    pub fn delete_kv(&mut self, bucket: &str, key: &str) -> &mut Self {
        self.kv.remove(&(bucket.to_string(), key.to_string()));
        self
    }

    /// List all keys in a KV bucket.
    #[must_use]
    pub fn list_kv_keys(&self, bucket: &str) -> Vec<String> {
        self.kv
            .keys()
            .filter(|(b, _)| b == bucket)
            .map(|(_, k)| k.clone())
            .collect()
    }

    /// Set a mock secret.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let mut host = MockHost::new();
    /// host.set_secret("api_key", "super-secret-key");
    /// ```
    pub fn set_secret(&mut self, name: &str, value: &str) -> &mut Self {
        self.secrets.insert(name.to_string(), value.to_string());
        self
    }

    /// Set multiple mock secrets at once.
    pub fn set_secrets(&mut self, secrets: &[(&str, &str)]) -> &mut Self {
        for (name, value) in secrets {
            self.secrets
                .insert((*name).to_string(), (*value).to_string());
        }
        self
    }

    /// Get a secret value.
    #[must_use]
    pub fn get_secret(&self, name: &str) -> Option<&String> {
        self.secrets.get(name)
    }

    /// Add a mock HTTP response for a URL.
    ///
    /// When the plugin makes an HTTP request to this URL, the mock response
    /// will be returned instead of making a real request.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let mut host = MockHost::new();
    /// host.set_http_response(
    ///     "https://api.example.com/users",
    ///     MockHttpResponse::json(r#"{"users": []}"#),
    /// );
    /// ```
    pub fn set_http_response(&mut self, url: &str, response: MockHttpResponse) -> &mut Self {
        self.http_responses.insert(url.to_string(), response);
        self
    }

    /// Get a mock HTTP response for a URL.
    #[must_use]
    pub fn get_http_response(&self, url: &str) -> Option<&MockHttpResponse> {
        self.http_responses.get(url)
    }

    /// Record a log message (called by mock log functions).
    ///
    /// Returns `&mut Self` for method chaining.
    pub fn log(&mut self, level: LogLevel, message: &str) -> &mut Self {
        self.logs.push((level, message.to_string()));
        self
    }

    /// Get all captured logs.
    #[must_use]
    pub fn logs(&self) -> &[(LogLevel, String)] {
        &self.logs
    }

    /// Get logs filtered by level.
    #[must_use]
    pub fn logs_at_level(&self, level: LogLevel) -> Vec<&String> {
        self.logs
            .iter()
            .filter(|(l, _)| *l == level)
            .map(|(_, msg)| msg)
            .collect()
    }

    /// Clear all captured logs.
    pub fn clear_logs(&mut self) {
        self.logs.clear();
    }

    /// Check if a log message was captured at the given level containing the specified text.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let host = MockHost::new();
    /// // ... run plugin logic that logs ...
    /// assert!(host.has_log(LogLevel::Info, "request processed"));
    /// assert!(host.has_log(LogLevel::Error, "connection failed"));
    /// ```
    #[must_use]
    pub fn has_log(&self, level: LogLevel, contains: &str) -> bool {
        self.logs
            .iter()
            .any(|(l, msg)| *l == level && msg.contains(contains))
    }

    /// Check if any log message contains the specified text (at any level).
    #[must_use]
    pub fn has_log_containing(&self, contains: &str) -> bool {
        self.logs.iter().any(|(_, msg)| msg.contains(contains))
    }

    /// Get the count of log messages at a given level.
    #[must_use]
    pub fn log_count(&self, level: LogLevel) -> usize {
        self.logs.iter().filter(|(l, _)| *l == level).count()
    }

    /// Get the total count of all log messages.
    #[must_use]
    pub fn total_log_count(&self) -> usize {
        self.logs.len()
    }

    /// Assert that a log message exists (panics with helpful message if not found).
    ///
    /// # Panics
    ///
    /// Panics if no log message at the given level contains the specified text.
    pub fn assert_log(&self, level: LogLevel, contains: &str) {
        assert!(
            self.has_log(level, contains),
            "Expected log at level {:?} containing '{}', but found: {:?}",
            level,
            contains,
            self.logs
        );
    }

    /// Assert that no log message at the given level contains the specified text.
    ///
    /// # Panics
    ///
    /// Panics if a log message at the given level contains the specified text.
    pub fn assert_no_log(&self, level: LogLevel, contains: &str) {
        assert!(
            !self.has_log(level, contains),
            "Expected no log at level {level:?} containing '{contains}', but found one",
        );
    }

    /// Assert that no error logs were captured.
    ///
    /// # Panics
    ///
    /// Panics if any error logs exist.
    pub fn assert_no_errors(&self) {
        let errors: Vec<_> = self.logs_at_level(LogLevel::Error);
        assert!(
            errors.is_empty(),
            "Expected no error logs, but found: {errors:?}",
        );
    }

    /// Reset all state (config, KV, secrets, logs, HTTP responses).
    pub fn reset(&mut self) {
        self.config.clear();
        self.kv.clear();
        self.secrets.clear();
        self.logs.clear();
        self.http_responses.clear();
    }
}

impl Default for MockHost {
    fn default() -> Self {
        Self::new()
    }
}

/// Macro for running a test with a mock host.
///
/// This macro creates a test function that sets up a `MockHost`,
/// runs a setup closure to configure it, and then runs a test closure.
///
/// # Example
///
/// ```rust,ignore
/// use zlayer_sdk::zlayer_test;
/// use zlayer_sdk::testing::LogLevel;
///
/// zlayer_test!(test_config_loading, |host| {
///     host.set_config("api_endpoint", "https://api.example.com")
///         .set_secret("api_key", "test-key-123");
/// }, |host| {
///     // Your test assertions here
///     assert!(host.get_config("api_endpoint").is_some());
/// });
/// ```
#[macro_export]
macro_rules! zlayer_test {
    ($name:ident, $setup:expr, $test:expr) => {
        #[test]
        fn $name() {
            let mut host = $crate::testing::MockHost::new();
            $setup(&mut host);
            $test(&host);
        }
    };
}

/// Macro for running an async test with a mock host.
///
/// Similar to `zlayer_test!` but for async test functions.
/// Requires a compatible async test runtime (like tokio).
///
/// # Example
///
/// ```rust,ignore
/// use zlayer_sdk::zlayer_async_test;
/// use zlayer_sdk::testing::LogLevel;
///
/// zlayer_async_test!(test_async_operation, |host| {
///     host.set_config("timeout", "30");
/// }, |host| async {
///     // Your async test assertions here
///     assert!(host.get_config("timeout").is_some());
/// });
/// ```
#[macro_export]
macro_rules! zlayer_async_test {
    ($name:ident, $setup:expr, $test:expr) => {
        #[tokio::test]
        async fn $name() {
            let mut host = $crate::testing::MockHost::new();
            $setup(&mut host);
            $test(&host).await;
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_host_config() {
        let mut host = MockHost::new();
        host.set_config("key1", "value1")
            .set_config("key2", "value2");

        assert_eq!(host.get_config("key1"), Some(&"value1".to_string()));
        assert_eq!(host.get_config("key2"), Some(&"value2".to_string()));
        assert_eq!(host.get_config("key3"), None);
    }

    #[test]
    fn test_mock_host_configs_batch() {
        let mut host = MockHost::new();
        host.set_configs(&[("a", "1"), ("b", "2"), ("c", "3")]);

        assert_eq!(host.get_config("a"), Some(&"1".to_string()));
        assert_eq!(host.get_config("b"), Some(&"2".to_string()));
        assert_eq!(host.get_config("c"), Some(&"3".to_string()));
    }

    #[test]
    fn test_mock_host_kv() {
        let mut host = MockHost::new();
        host.set_kv("bucket1", "key1", &[1, 2, 3])
            .set_kv_string("bucket1", "key2", "hello");

        assert_eq!(host.get_kv("bucket1", "key1"), Some(&vec![1, 2, 3]));
        assert_eq!(
            host.get_kv_string("bucket1", "key2"),
            Some("hello".to_string())
        );
        assert_eq!(host.get_kv("bucket1", "key3"), None);
    }

    #[test]
    fn test_mock_host_kv_list_keys() {
        let mut host = MockHost::new();
        host.set_kv_string("cache", "user:1", "alice")
            .set_kv_string("cache", "user:2", "bob")
            .set_kv_string("other", "data", "value");

        let keys = host.list_kv_keys("cache");
        assert_eq!(keys.len(), 2);
        assert!(keys.contains(&"user:1".to_string()));
        assert!(keys.contains(&"user:2".to_string()));
    }

    #[test]
    fn test_mock_host_kv_delete() {
        let mut host = MockHost::new();
        host.set_kv_string("bucket", "key", "value");
        assert!(host.get_kv("bucket", "key").is_some());

        host.delete_kv("bucket", "key");
        assert!(host.get_kv("bucket", "key").is_none());
    }

    #[test]
    fn test_mock_host_secrets() {
        let mut host = MockHost::new();
        host.set_secret("api_key", "secret123")
            .set_secrets(&[("db_pass", "pass456"), ("token", "tok789")]);

        assert_eq!(host.get_secret("api_key"), Some(&"secret123".to_string()));
        assert_eq!(host.get_secret("db_pass"), Some(&"pass456".to_string()));
        assert_eq!(host.get_secret("token"), Some(&"tok789".to_string()));
        assert_eq!(host.get_secret("missing"), None);
    }

    #[test]
    fn test_mock_host_logging() {
        let mut host = MockHost::new();
        host.log(LogLevel::Info, "Application started");
        host.log(LogLevel::Warn, "Connection slow");
        host.log(LogLevel::Error, "Database connection failed");

        assert_eq!(host.logs().len(), 3);
        assert!(host.has_log(LogLevel::Info, "started"));
        assert!(host.has_log(LogLevel::Warn, "slow"));
        assert!(host.has_log(LogLevel::Error, "failed"));
        assert!(!host.has_log(LogLevel::Debug, "anything"));
    }

    #[test]
    fn test_mock_host_log_filtering() {
        let mut host = MockHost::new();
        host.log(LogLevel::Info, "info1");
        host.log(LogLevel::Info, "info2");
        host.log(LogLevel::Error, "error1");

        assert_eq!(host.log_count(LogLevel::Info), 2);
        assert_eq!(host.log_count(LogLevel::Error), 1);
        assert_eq!(host.log_count(LogLevel::Debug), 0);
        assert_eq!(host.total_log_count(), 3);

        let info_logs = host.logs_at_level(LogLevel::Info);
        assert_eq!(info_logs.len(), 2);
    }

    #[test]
    fn test_mock_host_clear_logs() {
        let mut host = MockHost::new();
        host.log(LogLevel::Info, "message");
        assert_eq!(host.logs().len(), 1);

        host.clear_logs();
        assert_eq!(host.logs().len(), 0);
    }

    #[test]
    fn test_mock_host_has_log_containing() {
        let mut host = MockHost::new();
        host.log(LogLevel::Info, "Processing request ID=123");

        assert!(host.has_log_containing("ID=123"));
        assert!(host.has_log_containing("Processing"));
        assert!(!host.has_log_containing("Error"));
    }

    #[test]
    fn test_mock_host_assert_log() {
        let mut host = MockHost::new();
        host.log(LogLevel::Info, "Operation completed successfully");

        host.assert_log(LogLevel::Info, "completed");
        host.assert_no_log(LogLevel::Error, "failed");
    }

    #[test]
    fn test_mock_host_assert_no_errors() {
        let mut host = MockHost::new();
        host.log(LogLevel::Info, "All good");
        host.log(LogLevel::Warn, "Minor issue");

        host.assert_no_errors();
    }

    #[test]
    #[should_panic(expected = "Expected no error logs")]
    fn test_mock_host_assert_no_errors_fails() {
        let mut host = MockHost::new();
        host.log(LogLevel::Error, "Something went wrong");

        host.assert_no_errors();
    }

    #[test]
    fn test_mock_host_http_response() {
        let mut host = MockHost::new();
        host.set_http_response(
            "https://api.example.com/data",
            MockHttpResponse::json(r#"{"result": "ok"}"#),
        );

        let response = host
            .get_http_response("https://api.example.com/data")
            .unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(
            response.headers.get("content-type"),
            Some(&"application/json".to_string())
        );
    }

    #[test]
    fn test_mock_http_response_builder() {
        let response = MockHttpResponse::new(404)
            .with_header("x-custom", "value")
            .with_body(b"Not Found");

        assert_eq!(response.status, 404);
        assert_eq!(response.headers.get("x-custom"), Some(&"value".to_string()));
        assert_eq!(response.body, b"Not Found");
    }

    #[test]
    fn test_mock_host_reset() {
        let mut host = MockHost::new();
        host.set_config("key", "value")
            .set_kv_string("bucket", "key", "data")
            .set_secret("secret", "shhh")
            .log(LogLevel::Info, "message")
            .set_http_response("https://example.com", MockHttpResponse::ok(b""));

        host.reset();

        assert!(host.get_config("key").is_none());
        assert!(host.get_kv("bucket", "key").is_none());
        assert!(host.get_secret("secret").is_none());
        assert!(host.logs().is_empty());
        assert!(host.get_http_response("https://example.com").is_none());
    }

    #[test]
    fn test_log_level_as_str() {
        assert_eq!(LogLevel::Trace.as_str(), "TRACE");
        assert_eq!(LogLevel::Debug.as_str(), "DEBUG");
        assert_eq!(LogLevel::Info.as_str(), "INFO");
        assert_eq!(LogLevel::Warn.as_str(), "WARN");
        assert_eq!(LogLevel::Error.as_str(), "ERROR");
    }

    #[test]
    fn test_mock_host_default() {
        let host = MockHost::default();
        assert!(host.logs().is_empty());
    }
}
