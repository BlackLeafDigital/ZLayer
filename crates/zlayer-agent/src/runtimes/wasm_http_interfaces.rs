//! Host-side Rust bindings for ZLayer custom HTTP interfaces
//!
//! This module provides Rust types and helper functions for interacting with
//! WASM components that export the custom ZLayer HTTP interfaces defined in
//! `wit/zlayer/http.wit`:
//!
//! - **Router**: Custom request routing logic
//! - **Middleware**: Request/response interception
//! - **WebSocket**: WebSocket handling with message interception
//! - **Caching**: HTTP response caching policies
//!
//! ## Design
//!
//! These bindings provide:
//! 1. Rust types that mirror the WIT interface definitions
//! 2. Trait definitions for type-safe component interaction
//! 3. Helper functions for calling component exports
//! 4. Conversion utilities between Rust and WASM types
//!
//! ## Usage
//!
//! ```ignore
//! use zlayer_agent::runtimes::wasm_http_interfaces::*;
//!
//! // Example: Using router decision types
//! let decision = RoutingDecision::Forward(Upstream {
//!     host: "backend.example.com".to_string(),
//!     port: 8080,
//!     tls: true,
//!     connect_timeout_ns: 5_000_000_000, // 5 seconds
//!     request_timeout_ns: 30_000_000_000, // 30 seconds
//! });
//! ```

use std::time::Duration;

// =============================================================================
// Common Types (from types.wit - common interface)
// =============================================================================

/// A key-value pair for headers, metadata, environment, etc.
///
/// Maps to WIT: `record key-value { key: string, value: string }`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyValue {
    pub key: String,
    pub value: String,
}

impl KeyValue {
    /// Create a new key-value pair
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
        }
    }
}

impl From<(String, String)> for KeyValue {
    fn from((key, value): (String, String)) -> Self {
        Self { key, value }
    }
}

impl From<(&str, &str)> for KeyValue {
    fn from((key, value): (&str, &str)) -> Self {
        Self {
            key: key.to_string(),
            value: value.to_string(),
        }
    }
}

impl From<KeyValue> for (String, String) {
    fn from(kv: KeyValue) -> Self {
        (kv.key, kv.value)
    }
}

/// Timestamp in nanoseconds since Unix epoch
///
/// Maps to WIT: `type timestamp = u64`
pub type Timestamp = u64;

/// Duration in nanoseconds
///
/// Maps to WIT: `type duration = u64`
pub type DurationNs = u64;

/// Convert std::time::Duration to nanoseconds
pub fn duration_to_ns(d: Duration) -> DurationNs {
    d.as_nanos() as u64
}

/// Convert nanoseconds to std::time::Duration
pub fn ns_to_duration(ns: DurationNs) -> Duration {
    Duration::from_nanos(ns)
}

// =============================================================================
// HTTP Types (from http.wit - http-types interface)
// =============================================================================

/// HTTP version enum
///
/// Maps to WIT:
/// ```wit
/// enum http-version {
///     http-1-0,
///     http-1-1,
///     http-2,
///     http-3,
/// }
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum HttpVersion {
    Http10 = 0,
    #[default]
    Http11 = 1,
    Http2 = 2,
    Http3 = 3,
}


impl std::fmt::Display for HttpVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpVersion::Http10 => write!(f, "HTTP/1.0"),
            HttpVersion::Http11 => write!(f, "HTTP/1.1"),
            HttpVersion::Http2 => write!(f, "HTTP/2"),
            HttpVersion::Http3 => write!(f, "HTTP/3"),
        }
    }
}

/// Request metadata from the proxy layer
///
/// Maps to WIT:
/// ```wit
/// record request-metadata {
///     client-ip: string,
///     client-port: u16,
///     tls-version: option<string>,
///     tls-cipher: option<string>,
///     server-name: option<string>,
///     http-version: http-version,
///     received-at: timestamp,
/// }
/// ```
#[derive(Debug, Clone)]
pub struct RequestMetadata {
    /// Original client IP address
    pub client_ip: String,
    /// Client port
    pub client_port: u16,
    /// TLS version if HTTPS (e.g., "TLSv1.3")
    pub tls_version: Option<String>,
    /// TLS cipher suite if HTTPS
    pub tls_cipher: Option<String>,
    /// Server name from SNI
    pub server_name: Option<String>,
    /// HTTP version used
    pub http_version: HttpVersion,
    /// Request received timestamp (nanoseconds since epoch)
    pub received_at: Timestamp,
}

impl Default for RequestMetadata {
    fn default() -> Self {
        Self {
            client_ip: "127.0.0.1".to_string(),
            client_port: 0,
            tls_version: None,
            tls_cipher: None,
            server_name: None,
            http_version: HttpVersion::Http11,
            received_at: 0,
        }
    }
}

impl RequestMetadata {
    /// Create metadata for a local request (testing/internal)
    pub fn local() -> Self {
        Self::default()
    }

