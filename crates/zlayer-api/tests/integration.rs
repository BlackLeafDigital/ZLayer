//! API integration tests

use axum::{
    body::Body,
    http::{header, Request, StatusCode},
};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::RwLock;
use tower::ServiceExt;

use zlayer_agent::{MockRuntime, ServiceManager};
use zlayer_api::storage::InMemoryStorage;
use zlayer_api::{
    build_router, build_router_with_internal, create_token, ApiConfig, InternalState,
    INTERNAL_AUTH_HEADER,
};

fn test_config() -> ApiConfig {
    ApiConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        jwt_secret: "test-secret-for-integration-tests".to_string(),
        swagger_enabled: true,
        ..Default::default()
    }
}

fn create_test_token(config: &ApiConfig) -> String {
    create_token(
        &config.jwt_secret,
        "test-user",
        std::time::Duration::from_secs(3600),
        vec!["admin".to_string()],
    )
    .unwrap()
}

#[tokio::test]
async fn test_health_liveness() {
    let config = test_config();
    let app = build_router(&config);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health/live")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["status"], "ok");
}

#[tokio::test]
async fn test_health_readiness() {
    let config = test_config();
    let app = build_router(&config);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_auth_token_success() {
    let config = test_config();
    let app = build_router(&config);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_string(&json!({
                        "api_key": "dev",
                        "api_secret": "dev-secret"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["token_type"], "Bearer");
    assert!(json["access_token"].as_str().is_some());
}

#[tokio::test]
async fn test_auth_token_invalid_credentials() {
    let config = test_config();
    let app = build_router(&config);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_string(&json!({
                        "api_key": "wrong",
                        "api_secret": "wrong"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_deployments_requires_auth() {
    let config = test_config();
    let app = build_router(&config);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/deployments")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_deployments_with_auth() {
    let config = test_config();
    let token = create_test_token(&config);
    let app = build_router(&config);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/deployments")
                .header(header::AUTHORIZATION, format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert!(json.as_array().is_some());
}

#[tokio::test]
async fn test_deployment_not_found() {
    let config = test_config();
    let token = create_test_token(&config);
    let app = build_router(&config);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/deployments/nonexistent")
                .header(header::AUTHORIZATION, format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_swagger_ui_available() {
    let config = test_config();
    let app = build_router(&config);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/swagger-ui/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Swagger UI redirects or returns 200
    assert!(
        response.status() == StatusCode::OK
            || response.status() == StatusCode::MOVED_PERMANENTLY
            || response.status() == StatusCode::TEMPORARY_REDIRECT
    );
}

#[tokio::test]
async fn test_openapi_json_available() {
    let config = test_config();
    let app = build_router(&config);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api-docs/openapi.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["info"]["title"], "ZLayer API");
}

#[tokio::test]
async fn test_swagger_disabled() {
    let config = ApiConfig {
        swagger_enabled: false,
        ..test_config()
    };
    let app = build_router(&config);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/swagger-ui/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Should return 404 when swagger is disabled
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_invalid_bearer_token() {
    let config = test_config();
    let app = build_router(&config);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/deployments")
                .header(header::AUTHORIZATION, "Bearer invalid-token-here")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_malformed_auth_header() {
    let config = test_config();
    let app = build_router(&config);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/deployments")
                .header(header::AUTHORIZATION, "NotBearer token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

// Internal API tests

fn create_app_with_internal() -> (axum::Router, String) {
    let config = test_config();
    let storage: Arc<dyn zlayer_api::DeploymentStorage + Send + Sync> =
        Arc::new(InMemoryStorage::new());
    let runtime: Arc<dyn zlayer_agent::Runtime + Send + Sync> = Arc::new(MockRuntime::new());
    let service_manager = Arc::new(RwLock::new(ServiceManager::new(runtime)));
    let internal_token = "test-internal-secret".to_string();

    let app = build_router_with_internal(&config, storage, service_manager, internal_token.clone());
    (app, internal_token)
}

#[tokio::test]
async fn test_internal_scale_requires_token() {
    let (app, _token) = create_app_with_internal();

    // Request without internal token should fail
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/internal/scale")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_string(&json!({
                        "service": "web",
                        "replicas": 3
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_internal_scale_invalid_token() {
    let (app, _token) = create_app_with_internal();

    // Request with wrong internal token should fail
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/internal/scale")
                .header(header::CONTENT_TYPE, "application/json")
                .header(INTERNAL_AUTH_HEADER, "wrong-token")
                .body(Body::from(
                    serde_json::to_string(&json!({
                        "service": "web",
                        "replicas": 3
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_internal_scale_service_not_found() {
    let (app, token) = create_app_with_internal();

    // Request for non-existent service should return 404
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/internal/scale")
                .header(header::CONTENT_TYPE, "application/json")
                .header(INTERNAL_AUTH_HEADER, token)
                .body(Body::from(
                    serde_json::to_string(&json!({
                        "service": "nonexistent",
                        "replicas": 3
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_internal_get_replicas_requires_token() {
    let (app, _token) = create_app_with_internal();

    // Request without internal token should fail
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/internal/replicas/web")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_internal_get_replicas_not_found() {
    let (app, token) = create_app_with_internal();

    // Request for non-existent service should return 404
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/internal/replicas/nonexistent")
                .header(INTERNAL_AUTH_HEADER, token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
