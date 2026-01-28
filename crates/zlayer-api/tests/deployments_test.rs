//! Integration tests for the Deployments API
//!
//! These tests cover the full deployment lifecycle:
//! - Create, List, Get, Delete operations
//! - Authentication requirements
//! - Error handling for invalid inputs and non-existent resources

use axum::{
    body::Body,
    http::{header, Request, StatusCode},
};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

use zlayer_api::{
    build_router_with_storage, create_token, storage::InMemoryStorage, ApiConfig,
    DeploymentStorage, StoredDeployment,
};

// =============================================================================
// Test Helpers
// =============================================================================

/// Create a test configuration
fn test_config() -> ApiConfig {
    ApiConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        jwt_secret: "test-secret-for-deployments-tests".to_string(),
        swagger_enabled: false,
        ..Default::default()
    }
}

/// Create a valid JWT token for testing
fn create_test_token(config: &ApiConfig) -> String {
    create_token(
        &config.jwt_secret,
        "test-user",
        std::time::Duration::from_secs(3600),
        vec!["admin".to_string()],
    )
    .unwrap()
}

/// Create a valid deployment YAML spec
fn valid_deployment_spec(name: &str) -> String {
    format!(
        r#"version: v1
deployment: {}
services:
  api:
    rtype: service
    image:
      name: nginx:latest
    endpoints:
      - name: http
        protocol: http
        port: 8080
"#,
        name
    )
}

/// Create an invalid deployment YAML spec (missing required fields)
fn invalid_deployment_spec() -> String {
    // Invalid: version v2 is not supported
    r#"version: v2
deployment: invalid-app
services:
  api:
    image:
      name: nginx:latest
"#
    .to_string()
}