    /// Create metadata with client connection info
    pub fn with_client(ip: impl Into<String>, port: u16) -> Self {
        Self {
            client_ip: ip.into(),
            client_port: port,
            ..Default::default()
        }
    }

    /// Set TLS information
    pub fn with_tls(mut self, version: impl Into<String>, cipher: impl Into<String>) -> Self {
        self.tls_version = Some(version.into());
        self.tls_cipher = Some(cipher.into());
        self
    }

    /// Set server name (from SNI)
    pub fn with_server_name(mut self, name: impl Into<String>) -> Self {
        self.server_name = Some(name.into());
        self
    }

    /// Set HTTP version
    pub fn with_http_version(mut self, version: HttpVersion) -> Self {
        self.http_version = version;
        self
    }

    /// Set timestamp
    pub fn with_timestamp(mut self, ts: Timestamp) -> Self {
        self.received_at = ts;
        self
    }
}

/// Upstream backend information
///
/// Maps to WIT:
/// ```wit
/// record upstream {
///     host: string,
///     port: u16,
///     tls: bool,
///     connect-timeout: duration,
///     request-timeout: duration,
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upstream {
    /// Backend host
    pub host: String,
    /// Backend port
    pub port: u16,
    /// Use TLS for upstream connection
    pub tls: bool,
    /// Connection timeout in nanoseconds
    pub connect_timeout_ns: DurationNs,
    /// Request timeout in nanoseconds
    pub request_timeout_ns: DurationNs,
}

impl Upstream {
    /// Create a new upstream with default timeouts (5s connect, 30s request)
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            tls: false,
            connect_timeout_ns: duration_to_ns(Duration::from_secs(5)),
            request_timeout_ns: duration_to_ns(Duration::from_secs(30)),
        }
    }

    /// Create an HTTPS upstream
    pub fn https(host: impl Into<String>, port: u16) -> Self {
        Self {
            tls: true,
            ..Self::new(host, port)
        }
    }

    /// Set connection timeout
    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout_ns = duration_to_ns(timeout);
        self
    }

    /// Set request timeout
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout_ns = duration_to_ns(timeout);
        self
    }

    /// Get connection timeout as Duration
    pub fn connect_timeout(&self) -> Duration {
        ns_to_duration(self.connect_timeout_ns)
    }

    /// Get request timeout as Duration
    pub fn request_timeout(&self) -> Duration {
        ns_to_duration(self.request_timeout_ns)
    }

    /// Get the upstream URL
    pub fn url(&self) -> String {
        let scheme = if self.tls { "https" } else { "http" };
        format!("{}://{}:{}", scheme, self.host, self.port)
    }
}

/// Redirect information
///
/// Maps to WIT:
/// ```wit
/// record redirect-info {
///     location: string,
///     status: u16,
///     preserve-body: bool,
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedirectInfo {
    /// Redirect location URL
    pub location: String,
    /// HTTP status code (301, 302, 307, 308)
    pub status: u16,
    /// Preserve request body on redirect (307/308 only)
    pub preserve_body: bool,
}

impl RedirectInfo {
    /// Create a 301 Moved Permanently redirect
    pub fn permanent(location: impl Into<String>) -> Self {
        Self {
            location: location.into(),
            status: 301,
            preserve_body: false,
        }
    }

    /// Create a 302 Found redirect
    pub fn temporary(location: impl Into<String>) -> Self {
        Self {
            location: location.into(),
            status: 302,
            preserve_body: false,
        }
    }

    /// Create a 307 Temporary Redirect (preserves method and body)
    pub fn temporary_with_body(location: impl Into<String>) -> Self {
        Self {
            location: location.into(),
            status: 307,
            preserve_body: true,
        }
    }

    /// Create a 308 Permanent Redirect (preserves method and body)
    pub fn permanent_with_body(location: impl Into<String>) -> Self {
        Self {
            location: location.into(),
            status: 308,
            preserve_body: true,
        }
    }
}

/// Immediate response without forwarding
///
/// Maps to WIT:
/// ```wit
/// record immediate-response {
///     status: u16,
///     headers: list<key-value>,
///     body: list<u8>,
/// }
/// ```
#[derive(Debug, Clone)]
pub struct ImmediateResponse {
    /// HTTP status code
    pub status: u16,
    /// Response headers
    pub headers: Vec<KeyValue>,
    /// Response body
    pub body: Vec<u8>,
}

impl ImmediateResponse {
    /// Create a new immediate response
    pub fn new(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    /// Create a 200 OK response
    pub fn ok() -> Self {
        Self::new(200)
    }

    /// Create a 404 Not Found response
    pub fn not_found() -> Self {
        Self::new(404)
    }

    /// Create a 403 Forbidden response
    pub fn forbidden() -> Self {
        Self::new(403)
    }

    /// Create a 500 Internal Server Error response
    pub fn internal_error() -> Self {
        Self::new(500)
    }

    /// Add a header
    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push(KeyValue::new(key, value));
        self
    }

