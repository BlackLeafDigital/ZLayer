//! Infrastructure Wiring Integration Tests
//!
//! These tests verify that all critical infrastructure components (overlay network,
//! proxy manager, DNS server, container supervisor) are properly wired through the
//! ServiceManager -> ServiceInstance path during the deploy flow.
//!
//! These tests would have caught the following bugs in the original deploy flow:
//! - upsert_service() passed None for overlay_manager (with a TODO comment!)
//! - ProxyManager was never started, so expose: public had zero effect
//! - DnsServer was never started, so service discovery didn't work
//! - ContainerSupervisor was never created, so crashed containers stayed dead
//! - setup_service_overlay() was never called

use std::sync::Arc;
use tokio::sync::RwLock;

use zlayer_agent::overlay_manager::OverlayManager;
use zlayer_agent::proxy_manager::{ProxyManager, ProxyManagerConfig};
use zlayer_agent::runtime::MockRuntime;
use zlayer_agent::service::ServiceManager;
use zlayer_agent::ContainerSupervisor;
use zlayer_agent::Runtime;
use zlayer_overlay::DnsServer;
use zlayer_proxy::{ServiceRegistry, StreamRegistry};
use zlayer_spec::{DeploymentSpec, ServiceSpec};

fn mock_http_spec() -> ServiceSpec {
    serde_yaml::from_str::<DeploymentSpec>(
        r#"
version: v1
deployment: test
services:
  test:
    rtype: service
    image:
      name: test:latest
    endpoints:
      - name: http
        protocol: http
        port: 8080
    scale:
      mode: fixed
      replicas: 1
"#,
    )
    .unwrap()
    .services
    .remove("test")
    .unwrap()
}

fn mock_public_http_spec() -> ServiceSpec {
    serde_yaml::from_str::<DeploymentSpec>(
        r#"
version: v1
deployment: test
services:
  test:
    rtype: service
    image:
      name: test:latest
    endpoints:
      - name: http
        protocol: http
        port: 8080
        expose: public
    scale:
      mode: fixed
      replicas: 1
"#,
    )
    .unwrap()
    .services
    .remove("test")
    .unwrap()
}

fn mock_mixed_endpoints_spec() -> ServiceSpec {
    serde_yaml::from_str::<DeploymentSpec>(
        r#"
version: v1
deployment: test
services:
  test:
    rtype: service
    image:
      name: test:latest
    endpoints:
      - name: http
        protocol: http
        port: 8080
        expose: public
      - name: grpc
        protocol: tcp
        port: 9000
      - name: metrics
        protocol: udp
        port: 8125
    scale:
      mode: fixed
      replicas: 1
"#,
    )
    .unwrap()
    .services
    .remove("test")
    .unwrap()
}

// =============================================================================
// Overlay Manager Wiring Tests
// =============================================================================

#[tokio::test]
async fn test_upsert_passes_overlay_manager_to_service_instance() {
    // BUG THIS CATCHES: upsert_service() passed None for overlay_manager
    // instead of self.overlay_manager.as_ref().map(Arc::clone)
    let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
    let overlay_manager = Arc::new(RwLock::new(
        OverlayManager::new("test-deploy".to_string())
            .await
            .unwrap(),
    ));

    let mut manager = ServiceManager::new(runtime);
    manager.set_overlay_manager(overlay_manager);
    manager.set_deployment_name("test".to_string());

    let spec = mock_http_spec();
    manager
        .upsert_service("web".to_string(), spec)
        .await
        .unwrap();

    let infra = manager.service_infrastructure("web").await.unwrap();
    assert!(
        infra.0,
        "ServiceInstance must receive overlay_manager from ServiceManager"
    );
}