/// Create a deployment spec with invalid YAML syntax
fn malformed_yaml_spec() -> String {
    r#"version: v1
deployment: test
services:
  api:
    image:
      name: [invalid yaml here
"#
    .to_string()
}

/// Helper to extract JSON body from response
async fn extract_body(response: axum::response::Response) -> Value {
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap_or_else(|_| {
        // If body isn't JSON, return as string value
        Value::String(String::from_utf8_lossy(&body).to_string())
    })
}

// =============================================================================
// Create Deployment Tests
// =============================================================================

#[tokio::test]
async fn test_create_deployment_success() {
    let config = test_config();
    let token = create_test_token(&config);
    let storage = Arc::new(InMemoryStorage::new());
    let app = build_router_with_storage(&config, storage);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/deployments")
                .header(header::AUTHORIZATION, format!("Bearer {}", token))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_string(&json!({
                        "spec": valid_deployment_spec("my-test-app")
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::CREATED,
        "Expected 201 Created"
    );

    let body = extract_body(response).await;
    assert_eq!(body["name"], "my-test-app");
    assert_eq!(body["status"], "pending");
    assert!(body["services"].as_array().is_some());
    assert!(body["created_at"].as_str().is_some());
    assert!(body["updated_at"].as_str().is_some());
}

#[tokio::test]
async fn test_create_deployment_invalid_spec_returns_400() {
    let config = test_config();
    let token = create_test_token(&config);
    let storage = Arc::new(InMemoryStorage::new());
    let app = build_router_with_storage(&config, storage);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/deployments")
                .header(header::AUTHORIZATION, format!("Bearer {}", token))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_string(&json!({
                        "spec": invalid_deployment_spec()
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "Expected 400 Bad Request for invalid spec"
    );
}

#[tokio::test]
async fn test_create_deployment_malformed_yaml_returns_400() {
    let config = test_config();
    let token = create_test_token(&config);
    let storage = Arc::new(InMemoryStorage::new());
    let app = build_router_with_storage(&config, storage);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/deployments")
                .header(header::AUTHORIZATION, format!("Bearer {}", token))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_string(&json!({
                        "spec": malformed_yaml_spec()
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "Expected 400 Bad Request for malformed YAML"
    );
}

#[tokio::test]
async fn test_create_deployment_missing_spec_returns_400() {
    let config = test_config();
    let token = create_test_token(&config);
    let storage = Arc::new(InMemoryStorage::new());
    let app = build_router_with_storage(&config, storage);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/deployments")
                .header(header::AUTHORIZATION, format!("Bearer {}", token))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_string(&json!({})).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "Expected 422 for missing required field"
    );
}

#[tokio::test]
async fn test_create_deployment_requires_auth() {
    let config = test_config();
    let storage = Arc::new(InMemoryStorage::new());
    let app = build_router_with_storage(&config, storage);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/deployments")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_string(&json!({
                        "spec": valid_deployment_spec("test-app")
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "Expected 401 Unauthorized without token"
    );
}

// =============================================================================
// List Deployments Tests
// =============================================================================

#[tokio::test]
async fn test_list_deployments_empty() {
    let config = test_config();
    let token = create_test_token(&config);
    let storage = Arc::new(InMemoryStorage::new());
    let app = build_router_with_storage(&config, storage);

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

    let body = extract_body(response).await;
    let deployments = body.as_array().expect("Expected array response");
    assert!(deployments.is_empty(), "Expected empty list");
}

#[tokio::test]
async fn test_list_deployments_with_data() {
    let config = test_config();
    let token = create_test_token(&config);
    let storage = Arc::new(InMemoryStorage::new());

    // Pre-populate storage with deployments
    let spec1 = zlayer_spec::from_yaml_str(&valid_deployment_spec("app-alpha")).unwrap();
    let spec2 = zlayer_spec::from_yaml_str(&valid_deployment_spec("app-beta")).unwrap();
    storage
        .store(&StoredDeployment::new(spec1))
        .await
        .unwrap();
    storage
        .store(&StoredDeployment::new(spec2))
        .await
        .unwrap();

    let app = build_router_with_storage(&config, storage);

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

    let body = extract_body(response).await;
    let deployments = body.as_array().expect("Expected array response");
    assert_eq!(deployments.len(), 2);

    // Verify deployments are sorted by name
    assert_eq!(deployments[0]["name"], "app-alpha");
    assert_eq!(deployments[1]["name"], "app-beta");

    // Verify summary fields are present
    assert!(deployments[0]["status"].as_str().is_some());
    assert!(deployments[0]["service_count"].as_i64().is_some());
    assert!(deployments[0]["created_at"].as_str().is_some());
}

#[tokio::test]
async fn test_list_deployments_requires_auth() {
    let config = test_config();
    let storage = Arc::new(InMemoryStorage::new());
    let app = build_router_with_storage(&config, storage);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/deployments")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "Expected 401 Unauthorized without token"
    );
}

// =============================================================================
// Get Deployment Tests
// =============================================================================

#[tokio::test]
async fn test_get_deployment_success() {
    let config = test_config();
    let token = create_test_token(&config);
    let storage = Arc::new(InMemoryStorage::new());

    // Pre-populate storage
    let spec = zlayer_spec::from_yaml_str(&valid_deployment_spec("my-app")).unwrap();
    storage
        .store(&StoredDeployment::new(spec))
        .await
        .unwrap();

    let app = build_router_with_storage(&config, storage);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/deployments/my-app")
                .header(header::AUTHORIZATION, format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = extract_body(response).await;
    assert_eq!(body["name"], "my-app");
    assert_eq!(body["status"], "pending");
    assert!(body["services"].as_array().is_some());
    assert!(body["created_at"].as_str().is_some());
    assert!(body["updated_at"].as_str().is_some());
}

#[tokio::test]
async fn test_get_deployment_not_found() {
    let config = test_config();
    let token = create_test_token(&config);
    let storage = Arc::new(InMemoryStorage::new());
    let app = build_router_with_storage(&config, storage);

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

    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "Expected 404 for non-existent deployment"
    );
}

#[tokio::test]
async fn test_get_deployment_requires_auth() {
    let config = test_config();
    let storage = Arc::new(InMemoryStorage::new());

    // Pre-populate storage
    let spec = zlayer_spec::from_yaml_str(&valid_deployment_spec("my-app")).unwrap();
    storage
        .store(&StoredDeployment::new(spec))
        .await
        .unwrap();

    let app = build_router_with_storage(&config, storage);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/deployments/my-app")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "Expected 401 Unauthorized without token"
    );
}

// =============================================================================
// Delete Deployment Tests
// =============================================================================

#[tokio::test]
async fn test_delete_deployment_success() {
    let config = test_config();
    let token = create_test_token(&config);
    let storage = Arc::new(InMemoryStorage::new());

    // Pre-populate storage
    let spec = zlayer_spec::from_yaml_str(&valid_deployment_spec("to-delete")).unwrap();
    storage
        .store(&StoredDeployment::new(spec))
        .await
        .unwrap();

    let app = build_router_with_storage(&config, storage.clone());

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/deployments/to-delete")
                .header(header::AUTHORIZATION, format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::NO_CONTENT,
        "Expected 204 No Content on successful delete"
    );

    // Verify it's actually gone
    let exists = storage.exists("to-delete").await.unwrap();
    assert!(!exists, "Deployment should be deleted from storage");
}

#[tokio::test]
async fn test_delete_deployment_not_found() {
    let config = test_config();
    let token = create_test_token(&config);
    let storage = Arc::new(InMemoryStorage::new());
    let app = build_router_with_storage(&config, storage);

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/deployments/nonexistent")
                .header(header::AUTHORIZATION, format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "Expected 404 for non-existent deployment"
    );
}

#[tokio::test]
async fn test_delete_deployment_requires_auth() {
    let config = test_config();
    let storage = Arc::new(InMemoryStorage::new());

    // Pre-populate storage
    let spec = zlayer_spec::from_yaml_str(&valid_deployment_spec("my-app")).unwrap();
    storage
        .store(&StoredDeployment::new(spec))
        .await
        .unwrap();

    let app = build_router_with_storage(&config, storage);

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/deployments/my-app")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "Expected 401 Unauthorized without token"
    );
}

// =============================================================================
// Full Lifecycle Test
// =============================================================================

#[tokio::test]
async fn test_deployment_full_lifecycle() {
    let config = test_config();
    let token = create_test_token(&config);
    let storage = Arc::new(InMemoryStorage::new());

    // Step 1: Create deployment
    {
        let app = build_router_with_storage(&config, storage.clone());
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/deployments")
                    .header(header::AUTHORIZATION, format!("Bearer {}", token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_string(&json!({
                            "spec": valid_deployment_spec("lifecycle-test")
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            response.status(),
            StatusCode::CREATED,
            "Step 1: Create should return 201"
        );

        let body = extract_body(response).await;
        assert_eq!(body["name"], "lifecycle-test");
    }

    // Step 2: List deployments - should contain our deployment
    {
        let app = build_router_with_storage(&config, storage.clone());
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

        assert_eq!(
            response.status(),
            StatusCode::OK,
            "Step 2: List should return 200"
        );

        let body = extract_body(response).await;
        let deployments = body.as_array().expect("Expected array");
        assert_eq!(deployments.len(), 1, "Step 2: Should have 1 deployment");
        assert_eq!(deployments[0]["name"], "lifecycle-test");
    }

    // Step 3: Get deployment details
    {
        let app = build_router_with_storage(&config, storage.clone());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/deployments/lifecycle-test")
                    .header(header::AUTHORIZATION, format!("Bearer {}", token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            response.status(),
            StatusCode::OK,
            "Step 3: Get should return 200"
        );

        let body = extract_body(response).await;
        assert_eq!(body["name"], "lifecycle-test");
        assert!(body["services"].as_array().is_some());
    }

    // Step 4: Delete deployment
    {
        let app = build_router_with_storage(&config, storage.clone());
        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/v1/deployments/lifecycle-test")
                    .header(header::AUTHORIZATION, format!("Bearer {}", token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            response.status(),
            StatusCode::NO_CONTENT,
            "Step 4: Delete should return 204"
        );
    }

    // Step 5: Verify deployment is gone
    {
        let app = build_router_with_storage(&config, storage.clone());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/deployments/lifecycle-test")
                    .header(header::AUTHORIZATION, format!("Bearer {}", token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "Step 5: Get after delete should return 404"
        );
    }

    // Step 6: List should be empty again
    {
        let app = build_router_with_storage(&config, storage.clone());
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

        assert_eq!(
            response.status(),
            StatusCode::OK,
            "Step 6: List should return 200"
        );

        let body = extract_body(response).await;
        let deployments = body.as_array().expect("Expected array");
        assert!(
            deployments.is_empty(),
            "Step 6: List should be empty after delete"
        );
    }
}

// =============================================================================
// Authentication Edge Cases
// =============================================================================

#[tokio::test]
async fn test_invalid_bearer_token_rejected() {
    let config = test_config();
    let storage = Arc::new(InMemoryStorage::new());
    let app = build_router_with_storage(&config, storage);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/deployments")
                .header(header::AUTHORIZATION, "Bearer invalid-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "Expected 401 for invalid token"
    );
}

#[tokio::test]
async fn test_malformed_auth_header_rejected() {
    let config = test_config();
    let storage = Arc::new(InMemoryStorage::new());
    let app = build_router_with_storage(&config, storage);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/deployments")
                .header(header::AUTHORIZATION, "Basic dXNlcjpwYXNz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "Expected 401 for non-Bearer auth scheme"
    );
}

// Note: Token expiration is tested at the unit level in auth.rs.
// At the integration level, JWT libraries have a default 60-second leeway
// for clock skew, making this test impractical without long waits.
// The invalid token and malformed header tests above cover auth rejection paths.

// =============================================================================
// Multiple Deployments Tests
// =============================================================================

#[tokio::test]
async fn test_create_multiple_deployments() {
    let config = test_config();
    let token = create_test_token(&config);
    let storage = Arc::new(InMemoryStorage::new());

    // Create first deployment
    {
        let app = build_router_with_storage(&config, storage.clone());
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/deployments")
                    .header(header::AUTHORIZATION, format!("Bearer {}", token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_string(&json!({
                            "spec": valid_deployment_spec("app-one")
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
    }

    // Create second deployment
    {
        let app = build_router_with_storage(&config, storage.clone());
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/deployments")
                    .header(header::AUTHORIZATION, format!("Bearer {}", token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_string(&json!({
                            "spec": valid_deployment_spec("app-two")
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
    }

    // Create third deployment
    {
        let app = build_router_with_storage(&config, storage.clone());
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/deployments")
                    .header(header::AUTHORIZATION, format!("Bearer {}", token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_string(&json!({
                            "spec": valid_deployment_spec("app-three")
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
    }

    // List all deployments
    {
        let app = build_router_with_storage(&config, storage.clone());
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

        let body = extract_body(response).await;
        let deployments = body.as_array().expect("Expected array");
        assert_eq!(deployments.len(), 3);

        // Verify sorted order
        assert_eq!(deployments[0]["name"], "app-one");
        assert_eq!(deployments[1]["name"], "app-three");
        assert_eq!(deployments[2]["name"], "app-two");
    }

    // Delete middle deployment
    {
        let app = build_router_with_storage(&config, storage.clone());
        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/v1/deployments/app-three")
                    .header(header::AUTHORIZATION, format!("Bearer {}", token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    // Verify only 2 remain
    {
        let app = build_router_with_storage(&config, storage.clone());
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

        let body = extract_body(response).await;
        let deployments = body.as_array().expect("Expected array");
        assert_eq!(deployments.len(), 2);
        assert_eq!(deployments[0]["name"], "app-one");
        assert_eq!(deployments[1]["name"], "app-two");
    }
}

// =============================================================================
// Content-Type and Request Body Tests
// =============================================================================

#[tokio::test]
async fn test_create_deployment_without_content_type() {
    let config = test_config();
    let token = create_test_token(&config);
    let storage = Arc::new(InMemoryStorage::new());
    let app = build_router_with_storage(&config, storage);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/deployments")
                .header(header::AUTHORIZATION, format!("Bearer {}", token))
                // Intentionally omitting Content-Type header
                .body(Body::from(
                    serde_json::to_string(&json!({
                        "spec": valid_deployment_spec("test-app")
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    // Axum typically returns 415 Unsupported Media Type when Content-Type is missing for JSON
    assert!(
        response.status() == StatusCode::UNSUPPORTED_MEDIA_TYPE
            || response.status() == StatusCode::BAD_REQUEST,
        "Expected 415 or 400 when Content-Type is missing, got {}",
        response.status()
    );
}

#[tokio::test]
async fn test_create_deployment_with_empty_body() {
    let config = test_config();
    let token = create_test_token(&config);
    let storage = Arc::new(InMemoryStorage::new());
    let app = build_router_with_storage(&config, storage);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/deployments")
                .header(header::AUTHORIZATION, format!("Bearer {}", token))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Empty body should fail JSON parsing
    assert!(
        response.status() == StatusCode::BAD_REQUEST
            || response.status() == StatusCode::UNPROCESSABLE_ENTITY,
        "Expected 400 or 422 for empty body, got {}",
        response.status()
    );
}

// =============================================================================
// Deployment with Complex Spec
// =============================================================================

#[tokio::test]
async fn test_create_deployment_with_multiple_services() {
    let config = test_config();
    let token = create_test_token(&config);
    let storage = Arc::new(InMemoryStorage::new());
    let app = build_router_with_storage(&config, storage);

    let complex_spec = r#"version: v1
deployment: complex-app
services:
  api:
    rtype: service
    image:
      name: api:v1.0.0
    endpoints:
      - name: http
        protocol: http
        port: 8080
    depends:
      - service: database
  database:
    rtype: service
    image:
      name: postgres:15
    endpoints:
      - name: postgres
        protocol: tcp
        port: 5432
  cache:
    rtype: service
    image:
      name: redis:7
    endpoints:
      - name: redis
        protocol: tcp
        port: 6379
"#;

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/deployments")
                .header(header::AUTHORIZATION, format!("Bearer {}", token))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_string(&json!({
                        "spec": complex_spec
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);

    let body = extract_body(response).await;
    assert_eq!(body["name"], "complex-app");

    let services = body["services"]
        .as_array()
        .expect("Expected services array");
    assert_eq!(services.len(), 3);
}