    /// Set the body
    pub fn with_body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = body.into();
        self
    }

    /// Set a JSON body
    pub fn with_json_body(mut self, body: impl AsRef<[u8]>) -> Self {
        self.headers
            .push(KeyValue::new("Content-Type", "application/json"));
        self.body = body.as_ref().to_vec();
        self
    }

    /// Set a text body
    pub fn with_text_body(mut self, body: impl Into<String>) -> Self {
        self.headers
            .push(KeyValue::new("Content-Type", "text/plain"));
        self.body = body.into().into_bytes();
        self
    }
}

/// Routing decision for requests
///
/// Maps to WIT:
/// ```wit
/// variant routing-decision {
///     forward(upstream),
///     redirect(redirect-info),
///     respond-immediate(immediate-response),
///     continue-processing,
/// }
/// ```
#[derive(Debug, Clone)]
pub enum RoutingDecision {
    /// Forward to the specified upstream
    Forward(Upstream),
    /// Redirect the client
    Redirect(RedirectInfo),
    /// Return a response directly
    RespondImmediate(ImmediateResponse),
    /// Continue to next handler
    ContinueProcessing,
}

impl RoutingDecision {
    /// Create a forward decision to an HTTP upstream
    pub fn forward_http(host: impl Into<String>, port: u16) -> Self {
        Self::Forward(Upstream::new(host, port))
    }

    /// Create a forward decision to an HTTPS upstream
    pub fn forward_https(host: impl Into<String>, port: u16) -> Self {
        Self::Forward(Upstream::https(host, port))
    }

    /// Create a permanent redirect
    pub fn redirect_permanent(location: impl Into<String>) -> Self {
        Self::Redirect(RedirectInfo::permanent(location))
    }

    /// Create a temporary redirect
    pub fn redirect_temporary(location: impl Into<String>) -> Self {
        Self::Redirect(RedirectInfo::temporary(location))
    }

    /// Create an immediate response
    pub fn respond(status: u16) -> Self {
        Self::RespondImmediate(ImmediateResponse::new(status))
    }

    /// Continue processing (pass to next handler)
    pub fn continue_processing() -> Self {
        Self::ContinueProcessing
    }
}

// =============================================================================
// Middleware Types (from http.wit - middleware interface)
// =============================================================================

/// Middleware action returned from request/response hooks
///
/// Maps to WIT:
/// ```wit
/// variant middleware-action {
///     continue-with(list<key-value>),
///     abort(u16, string),
/// }
/// ```
#[derive(Debug, Clone)]
pub enum MiddlewareAction {
    /// Continue processing with modified/additional headers
    ContinueWith(Vec<KeyValue>),
    /// Abort with error response
    Abort {
        /// HTTP status code
        status: u16,
        /// Error reason/message
        reason: String,
    },
}

impl MiddlewareAction {
    /// Continue without modifications
    pub fn continue_unchanged() -> Self {
        Self::ContinueWith(Vec::new())
    }

    /// Continue with additional headers
    pub fn continue_with_headers(headers: Vec<KeyValue>) -> Self {
        Self::ContinueWith(headers)
    }

    /// Continue with a single additional header
    pub fn continue_with_header(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self::ContinueWith(vec![KeyValue::new(key, value)])
    }

    /// Abort with 400 Bad Request
    pub fn bad_request(reason: impl Into<String>) -> Self {
        Self::Abort {
            status: 400,
            reason: reason.into(),
        }
    }

    /// Abort with 401 Unauthorized
    pub fn unauthorized(reason: impl Into<String>) -> Self {
        Self::Abort {
            status: 401,
            reason: reason.into(),
        }
    }

    /// Abort with 403 Forbidden
    pub fn forbidden(reason: impl Into<String>) -> Self {
        Self::Abort {
            status: 403,
            reason: reason.into(),
        }
    }

    /// Abort with 429 Too Many Requests
    pub fn rate_limited(reason: impl Into<String>) -> Self {
        Self::Abort {
            status: 429,
            reason: reason.into(),
        }
    }

    /// Abort with 500 Internal Server Error
    pub fn internal_error(reason: impl Into<String>) -> Self {
        Self::Abort {
            status: 500,
            reason: reason.into(),
        }
    }

    /// Abort with custom status and reason
    pub fn abort(status: u16, reason: impl Into<String>) -> Self {
        Self::Abort {
            status,
            reason: reason.into(),
        }
    }

    /// Check if this action allows continuation
    pub fn is_continue(&self) -> bool {
        matches!(self, Self::ContinueWith(_))
    }

    /// Check if this action is an abort
    pub fn is_abort(&self) -> bool {
        matches!(self, Self::Abort { .. })
    }
}

// =============================================================================
// WebSocket Types (from http.wit - websocket interface)
// =============================================================================

/// WebSocket message type
///
/// Maps to WIT:
/// ```wit
/// enum message-type {
///     text,
///     binary,
///     ping,
///     pong,
///     close,
/// }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum MessageType {
    Text = 0,
    Binary = 1,
    Ping = 2,
    Pong = 3,
    Close = 4,
}

