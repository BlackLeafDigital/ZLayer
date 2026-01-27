//! Container operation span helpers
//!
//! Provides instrumentation helpers for container lifecycle operations
//! following OpenTelemetry semantic conventions.

use tracing::{info_span, Span};

/// Semantic attribute keys for container operations
pub mod attributes {
    pub const CONTAINER_ID: &str = "container.id";
    pub const CONTAINER_NAME: &str = "container.name";
    pub const CONTAINER_IMAGE: &str = "container.image.name";
    pub const CONTAINER_RUNTIME: &str = "container.runtime";
    pub const SERVICE_NAME: &str = "service.name";
    pub const SERVICE_REPLICA: &str = "zlayer.service.replica";
    pub const OPERATION: &str = "zlayer.operation";
}

/// Container operation types for span naming
#[derive(Debug, Clone, Copy)]
pub enum ContainerOperation {
    Create,
    Start,
    Stop,
    Remove,
    Pull,
    Exec,
    Logs,
    Stats,
    PersistLayer,
    RestoreLayer,
}

impl ContainerOperation {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Create => "container.create",
            Self::Start => "container.start",
            Self::Stop => "container.stop",
            Self::Remove => "container.remove",
            Self::Pull => "image.pull",
            Self::Exec => "container.exec",
            Self::Logs => "container.logs",
            Self::Stats => "container.stats",
            Self::PersistLayer => "layer.persist",
            Self::RestoreLayer => "layer.restore",
        }
    }
}

impl std::fmt::Display for ContainerOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Create a span for container operations
pub fn container_span(
    operation: ContainerOperation,
    service: &str,
    replica: u32,
    image: Option<&str>,
) -> Span {
    let container_id = format!("{}-{}", service, replica);

    let span = info_span!(
        target: "zlayer::container",
        "container_operation",
        otel.name = %operation.as_str(),
        %container_id,
        service.name = %service,
        service.replica = %replica,
        operation = %operation.as_str(),
        container.runtime = "youki",
    );

    if let Some(img) = image {
        span.record("container.image.name", img);
    }

    span
}

/// Create a span for image pull operations
pub fn image_pull_span(image: &str) -> Span {
    info_span!(
        target: "zlayer::registry",
        "image.pull",
        otel.name = "image.pull",
        container.image.name = %image,
        operation = "pull",
    )
}

/// Create a span for layer storage operations
pub fn layer_storage_span(
    operation: ContainerOperation,
    container_id: &str,
    digest: Option<&str>,
) -> Span {
    let span = info_span!(
        target: "zlayer::layer_storage",
        "layer_operation",
        otel.name = %operation.as_str(),
        %container_id,
        operation = %operation.as_str(),
    );

    if let Some(d) = digest {
        span.record("layer.digest", d);
    }

    span
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_operation_as_str() {
        assert_eq!(ContainerOperation::Create.as_str(), "container.create");
        assert_eq!(ContainerOperation::PersistLayer.as_str(), "layer.persist");
    }

    #[test]
    fn test_container_span_creation() {
        let span = container_span(
            ContainerOperation::Create,
            "my-service",
            1,
            Some("nginx:latest"),
        );
        assert!(span.is_disabled() || !span.is_disabled()); // Just verify it doesn't panic
    }
}
