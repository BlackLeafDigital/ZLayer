//! OpenTelemetry distributed tracing
//!
//! Provides trace context propagation and OTLP export for distributed systems.

use opentelemetry::{global, trace::TracerProvider as _, KeyValue};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    trace::{RandomIdGenerator, Sampler, SdkTracerProvider},
    Resource,
};
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::Registry;

use crate::config::TracingConfig;
use crate::error::{ObservabilityError, Result};

/// Guard that shuts down the tracer provider when dropped
pub struct TracingGuard {
    _provider: Option<SdkTracerProvider>,
}

impl Drop for TracingGuard {
    fn drop(&mut self) {
        if let Some(ref provider) = self._provider {
            // Use the provider's shutdown method directly (0.31+ API)
            if let Err(e) = provider.shutdown() {
                tracing::warn!("Error shutting down tracer provider: {:?}", e);
            }
        }
    }
}

/// Initialize OpenTelemetry tracing
///
/// Returns a guard that must be held for the lifetime of the application.
/// When dropped, it will flush pending traces and shut down the provider.
///
/// Note: This function does NOT set a global tracing subscriber. It only
/// initializes the OpenTelemetry provider. Use `create_otel_layer()` to get
/// a layer that can be combined with other tracing-subscriber layers.
pub fn init_tracing(config: &TracingConfig) -> Result<TracingGuard> {
    if !config.enabled {
        tracing::info!("OpenTelemetry tracing disabled");
        return Ok(TracingGuard { _provider: None });
    }

    let endpoint = config.otlp_endpoint.as_ref().ok_or_else(|| {
        ObservabilityError::TracingInit(
            "OTLP endpoint required when tracing is enabled".to_string(),
        )
    })?;

    // Build the OTLP exporter (0.31+ API: with_tonic() first, then configure)
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()
        .map_err(|e| ObservabilityError::TracingInit(e.to_string()))?;

    // Build the sampler
    let sampler = if config.sampling_ratio >= 1.0 {
        Sampler::AlwaysOn
    } else if config.sampling_ratio <= 0.0 {
        Sampler::AlwaysOff
    } else {
        Sampler::TraceIdRatioBased(config.sampling_ratio)
    };

    // Build the tracer provider with batch exporter (0.31+ API)
    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_sampler(sampler)
        .with_id_generator(RandomIdGenerator::default())
        .with_resource(
            Resource::builder_empty()
                .with_service_name(config.service_name.clone())
                .with_attribute(KeyValue::new("service.version", env!("CARGO_PKG_VERSION")))
                .build(),
        )
        .build();

    // Set the global tracer provider
    global::set_tracer_provider(provider.clone());

    tracing::info!(
        endpoint = %endpoint,
        service_name = %config.service_name,
        sampling_ratio = config.sampling_ratio,
        "OpenTelemetry tracing initialized"
    );

    Ok(TracingGuard {
        _provider: Some(provider),
    })
}

/// Create an OpenTelemetry layer that can be combined with other layers
///
/// Use this when you want to integrate with an existing tracing-subscriber setup.
/// The returned layer can be combined with other layers using `Registry::with()`.
///
/// Returns `None` if tracing is disabled in the configuration.
pub fn create_otel_layer(
    config: &TracingConfig,
) -> Result<Option<OpenTelemetryLayer<Registry, opentelemetry_sdk::trace::Tracer>>> {
    if !config.enabled {
        return Ok(None);
    }

    let endpoint = config.otlp_endpoint.as_ref().ok_or_else(|| {
        ObservabilityError::TracingInit(
            "OTLP endpoint required when tracing is enabled".to_string(),
        )
    })?;

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()
        .map_err(|e| ObservabilityError::TracingInit(e.to_string()))?;

    let sampler = if config.sampling_ratio >= 1.0 {
        Sampler::AlwaysOn
    } else if config.sampling_ratio <= 0.0 {
        Sampler::AlwaysOff
    } else {
        Sampler::TraceIdRatioBased(config.sampling_ratio)
    };

    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_sampler(sampler)
        .with_id_generator(RandomIdGenerator::default())
        .with_resource(
            Resource::builder_empty()
                .with_service_name(config.service_name.clone())
                .build(),
        )
        .build();

    let tracer = provider.tracer("zlayer");
    global::set_tracer_provider(provider);

    Ok(Some(OpenTelemetryLayer::new(tracer)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_disabled_tracing() {
        let config = TracingConfig {
            enabled: false,
            ..Default::default()
        };

        let guard = init_tracing(&config).unwrap();
        assert!(guard._provider.is_none());
    }

    #[test]
    fn test_enabled_without_endpoint_fails() {
        let config = TracingConfig {
            enabled: true,
            otlp_endpoint: None,
            ..Default::default()
        };

        let result = init_tracing(&config);
        assert!(result.is_err());
    }

    #[test]
    fn test_create_layer_disabled() {
        let config = TracingConfig {
            enabled: false,
            ..Default::default()
        };

        let layer = create_otel_layer(&config).unwrap();
        assert!(layer.is_none());
    }

    #[test]
    fn test_create_layer_without_endpoint_fails() {
        let config = TracingConfig {
            enabled: true,
            otlp_endpoint: None,
            ..Default::default()
        };

        let result = create_otel_layer(&config);
        assert!(result.is_err());
    }
}