impl std::fmt::Display for MessageType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MessageType::Text => write!(f, "text"),
            MessageType::Binary => write!(f, "binary"),
            MessageType::Ping => write!(f, "ping"),
            MessageType::Pong => write!(f, "pong"),
            MessageType::Close => write!(f, "close"),
        }
    }
}

/// WebSocket message
///
/// Maps to WIT:
/// ```wit
/// record message {
///     msg-type: message-type,
///     data: list<u8>,
/// }
/// ```
#[derive(Debug, Clone)]
pub struct WebSocketMessage {
    /// Message type
    pub msg_type: MessageType,
    /// Message data
    pub data: Vec<u8>,
}

impl WebSocketMessage {
    /// Create a text message
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            msg_type: MessageType::Text,
            data: content.into().into_bytes(),
        }
    }

    /// Create a binary message
    pub fn binary(data: impl Into<Vec<u8>>) -> Self {
        Self {
            msg_type: MessageType::Binary,
            data: data.into(),
        }
    }

    /// Create a ping message
    pub fn ping(data: impl Into<Vec<u8>>) -> Self {
        Self {
            msg_type: MessageType::Ping,
            data: data.into(),
        }
    }

    /// Create a pong message
    pub fn pong(data: impl Into<Vec<u8>>) -> Self {
        Self {
            msg_type: MessageType::Pong,
            data: data.into(),
        }
    }

    /// Create a close message
    pub fn close() -> Self {
        Self {
            msg_type: MessageType::Close,
            data: Vec::new(),
        }
    }

    /// Create a close message with a reason code
    pub fn close_with_code(code: u16) -> Self {
        Self {
            msg_type: MessageType::Close,
            data: code.to_be_bytes().to_vec(),
        }
    }

    /// Get text content if this is a text message
    pub fn as_text(&self) -> Option<&str> {
        if self.msg_type == MessageType::Text {
            std::str::from_utf8(&self.data).ok()
        } else {
            None
        }
    }

    /// Check if this is a control frame (ping, pong, close)
    pub fn is_control(&self) -> bool {
        matches!(
            self.msg_type,
            MessageType::Ping | MessageType::Pong | MessageType::Close
        )
    }
}

/// WebSocket upgrade decision
///
/// Maps to WIT:
/// ```wit
/// variant upgrade-decision {
///     accept,
///     accept-with-headers(list<key-value>),
///     reject(u16, string),
/// }
/// ```
#[derive(Debug, Clone)]
pub enum UpgradeDecision {
    /// Accept the upgrade
    Accept,
    /// Accept with modified headers
    AcceptWithHeaders(Vec<KeyValue>),
    /// Reject the upgrade with status and reason
    Reject {
        /// HTTP status code
        status: u16,
        /// Rejection reason
        reason: String,
    },
}

impl UpgradeDecision {
    /// Accept the WebSocket upgrade
    pub fn accept() -> Self {
        Self::Accept
    }

    /// Accept with custom response headers
    pub fn accept_with_headers(headers: Vec<KeyValue>) -> Self {
        Self::AcceptWithHeaders(headers)
    }

    /// Accept with a subprotocol
    pub fn accept_subprotocol(protocol: impl Into<String>) -> Self {
        Self::AcceptWithHeaders(vec![KeyValue::new("Sec-WebSocket-Protocol", protocol)])
    }

    /// Reject with 403 Forbidden
    pub fn reject_forbidden(reason: impl Into<String>) -> Self {
        Self::Reject {
            status: 403,
            reason: reason.into(),
        }
    }

    /// Reject with 401 Unauthorized
    pub fn reject_unauthorized(reason: impl Into<String>) -> Self {
        Self::Reject {
            status: 401,
            reason: reason.into(),
        }
    }

    /// Reject with custom status and reason
    pub fn reject(status: u16, reason: impl Into<String>) -> Self {
        Self::Reject {
            status,
            reason: reason.into(),
        }
    }

    /// Check if this decision accepts the upgrade
    pub fn is_accepted(&self) -> bool {
        matches!(self, Self::Accept | Self::AcceptWithHeaders(_))
    }
}

// =============================================================================
// Caching Types (from http.wit - caching interface)
// =============================================================================

/// Cache entry with metadata
///
/// Maps to WIT:
/// ```wit
/// record cache-entry {
///     ttl: duration,
///     tags: list<string>,
///     vary: list<string>,
///     stale-while-revalidate: option<duration>,
/// }
/// ```
#[derive(Debug, Clone)]
pub struct CacheEntry {
    /// Time to live in nanoseconds
    pub ttl_ns: DurationNs,
    /// Tags for cache invalidation
    pub tags: Vec<String>,
    /// Vary headers - cache separately for different values
    pub vary: Vec<String>,
    /// Allow serving stale content while revalidating
    pub stale_while_revalidate_ns: Option<DurationNs>,
}

