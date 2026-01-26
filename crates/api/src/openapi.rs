//! OpenAPI documentation generation

use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};

// Import types for schema definitions
use crate::handlers::auth::{TokenRequest, TokenResponse};
use crate::handlers::build::{
    BuildRequest, BuildRequestWithContext, BuildStateEnum, BuildStatus, TemplateInfo,
    TriggerBuildResponse,
};
use crate::handlers::deployments::{CreateDeploymentRequest, DeploymentDetails, DeploymentSummary};
use crate::handlers::health::HealthResponse;
use crate::handlers::services::{
    ScaleRequest, ServiceDetails, ServiceEndpoint, ServiceMetrics, ServiceSummary,
};

// Import the auto-generated path types from utoipa macros
use crate::handlers::auth::__path_get_token;
use crate::handlers::build::{
    __path_get_build_logs, __path_get_build_status, __path_list_builds,
    __path_list_runtime_templates, __path_start_build, __path_start_build_json,
    __path_stream_build,
};
use crate::handlers::deployments::{
    __path_create_deployment, __path_delete_deployment, __path_get_deployment,
    __path_list_deployments,
};
use crate::handlers::health::{__path_liveness, __path_readiness};
use crate::handlers::services::{
    __path_get_service, __path_get_service_logs, __path_list_services, __path_scale_service,
};

/// Security addon for adding Bearer JWT authentication
struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        if let Some(components) = openapi.components.as_mut() {
            components.add_security_scheme(
                "bearer_auth",
                SecurityScheme::Http(
                    HttpBuilder::new()
                        .scheme(HttpAuthScheme::Bearer)
                        .bearer_format("JWT")
                        .build(),
                ),
            );
        }
    }
}

/// ZLayer API OpenAPI documentation
#[derive(OpenApi)]
#[openapi(
    info(
        title = "ZLayer API",
        description = "Container orchestration API for ZLayer",
        version = "0.1.0",
        license(name = "MIT OR Apache-2.0"),
        contact(
            name = "ZLayer",
            url = "https://github.com/zachhandley/ZLayer"
        )
    ),
    paths(
        // Health
        liveness,
        readiness,
        // Auth
        get_token,
        // Deployments
        list_deployments,
        get_deployment,
        create_deployment,
        delete_deployment,
        // Services
        list_services,
        get_service,
        scale_service,
        get_service_logs,
        // Build
        start_build,
        start_build_json,
        get_build_status,
        stream_build,
        get_build_logs,
        list_builds,
        list_runtime_templates,
    ),
    components(
        schemas(
            HealthResponse,
            TokenRequest,
            TokenResponse,
            DeploymentSummary,
            DeploymentDetails,
            CreateDeploymentRequest,
            ServiceSummary,
            ServiceDetails,
            ServiceEndpoint,
            ServiceMetrics,
            ScaleRequest,
            // Build schemas
            BuildRequest,
            BuildRequestWithContext,
            BuildStatus,
            BuildStateEnum,
            TemplateInfo,
            TriggerBuildResponse,
        )
    ),
    modifiers(&SecurityAddon),
    tags(
        (name = "Health", description = "Health check endpoints"),
        (name = "Authentication", description = "Authentication endpoints"),
        (name = "Deployments", description = "Deployment management"),
        (name = "Services", description = "Service management"),
        (name = "Build", description = "Container image building"),
    )
)]
pub struct ApiDoc;

#[cfg(test)]
mod tests {
    use super::*;
    use utoipa::OpenApi;

    #[test]
    fn test_openapi_generation() {
        let doc = ApiDoc::openapi();
        assert_eq!(doc.info.title, "ZLayer API");
        assert_eq!(doc.info.version, "0.1.0");
    }

    #[test]
    fn test_openapi_has_paths() {
        let doc = ApiDoc::openapi();
        assert!(!doc.paths.paths.is_empty());
    }

    #[test]
    fn test_openapi_has_security_schemes() {
        let doc = ApiDoc::openapi();
        let components = doc.components.as_ref().expect("should have components");
        assert!(components.security_schemes.contains_key("bearer_auth"));
    }

    #[test]
    fn test_openapi_json_serialization() {
        let doc = ApiDoc::openapi();
        let json = doc.to_json().expect("should serialize to JSON");
        assert!(json.contains("ZLayer API"));
        assert!(json.contains("bearer_auth"));
    }
}