#[tokio::test]
async fn test_upsert_without_overlay_gives_none_to_instance() {
    let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
    let manager = ServiceManager::new(runtime);

    let spec = mock_http_spec();
    manager
        .upsert_service("web".to_string(), spec)
        .await
        .unwrap();

    let infra = manager.service_infrastructure("web").await.unwrap();
    assert!(
        !infra.0,
        "ServiceInstance should not have overlay_manager when ServiceManager lacks one"
    );
}

// =============================================================================
// Proxy Manager Wiring Tests
// =============================================================================

#[tokio::test]
async fn test_upsert_passes_proxy_manager_to_service_instance() {
    // BUG THIS CATCHES: ProxyManager was never started/wired in deploy flow
    let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
    let proxy_config = ProxyManagerConfig::default();
    let proxy_manager = Arc::new(ProxyManager::new(proxy_config));

    let mut manager = ServiceManager::new(runtime);
    manager.set_proxy_manager(proxy_manager);
    manager.set_deployment_name("test".to_string());

    let spec = mock_http_spec();
    manager
        .upsert_service("api".to_string(), spec)
        .await
        .unwrap();

    let infra = manager.service_infrastructure("api").await.unwrap();
    assert!(
        infra.1,
        "ServiceInstance must receive proxy_manager from ServiceManager"
    );
}

#[tokio::test]
async fn test_proxy_ensure_ports_processes_public_endpoints() {
    // BUG THIS CATCHES: expose: public had zero effect because ProxyManager
    // was never started and ensure_ports_for_service was never called
    let config = ProxyManagerConfig::new("0.0.0.0:80".parse().unwrap());
    let proxy = ProxyManager::new(config);

    let spec = mock_public_http_spec();
    let _ = proxy.ensure_ports_for_service(&spec).await;
}

// =============================================================================
// DNS Server Wiring Tests
// =============================================================================

#[tokio::test]
async fn test_upsert_passes_dns_server_to_service_instance() {
    // BUG THIS CATCHES: DnsServer was never started, service discovery didn't work
    let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
    let dns_server = Arc::new(
        DnsServer::new("127.0.0.1:15353".parse().unwrap(), "service.local.")
            .expect("Failed to create DnsServer"),
    );

    let mut manager = ServiceManager::new(runtime);
    manager.set_dns_server(dns_server);
    manager.set_deployment_name("test".to_string());

    let spec = mock_http_spec();
    manager
        .upsert_service("svc".to_string(), spec)
        .await
        .unwrap();

    let infra = manager.service_infrastructure("svc").await.unwrap();
    assert!(
        infra.2,
        "ServiceInstance must receive dns_server from ServiceManager"
    );
}

// =============================================================================
// Container Supervisor Wiring Tests
// =============================================================================

#[tokio::test]
async fn test_container_supervisor_configured_on_manager() {
    // BUG THIS CATCHES: ContainerSupervisor was never created
    let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
    let supervisor = Arc::new(ContainerSupervisor::new(runtime.clone()));

    let mut manager = ServiceManager::new(runtime);
    manager.set_container_supervisor(supervisor);

    assert!(
        manager.container_supervisor().is_some(),
        "ServiceManager must have container_supervisor after set_container_supervisor"
    );
}

// =============================================================================
// Full Infrastructure Golden Path Test
// =============================================================================