impl CacheEntry {
    /// Create a new cache entry with the specified TTL
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl_ns: duration_to_ns(ttl),
            tags: Vec::new(),
            vary: Vec::new(),
            stale_while_revalidate_ns: None,
        }
    }

    /// Set TTL in seconds (convenience)
    pub fn ttl_secs(seconds: u64) -> Self {
        Self::new(Duration::from_secs(seconds))
    }

    /// Add a cache invalidation tag
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Add multiple cache invalidation tags
    pub fn with_tags(mut self, tags: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.tags.extend(tags.into_iter().map(Into::into));
        self
    }

    /// Add a vary header
    pub fn vary_on(mut self, header: impl Into<String>) -> Self {
        self.vary.push(header.into());
        self
    }

    /// Set stale-while-revalidate window
    pub fn with_stale_while_revalidate(mut self, duration: Duration) -> Self {
        self.stale_while_revalidate_ns = Some(duration_to_ns(duration));
        self
    }

    /// Get TTL as Duration
    pub fn ttl(&self) -> Duration {
        ns_to_duration(self.ttl_ns)
    }

    /// Get stale-while-revalidate as Duration
    pub fn stale_while_revalidate(&self) -> Option<Duration> {
        self.stale_while_revalidate_ns.map(ns_to_duration)
    }
}

/// Cache control decision
///
/// Maps to WIT:
/// ```wit
/// variant cache-decision {
///     no-cache,
///     cache-for(duration),
///     cache-with-tags(cache-entry),
/// }
/// ```
#[derive(Debug, Clone)]
pub enum CacheDecision {
    /// Don't cache this response
    NoCache,
    /// Cache with specified TTL (nanoseconds)
    CacheFor(DurationNs),
    /// Cache with full metadata
    CacheWithTags(CacheEntry),
}

impl CacheDecision {
    /// Don't cache this response
    pub fn no_cache() -> Self {
        Self::NoCache
    }

    /// Cache for the specified duration
    pub fn cache_for(duration: Duration) -> Self {
        Self::CacheFor(duration_to_ns(duration))
    }

    /// Cache for N seconds
    pub fn cache_for_secs(seconds: u64) -> Self {
        Self::cache_for(Duration::from_secs(seconds))
    }

    /// Cache with full entry metadata
    pub fn cache_with_entry(entry: CacheEntry) -> Self {
        Self::CacheWithTags(entry)
    }

    /// Check if caching is enabled
    pub fn is_cacheable(&self) -> bool {
        !matches!(self, Self::NoCache)
    }

    /// Get the TTL if cacheable
    pub fn ttl(&self) -> Option<Duration> {
        match self {
            Self::NoCache => None,
            Self::CacheFor(ns) => Some(ns_to_duration(*ns)),
            Self::CacheWithTags(entry) => Some(entry.ttl()),
        }
    }
}

// =============================================================================
// Request Types (from types.wit - request-types interface)
// =============================================================================

/// HTTP method enum
///
/// Maps to WIT:
/// ```wit
/// enum http-method {
///     get, post, put, delete, patch, head, options, connect, trace,
/// }
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum HttpMethod {
    #[default]
    Get = 0,
    Post = 1,
    Put = 2,
    Delete = 3,
    Patch = 4,
    Head = 5,
    Options = 6,
    Connect = 7,
    Trace = 8,
}


impl std::fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpMethod::Get => write!(f, "GET"),
            HttpMethod::Post => write!(f, "POST"),
            HttpMethod::Put => write!(f, "PUT"),
            HttpMethod::Delete => write!(f, "DELETE"),
            HttpMethod::Patch => write!(f, "PATCH"),
            HttpMethod::Head => write!(f, "HEAD"),
            HttpMethod::Options => write!(f, "OPTIONS"),
            HttpMethod::Connect => write!(f, "CONNECT"),
            HttpMethod::Trace => write!(f, "TRACE"),
        }
    }
}

impl std::str::FromStr for HttpMethod {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "GET" => Ok(HttpMethod::Get),
            "POST" => Ok(HttpMethod::Post),
            "PUT" => Ok(HttpMethod::Put),
            "DELETE" => Ok(HttpMethod::Delete),
            "PATCH" => Ok(HttpMethod::Patch),
            "HEAD" => Ok(HttpMethod::Head),
            "OPTIONS" => Ok(HttpMethod::Options),
            "CONNECT" => Ok(HttpMethod::Connect),
            "TRACE" => Ok(HttpMethod::Trace),
            _ => Err(format!("Unknown HTTP method: {}", s)),
        }
    }
}

