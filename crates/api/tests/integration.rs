//! API integration tests

use axum::{
    body::Body,
    http::{header, Request, StatusCode},
};
use serde_json::{json, Value};
use tower::ServiceExt;

use api::{build_router, create_token, ApiConfig};

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