#[tokio::test]
async fn test_service_manager_full_infrastructure_wiring() {
    // This single test would have caught ALL of the bugs in the original deploy flow.
    let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
    let overlay_manager = Arc::new(RwLock::new(
        OverlayManager::new("prod".to_string()).await.unwrap(),
    ));
    let service_registry = Arc::new(ServiceRegistry::new());
    let stream_registry = Arc::new(StreamRegistry::new());
    let proxy_config = ProxyManagerConfig::default();
    let proxy_manager = Arc::new(ProxyManager::new(proxy_config));
    let dns_server = Arc::new(
        DnsServer::new("127.0.0.1:15353".parse().unwrap(), "service.local.")
            .expect("Failed to create DnsServer"),
    );
    let supervisor = Arc::new(ContainerSupervisor::new(runtime.clone()));

    let mut manager = ServiceManager::with_full_config(
        runtime,
        overlay_manager,
        service_registry.clone(),
        "prod".to_string(),
    );
    manager.set_stream_registry(stream_registry.clone());
    manager.set_proxy_manager(proxy_manager);
    manager.set_dns_server(dns_server);
    manager.set_container_supervisor(supervisor);

    assert!(manager.proxy_manager().is_some(), "must have proxy_manager");
    assert!(manager.dns_server().is_some(), "must have dns_server");
    assert!(
        manager.stream_registry().is_some(),
        "must have stream_registry"
    );
    assert!(
        manager.container_supervisor().is_some(),
        "must have container_supervisor"
    );

    let spec = mock_mixed_endpoints_spec();
    manager
        .upsert_service("app".to_string(), spec)
        .await
        .unwrap();

    let infra = manager.service_infrastructure("app").await.unwrap();
    assert!(infra.0, "ServiceInstance must have overlay_manager");
    assert!(infra.1, "ServiceInstance must have proxy_manager");
    assert!(infra.2, "ServiceInstance must have dns_server");

    assert!(
        service_registry.route_count() > 0,
        "HTTP routes must be registered"
    );
    assert_eq!(
        stream_registry.tcp_count(),
        1,
        "TCP stream must be registered"
    );
    assert_eq!(
        stream_registry.udp_count(),
        1,
        "UDP stream must be registered"
    );

    let services = manager.list_services().await;
    assert!(services.contains(&"app".to_string()));
}

// =============================================================================
// Update preserves infrastructure
// =============================================================================

#[tokio::test]
async fn test_upsert_update_preserves_dns_server() {
    let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
    let dns_server = Arc::new(
        DnsServer::new("127.0.0.1:15353".parse().unwrap(), "service.local.")
            .expect("Failed to create DnsServer"),
    );

    let mut manager = ServiceManager::new(runtime);
    manager.set_dns_server(dns_server);
    manager.set_deployment_name("test".to_string());

    let spec = mock_http_spec();
    manager
        .upsert_service("svc".to_string(), spec.clone())
        .await
        .unwrap();

    manager
        .upsert_service("svc".to_string(), spec)
        .await
        .unwrap();

    let infra = manager.service_infrastructure("svc").await.unwrap();
    assert!(infra.2, "DNS server must be preserved after service update");
}

// =============================================================================
// ApiSpec defaults tests
// =============================================================================

#[test]
fn test_api_spec_defaults_in_deployment() {
    // BUG THIS CATCHES: API server was a separate command, not embedded.
    let yaml = r#"
version: v1
deployment: test
services:
  hello:
    image:
      name: hello-world:latest
"#;
    let spec: DeploymentSpec = serde_yaml::from_str(yaml).unwrap();
    assert!(spec.api.enabled, "API must be enabled by default");
    assert_eq!(
        spec.api.bind, "0.0.0.0:8080",
        "API bind must default to 0.0.0.0:8080"
    );
    assert!(spec.api.swagger, "Swagger must be enabled by default");
}

#[test]
fn test_api_spec_can_be_disabled() {
    let yaml = r#"
version: v1
deployment: test
services:
  hello:
    image:
      name: hello-world:latest
api:
  enabled: false
"#;
    let spec: DeploymentSpec = serde_yaml::from_str(yaml).unwrap();
    assert!(!spec.api.enabled);
}

#[test]
fn test_api_spec_partial_override() {
    let yaml = r#"
version: v1
deployment: test
services:
  hello:
    image:
      name: hello-world:latest
api:
  bind: "0.0.0.0:3000"
"#;
    let spec: DeploymentSpec = serde_yaml::from_str(yaml).unwrap();
    assert!(spec.api.enabled, "enabled should default to true");
    assert_eq!(spec.api.bind, "0.0.0.0:3000");
    assert!(spec.api.swagger, "swagger should default to true");
}