/// Incoming request to be processed by a plugin
///
/// Maps to WIT:
/// ```wit
/// record plugin-request {
///     request-id: string,
///     path: string,
///     method: http-method,
///     query: option<string>,
///     headers: list<key-value>,
///     body: list<u8>,
///     timestamp: timestamp,
///     context: list<key-value>,
/// }
/// ```
#[derive(Debug, Clone)]
pub struct PluginRequest {
    /// Unique request identifier for tracing
    pub request_id: String,
    /// Request path (e.g., "/api/users/123")
    pub path: String,
    /// HTTP method
    pub method: HttpMethod,
    /// Query string (without leading ?)
    pub query: Option<String>,
    /// Request headers
    pub headers: Vec<KeyValue>,
    /// Request body as bytes
    pub body: Vec<u8>,
    /// Request timestamp (nanoseconds since epoch)
    pub timestamp: Timestamp,
    /// Additional context from the host
    pub context: Vec<KeyValue>,
}

impl PluginRequest {
    /// Create a new request
    pub fn new(method: HttpMethod, path: impl Into<String>) -> Self {
        Self {
            request_id: uuid::Uuid::new_v4().to_string(),
            path: path.into(),
            method,
            query: None,
            headers: Vec::new(),
            body: Vec::new(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64,
            context: Vec::new(),
        }
    }

    /// Create a GET request
    pub fn get(path: impl Into<String>) -> Self {
        Self::new(HttpMethod::Get, path)
    }

    /// Create a POST request
    pub fn post(path: impl Into<String>) -> Self {
        Self::new(HttpMethod::Post, path)
    }

    /// Add a query string
    pub fn with_query(mut self, query: impl Into<String>) -> Self {
        self.query = Some(query.into());
        self
    }

    /// Add a header
    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push(KeyValue::new(key, value));
        self
    }

    /// Set the body
    pub fn with_body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = body.into();
        self
    }

    /// Add context
    pub fn with_context(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.context.push(KeyValue::new(key, value));
        self
    }

    /// Get a header value by name (case-insensitive)
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|kv| kv.key.eq_ignore_ascii_case(name))
            .map(|kv| kv.value.as_str())
    }

    /// Get a context value by key
    pub fn context_value(&self, key: &str) -> Option<&str> {
        self.context
            .iter()
            .find(|kv| kv.key == key)
            .map(|kv| kv.value.as_str())
    }

    /// Get the full URI (path + query)
    pub fn uri(&self) -> String {
        match &self.query {
            Some(q) if !q.is_empty() => format!("{}?{}", self.path, q),
            _ => self.path.clone(),
        }
    }
}

// =============================================================================
// Plugin Traits
// =============================================================================

/// Trait for components that export the router interface
///
/// Components implementing this trait can make custom routing decisions
/// for incoming HTTP requests.
pub trait RouterPlugin {
    /// Determine routing for an incoming request
    fn route(&mut self, request: &PluginRequest, metadata: &RequestMetadata) -> RoutingDecision;
}

/// Trait for components that export the middleware interface
///
/// Components implementing this trait can intercept and modify
/// requests and responses.
pub trait MiddlewarePlugin {
    /// Called before request is forwarded
    fn on_request(
        &mut self,
        method: &str,
        path: &str,
        headers: &[KeyValue],
        metadata: &RequestMetadata,
    ) -> MiddlewareAction;

    /// Called after response is received from upstream
    fn on_response(&mut self, status: u16, headers: &[KeyValue]) -> MiddlewareAction;
}

/// Trait for components that export the websocket interface
///
/// Components implementing this trait can handle WebSocket connections
/// with message interception capabilities.
pub trait WebSocketPlugin {
    /// Called when a WebSocket upgrade is requested
    fn on_upgrade(&mut self, path: &str, headers: &[KeyValue]) -> UpgradeDecision;

    /// Called when a message is received from client
    fn on_client_message(&mut self, message: &WebSocketMessage) -> Option<WebSocketMessage>;

    /// Called when a message is received from upstream
    fn on_upstream_message(&mut self, message: &WebSocketMessage) -> Option<WebSocketMessage>;
}

/// Trait for components that export the caching interface
///
/// Components implementing this trait can provide custom caching policies
/// for HTTP responses.
pub trait CachingPlugin {
    /// Determine caching policy for a response
    fn cache_policy(
        &mut self,
        method: &str,
        path: &str,
        status: u16,
        headers: &[KeyValue],
    ) -> CacheDecision;

    /// Generate cache key for a request
    fn cache_key(&mut self, method: &str, path: &str, headers: &[KeyValue]) -> String;
}

// =============================================================================
// Error Types
// =============================================================================

/// Errors that can occur when calling WASM HTTP interface functions
#[derive(Debug, Clone, thiserror::Error)]
pub enum WasmInterfaceError {
    /// Failed to get the exported function
    #[error("function '{function}' not exported by component")]
    FunctionNotFound { function: String },

    /// Failed to call the function
    #[error("failed to call '{function}': {reason}")]
    CallFailed { function: String, reason: String },

    /// Failed to convert return value
    #[error("failed to convert return value from '{function}': {reason}")]
    ConversionFailed { function: String, reason: String },

