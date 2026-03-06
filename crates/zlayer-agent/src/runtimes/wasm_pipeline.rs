//! WASM Pipeline Runtime
//!
//! Handles proxy-adjacent WASM worlds that are invoked during request processing:
//! - Transformer: request/response data transformation
//! - Authenticator: authentication/authorization decisions
//! - Rate Limiter: rate limiting decisions
//! - Middleware: request/response interception
//! - Router: custom routing decisions
//!
//! These components are invoked by the proxy layer, not by external HTTP requests directly.

use zlayer_spec::ServiceType;

use super::WasmPipelineConfig;

/// Pipeline runtime that manages WASM components for proxy-adjacent worlds.
///
/// Unlike `WasmHttpRuntime` which handles full HTTP request/response cycles,
/// `WasmPipelineRuntime` manages components that participate in the proxy's
/// request processing pipeline (auth -> rate-limit -> middleware -> route -> handle).
pub struct WasmPipelineRuntime {
    config: WasmPipelineConfig,
}

impl WasmPipelineRuntime {
    /// Create a new pipeline runtime for the given configuration
    pub fn new(
        config: WasmPipelineConfig,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        tracing::info!(
            service_type = ?config.service_type,
            "created WASM pipeline runtime"
        );
        Ok(Self { config })
    }

    /// Get the service type this runtime handles
    pub fn service_type(&self) -> ServiceType {
        self.config.service_type
    }

    /// Get the pipeline stage name for this service type
    pub fn pipeline_stage(&self) -> &'static str {
        match self.config.service_type {
            ServiceType::WasmTransformer => "transform",
            ServiceType::WasmAuthenticator => "authenticate",
            ServiceType::WasmRateLimiter => "rate_limit",
            ServiceType::WasmMiddleware => "middleware",
            ServiceType::WasmRouter => "route",
            _ => "unknown",
        }
    }

    /// Get the configuration
    pub fn config(&self) -> &WasmPipelineConfig {
        &self.config
    }
}