    /// Component does not implement the expected interface
    #[error("component does not implement interface '{interface}'")]
    InterfaceNotImplemented { interface: String },
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // KeyValue Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_key_value_new() {
        let kv = KeyValue::new("Content-Type", "application/json");
        assert_eq!(kv.key, "Content-Type");
        assert_eq!(kv.value, "application/json");
    }

    #[test]
    fn test_key_value_from_tuple() {
        let kv: KeyValue = ("Host".to_string(), "example.com".to_string()).into();
        assert_eq!(kv.key, "Host");
        assert_eq!(kv.value, "example.com");
    }

    #[test]
    fn test_key_value_to_tuple() {
        let kv = KeyValue::new("X-Custom", "value");
        let (k, v): (String, String) = kv.into();
        assert_eq!(k, "X-Custom");
        assert_eq!(v, "value");
    }

    // -------------------------------------------------------------------------
    // Duration Conversion Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_duration_conversion_roundtrip() {
        let original = Duration::from_secs(30);
        let ns = duration_to_ns(original);
        let converted = ns_to_duration(ns);
        assert_eq!(original, converted);
    }

    #[test]
    fn test_duration_conversion_millis() {
        let original = Duration::from_millis(500);
        let ns = duration_to_ns(original);
        assert_eq!(ns, 500_000_000);
        let converted = ns_to_duration(ns);
        assert_eq!(original, converted);
    }

    // -------------------------------------------------------------------------
    // HttpVersion Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_http_version_display() {
        assert_eq!(HttpVersion::Http10.to_string(), "HTTP/1.0");
        assert_eq!(HttpVersion::Http11.to_string(), "HTTP/1.1");
        assert_eq!(HttpVersion::Http2.to_string(), "HTTP/2");
        assert_eq!(HttpVersion::Http3.to_string(), "HTTP/3");
    }

    #[test]
    fn test_http_version_default() {
        assert_eq!(HttpVersion::default(), HttpVersion::Http11);
    }

    // -------------------------------------------------------------------------
    // RequestMetadata Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_request_metadata_local() {
        let meta = RequestMetadata::local();
        assert_eq!(meta.client_ip, "127.0.0.1");
        assert_eq!(meta.client_port, 0);
        assert!(meta.tls_version.is_none());
    }

    #[test]
    fn test_request_metadata_builder() {
        let meta = RequestMetadata::with_client("192.168.1.100", 54321)
            .with_tls("TLSv1.3", "TLS_AES_256_GCM_SHA384")
            .with_server_name("example.com")
            .with_http_version(HttpVersion::Http2);

        assert_eq!(meta.client_ip, "192.168.1.100");
        assert_eq!(meta.client_port, 54321);
        assert_eq!(meta.tls_version.as_deref(), Some("TLSv1.3"));
        assert_eq!(meta.tls_cipher.as_deref(), Some("TLS_AES_256_GCM_SHA384"));
        assert_eq!(meta.server_name.as_deref(), Some("example.com"));
        assert_eq!(meta.http_version, HttpVersion::Http2);
    }

    // -------------------------------------------------------------------------
    // Upstream Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_upstream_http() {
        let upstream = Upstream::new("backend.local", 8080);
        assert_eq!(upstream.host, "backend.local");
        assert_eq!(upstream.port, 8080);
        assert!(!upstream.tls);
        assert_eq!(upstream.url(), "http://backend.local:8080");
    }

    #[test]
    fn test_upstream_https() {
        let upstream = Upstream::https("api.example.com", 443);
        assert!(upstream.tls);
        assert_eq!(upstream.url(), "https://api.example.com:443");
    }

    #[test]
    fn test_upstream_timeouts() {
        let upstream = Upstream::new("backend", 80)
            .with_connect_timeout(Duration::from_secs(10))
            .with_request_timeout(Duration::from_secs(60));

        assert_eq!(upstream.connect_timeout(), Duration::from_secs(10));
        assert_eq!(upstream.request_timeout(), Duration::from_secs(60));
    }

    // -------------------------------------------------------------------------
    // RedirectInfo Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_redirect_permanent() {
        let redirect = RedirectInfo::permanent("https://example.com");
        assert_eq!(redirect.status, 301);
        assert!(!redirect.preserve_body);
    }

    #[test]
    fn test_redirect_temporary_with_body() {
        let redirect = RedirectInfo::temporary_with_body("https://example.com/new");
        assert_eq!(redirect.status, 307);
        assert!(redirect.preserve_body);
    }

    // -------------------------------------------------------------------------
    // ImmediateResponse Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_immediate_response_builder() {
        let resp = ImmediateResponse::ok()
            .with_header("X-Custom", "value")
            .with_json_body(r#"{"status":"ok"}"#);

        assert_eq!(resp.status, 200);
        assert_eq!(resp.headers.len(), 2);
        assert!(!resp.body.is_empty());
    }

    // -------------------------------------------------------------------------
    // RoutingDecision Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_routing_decision_forward() {
        let decision = RoutingDecision::forward_https("api.backend.com", 443);
        match decision {
            RoutingDecision::Forward(upstream) => {
                assert!(upstream.tls);
                assert_eq!(upstream.host, "api.backend.com");
            }
            _ => panic!("Expected Forward"),
        }
    }

    #[test]
    fn test_routing_decision_continue() {
        let decision = RoutingDecision::continue_processing();
        assert!(matches!(decision, RoutingDecision::ContinueProcessing));
    }

    // -------------------------------------------------------------------------
    // MiddlewareAction Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_middleware_action_continue() {
        let action = MiddlewareAction::continue_unchanged();
        assert!(action.is_continue());
        assert!(!action.is_abort());
    }

    #[test]
    fn test_middleware_action_abort() {
        let action = MiddlewareAction::forbidden("Access denied");
        assert!(action.is_abort());
        match action {
            MiddlewareAction::Abort { status, reason } => {
                assert_eq!(status, 403);
                assert_eq!(reason, "Access denied");
            }
            _ => panic!("Expected Abort"),
        }
    }

    // -------------------------------------------------------------------------
    // WebSocket Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_websocket_message_text() {
        let msg = WebSocketMessage::text("Hello, World!");
        assert_eq!(msg.msg_type, MessageType::Text);
        assert_eq!(msg.as_text(), Some("Hello, World!"));
        assert!(!msg.is_control());
    }

    #[test]
    fn test_websocket_message_control() {
        let ping = WebSocketMessage::ping(vec![1, 2, 3]);
        assert!(ping.is_control());
        assert_eq!(ping.msg_type, MessageType::Ping);
    }

    #[test]
    fn test_upgrade_decision_accept() {
        let decision = UpgradeDecision::accept();
        assert!(decision.is_accepted());
    }

    #[test]
    fn test_upgrade_decision_subprotocol() {
        let decision = UpgradeDecision::accept_subprotocol("graphql-ws");
        match decision {
            UpgradeDecision::AcceptWithHeaders(headers) => {
                assert_eq!(headers.len(), 1);
                assert_eq!(headers[0].key, "Sec-WebSocket-Protocol");
                assert_eq!(headers[0].value, "graphql-ws");
            }
            _ => panic!("Expected AcceptWithHeaders"),
        }
    }

    // -------------------------------------------------------------------------
    // Caching Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_cache_entry_builder() {
        let entry = CacheEntry::ttl_secs(300)
            .with_tag("products")
            .with_tag("catalog")
            .vary_on("Accept-Language")
            .with_stale_while_revalidate(Duration::from_secs(60));

        assert_eq!(entry.ttl(), Duration::from_secs(300));
        assert_eq!(entry.tags, vec!["products", "catalog"]);
        assert_eq!(entry.vary, vec!["Accept-Language"]);
        assert_eq!(
            entry.stale_while_revalidate(),
            Some(Duration::from_secs(60))
        );
    }

    #[test]
    fn test_cache_decision_no_cache() {
        let decision = CacheDecision::no_cache();
        assert!(!decision.is_cacheable());
        assert!(decision.ttl().is_none());
    }

    #[test]
    fn test_cache_decision_cache_for() {
        let decision = CacheDecision::cache_for_secs(3600);
        assert!(decision.is_cacheable());
        assert_eq!(decision.ttl(), Some(Duration::from_secs(3600)));
    }

    // -------------------------------------------------------------------------
    // HttpMethod Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_http_method_display() {
        assert_eq!(HttpMethod::Get.to_string(), "GET");
        assert_eq!(HttpMethod::Post.to_string(), "POST");
        assert_eq!(HttpMethod::Delete.to_string(), "DELETE");
    }

    #[test]
    fn test_http_method_from_str() {
        assert_eq!("GET".parse::<HttpMethod>().unwrap(), HttpMethod::Get);
        assert_eq!("post".parse::<HttpMethod>().unwrap(), HttpMethod::Post);
        assert!("INVALID".parse::<HttpMethod>().is_err());
    }

    // -------------------------------------------------------------------------
    // PluginRequest Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_plugin_request_builder() {
        let req = PluginRequest::post("/api/users")
            .with_query("page=1&limit=10")
            .with_header("Content-Type", "application/json")
            .with_body(r#"{"name":"test"}"#.as_bytes())
            .with_context("user_id", "123");

        assert_eq!(req.method, HttpMethod::Post);
        assert_eq!(req.path, "/api/users");
        assert_eq!(req.query.as_deref(), Some("page=1&limit=10"));
        assert_eq!(req.header("content-type"), Some("application/json"));
        assert_eq!(req.context_value("user_id"), Some("123"));
        assert_eq!(req.uri(), "/api/users?page=1&limit=10");
    }

    #[test]
    fn test_plugin_request_uri_without_query() {
        let req = PluginRequest::get("/api/health");
        assert_eq!(req.uri(), "/api/health");
    }

    // -------------------------------------------------------------------------
    // Error Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_wasm_interface_error_display() {
        let err = WasmInterfaceError::FunctionNotFound {
            function: "route".to_string(),
        };
        assert!(err.to_string().contains("route"));
        assert!(err.to_string().contains("not exported"));
    }
}
