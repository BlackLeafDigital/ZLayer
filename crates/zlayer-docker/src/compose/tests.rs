//! Integration tests for compose parsing and conversion.
//!
//! These tests exercise the public API: `parse_compose` + `compose_to_deployment`.
//! Unit tests for private helpers live in `convert.rs`'s inline `#[cfg(test)]` module.

use std::time::Duration;

use zlayer_spec::{DependencyCondition, HealthCheck, Protocol, ScaleSpec, StorageSpec};

use super::convert::compose_to_deployment;
use super::types::{ComposePort, ComposeServiceNetworks, ComposeVolume, DependsOnCondition};
use crate::compose::parse_compose;

// =========================================================================
// ComposePort::parse (pub method on a pub type)
// =========================================================================

#[test]
fn test_compose_port_parse_simple() {
    let port = ComposePort::parse("80").unwrap();
    assert_eq!(port.container_port, "80");
    assert!(port.host_port.is_none());
    assert!(port.host_ip.is_none());
    assert!(port.protocol.is_none());
}

#[test]
fn test_compose_port_parse_host_container() {
    let port = ComposePort::parse("8080:80").unwrap();
    assert_eq!(port.host_port.as_deref(), Some("8080"));
    assert_eq!(port.container_port, "80");
    assert!(port.host_ip.is_none());
}

#[test]
fn test_compose_port_parse_with_protocol() {
    let port = ComposePort::parse("8080:80/tcp").unwrap();
    assert_eq!(port.host_port.as_deref(), Some("8080"));
    assert_eq!(port.container_port, "80");
    assert_eq!(port.protocol.as_deref(), Some("tcp"));
}

#[test]
fn test_compose_port_parse_with_udp() {
    let port = ComposePort::parse("5353:53/udp").unwrap();
    assert_eq!(port.host_port.as_deref(), Some("5353"));
    assert_eq!(port.container_port, "53");
    assert_eq!(port.protocol.as_deref(), Some("udp"));
}

#[test]
fn test_compose_port_parse_host_ip() {
    let port = ComposePort::parse("127.0.0.1:8080:80").unwrap();
    assert_eq!(port.host_ip.as_deref(), Some("127.0.0.1"));
    assert_eq!(port.host_port.as_deref(), Some("8080"));
    assert_eq!(port.container_port, "80");
}

#[test]
fn test_compose_port_parse_host_ip_with_protocol() {
    let port = ComposePort::parse("127.0.0.1:8080:80/tcp").unwrap();
    assert_eq!(port.host_ip.as_deref(), Some("127.0.0.1"));
    assert_eq!(port.host_port.as_deref(), Some("8080"));
    assert_eq!(port.container_port, "80");
    assert_eq!(port.protocol.as_deref(), Some("tcp"));
}

#[test]
fn test_compose_port_parse_host_ip_random_port() {
    let port = ComposePort::parse("127.0.0.1::80").unwrap();
    assert_eq!(port.host_ip.as_deref(), Some("127.0.0.1"));
    assert!(port.host_port.is_none());
    assert_eq!(port.container_port, "80");
}

#[test]
fn test_compose_port_parse_range() {
    let port = ComposePort::parse("9090-9091:8080-8081").unwrap();
    assert_eq!(port.host_port.as_deref(), Some("9090-9091"));
    assert_eq!(port.container_port, "8080-8081");
}

#[test]
fn test_compose_port_parse_empty_fails() {
    assert!(ComposePort::parse("").is_err());
    assert!(ComposePort::parse("  ").is_err());
}

// =========================================================================
// ComposeVolume::parse (pub method on a pub type)
// =========================================================================

#[test]
fn test_compose_volume_parse_anonymous() {
    let vol = ComposeVolume::parse("/app/data").unwrap();
    assert!(vol.source.is_none());
    assert_eq!(vol.target.as_deref(), Some("/app/data"));
}

#[test]
fn test_compose_volume_parse_named() {
    let vol = ComposeVolume::parse("mydata:/app/data").unwrap();
    assert_eq!(vol.source.as_deref(), Some("mydata"));
    assert_eq!(vol.target.as_deref(), Some("/app/data"));
    assert!(!vol.read_only);
}

#[test]
fn test_compose_volume_parse_bind_relative() {
    let vol = ComposeVolume::parse("./data:/app/data").unwrap();
    assert_eq!(vol.source.as_deref(), Some("./data"));
    assert_eq!(vol.target.as_deref(), Some("/app/data"));
    assert!(
        matches!(vol.volume_type, Some(super::types::VolumeType::Bind)),
        "expected Bind volume type"
    );
}

#[test]
fn test_compose_volume_parse_readonly() {
    let vol = ComposeVolume::parse("./data:/app/data:ro").unwrap();
    assert!(vol.read_only);
}

#[test]
fn test_compose_volume_parse_empty_fails() {
    assert!(ComposeVolume::parse("").is_err());
}

// =========================================================================
// Empty compose file
// =========================================================================

#[test]
fn test_empty_compose_file() {
    let yaml = "services: {}\n";
    let compose = parse_compose(yaml).unwrap();
    assert!(compose.services.is_empty());

    let spec = compose_to_deployment(&compose, "empty-app").unwrap();
    assert_eq!(spec.services.len(), 0);
    assert_eq!(spec.deployment, "empty-app");
    assert_eq!(spec.version, "v1");
}

#[test]
fn test_minimal_compose_file() {
    let yaml = "services:\n  web:\n    image: nginx\n";
    let compose = parse_compose(yaml).unwrap();
    assert_eq!(compose.services.len(), 1);
    assert!(compose.services.contains_key("web"));
}

// =========================================================================
// Build-only and build+image services
// =========================================================================

#[test]
fn test_build_with_image() {
    let yaml = r"
services:
  app:
    build:
      context: .
      dockerfile: Dockerfile
      args:
        NODE_ENV: production
    image: myapp:v1
";
    let compose = parse_compose(yaml).unwrap();
    let spec = compose_to_deployment(&compose, "build-img-test").unwrap();

    let app = &spec.services["app"];
    assert_eq!(app.image.name, "myapp:v1");
}

// =========================================================================
// Complex port mappings
// =========================================================================

#[test]
fn test_complex_port_mappings() {
    let yaml = r#"
services:
  web:
    image: nginx:latest
    ports:
      - "80:80"
      - "443:443"
      - "127.0.0.1:8080:80/tcp"
      - "9090-9091:8080-8081"
      - "5353:53/udp"
"#;
    let compose = parse_compose(yaml).unwrap();
    let spec = compose_to_deployment(&compose, "ports-test").unwrap();

    let web = &spec.services["web"];
    assert_eq!(web.endpoints.len(), 5);

    // Port 80:80 -> HTTP
    let ep80 = web.endpoints.iter().find(|e| e.port == 80).unwrap();
    assert_eq!(ep80.target_port, Some(80));
    assert_eq!(ep80.protocol, Protocol::Http);

    // Port 443:443 -> HTTP (well-known)
    let ep443 = web.endpoints.iter().find(|e| e.port == 443).unwrap();
    assert_eq!(ep443.target_port, Some(443));
    assert_eq!(ep443.protocol, Protocol::Http);

    // 127.0.0.1:8080:80/tcp -> port 8080 targeting 80, inferred HTTP
    let ep8080 = web
        .endpoints
        .iter()
        .find(|e| e.port == 8080 && e.target_port == Some(80))
        .unwrap();
    assert_eq!(ep8080.protocol, Protocol::Http);

    // 5353:53/udp -> UDP
    let ep5353 = web.endpoints.iter().find(|e| e.port == 5353).unwrap();
    assert_eq!(ep5353.target_port, Some(53));
    assert_eq!(ep5353.protocol, Protocol::Udp);
}

#[test]
fn test_port_range_takes_first() {
    let yaml = r#"
services:
  app:
    image: myapp:latest
    ports:
      - "9090-9091:8080-8081"
"#;
    let compose = parse_compose(yaml).unwrap();
    let spec = compose_to_deployment(&compose, "range-test").unwrap();

    let app = &spec.services["app"];
    assert_eq!(app.endpoints.len(), 1);
    assert_eq!(app.endpoints[0].port, 9090);
    assert_eq!(app.endpoints[0].target_port, Some(8080));
}

#[test]
fn test_long_syntax_ports() {
    let yaml = r#"
services:
  web:
    image: nginx:latest
    ports:
      - target: 80
        published: 8080
        protocol: tcp
      - target: 443
        published: 8443
        protocol: tcp
        host_ip: "0.0.0.0"
"#;
    let compose = parse_compose(yaml).unwrap();
    let spec = compose_to_deployment(&compose, "long-port-test").unwrap();

    let web = &spec.services["web"];
    assert_eq!(web.endpoints.len(), 2);

    let ep80 = web
        .endpoints
        .iter()
        .find(|e| e.target_port == Some(80))
        .unwrap();
    assert_eq!(ep80.port, 8080);
    assert_eq!(ep80.protocol, Protocol::Http);

    let ep443 = web
        .endpoints
        .iter()
        .find(|e| e.target_port == Some(443))
        .unwrap();
    assert_eq!(ep443.port, 8443);
}

// =========================================================================
// Volume driver options (long syntax)
// =========================================================================

#[test]
fn test_volume_driver_options() {
    let yaml = r"
services:
  app:
    image: myapp:latest
    volumes:
      - type: volume
        source: mydata
        target: /app/data
        volume:
          nocopy: true
      - type: bind
        source: ./config
        target: /app/config
        read_only: true
        bind:
          propagation: rprivate
          create_host_path: true
";
    let compose = parse_compose(yaml).unwrap();
    let spec = compose_to_deployment(&compose, "vol-driver-test").unwrap();

    let app = &spec.services["app"];
    assert_eq!(app.storage.len(), 2);

    match &app.storage[0] {
        StorageSpec::Named {
            name,
            target,
            readonly,
            ..
        } => {
            assert_eq!(name, "mydata");
            assert_eq!(target, "/app/data");
            assert!(!readonly);
        }
        other => panic!("expected Named storage, got: {other:?}"),
    }

    match &app.storage[1] {
        StorageSpec::Bind {
            source,
            target,
            readonly,
        } => {
            assert_eq!(source, "./config");
            assert_eq!(target, "/app/config");
            assert!(readonly);
        }
        other => panic!("expected Bind storage, got: {other:?}"),
    }
}

#[test]
fn test_tmpfs_volume_with_options() {
    let yaml = r#"
services:
  app:
    image: myapp:latest
    volumes:
      - type: tmpfs
        target: /app/tmp
        tmpfs:
          size: "64M"
          mode: 1777
"#;
    let compose = parse_compose(yaml).unwrap();
    let spec = compose_to_deployment(&compose, "tmpfs-test").unwrap();

    let app = &spec.services["app"];
    assert_eq!(app.storage.len(), 1);

    match &app.storage[0] {
        StorageSpec::Tmpfs { target, size, mode } => {
            assert_eq!(target, "/app/tmp");
            assert_eq!(size.as_deref(), Some("64M"));
            assert_eq!(*mode, Some(1777));
        }
        other => panic!("expected Tmpfs storage, got: {other:?}"),
    }
}

// =========================================================================
// Multiple networks per service
// =========================================================================

#[test]
fn test_multiple_networks_list() {
    let yaml = r"
services:
  web:
    image: nginx:latest
    networks:
      - frontend
      - backend

networks:
  frontend:
  backend:
";
    let compose = parse_compose(yaml).unwrap();
    assert_eq!(compose.networks.len(), 2);

    let web = &compose.services["web"];
    match &web.networks {
        Some(ComposeServiceNetworks::List(list)) => {
            assert_eq!(list.len(), 2);
            assert!(list.contains(&"frontend".to_string()));
            assert!(list.contains(&"backend".to_string()));
        }
        other => panic!("expected List networks, got: {other:?}"),
    }

    // Conversion succeeds; ZLayer ignores networks but does not error.
    let spec = compose_to_deployment(&compose, "net-test").unwrap();
    assert!(spec.services.contains_key("web"));
}

#[test]
fn test_multiple_networks_map_with_config() {
    let yaml = r#"
services:
  web:
    image: nginx:latest
    networks:
      frontend:
        aliases:
          - web-frontend
        ipv4_address: "172.20.0.10"
      backend:
        aliases:
          - web-backend

networks:
  frontend:
    driver: bridge
  backend:
    driver: bridge
"#;
    let compose = parse_compose(yaml).unwrap();

    let web = &compose.services["web"];
    match &web.networks {
        Some(ComposeServiceNetworks::Map(map)) => {
            assert_eq!(map.len(), 2);
            let frontend = map.get("frontend").unwrap().as_ref().unwrap();
            assert!(frontend.aliases.contains(&"web-frontend".to_string()));
            assert_eq!(frontend.ipv4_address.as_deref(), Some("172.20.0.10"));
            let backend = map.get("backend").unwrap().as_ref().unwrap();
            assert!(backend.aliases.contains(&"web-backend".to_string()));
        }
        other => panic!("expected Map networks, got: {other:?}"),
    }
}

// =========================================================================
// Profiles
// =========================================================================

#[test]
fn test_profiles_parsing() {
    let yaml = r"
services:
  web:
    image: nginx:latest
    profiles:
      - production
  debug:
    image: busybox:latest
    profiles:
      - debug
      - development
  always:
    image: redis:7
";
    let compose = parse_compose(yaml).unwrap();

    let web = &compose.services["web"];
    assert_eq!(web.profiles, vec!["production".to_string()]);

    let debug = &compose.services["debug"];
    assert_eq!(
        debug.profiles,
        vec!["debug".to_string(), "development".to_string()]
    );

    let always = &compose.services["always"];
    assert!(always.profiles.is_empty());

    // Conversion still succeeds; profiles are a compose-level concept.
    let spec = compose_to_deployment(&compose, "profile-test").unwrap();
    assert_eq!(spec.services.len(), 3);
}

// =========================================================================
// env_file resolution (mock with temp file)
// =========================================================================

#[test]
fn test_env_file_single_path() {
    use std::io::Write;
    let dir = std::env::temp_dir().join("zlayer-test-envfile-single");
    let _ = std::fs::create_dir_all(&dir);
    let env_path = dir.join(".env");
    let mut f = std::fs::File::create(&env_path).unwrap();
    writeln!(f, "# comment").unwrap();
    writeln!(f, "DB_HOST=localhost").unwrap();
    writeln!(f, "DB_PORT=5432").unwrap();
    writeln!(f, "QUOTED=\"hello world\"").unwrap();
    writeln!(f, "SINGLE_QUOTED='foo'").unwrap();
    drop(f);

    let yaml = format!(
        r#"
services:
  app:
    image: myapp:latest
    env_file: "{}"
    environment:
      DB_PORT: "9999"
"#,
        env_path.display()
    );
    let compose = parse_compose(&yaml).unwrap();
    let spec = compose_to_deployment(&compose, "envfile-test").unwrap();

    let app = &spec.services["app"];
    assert_eq!(app.env.get("DB_HOST").unwrap(), "localhost");
    // Inline environment overrides env_file.
    assert_eq!(app.env.get("DB_PORT").unwrap(), "9999");
    // Quotes are stripped.
    assert_eq!(app.env.get("QUOTED").unwrap(), "hello world");
    assert_eq!(app.env.get("SINGLE_QUOTED").unwrap(), "foo");
}

#[test]
fn test_env_file_list_with_required_false() {
    let yaml = r"
services:
  app:
    image: myapp:latest
    env_file:
      - path: /nonexistent/required.env
        required: false
      - path: /nonexistent/also-optional.env
        required: false
";
    let compose = parse_compose(yaml).unwrap();
    // Both files are optional and missing, so conversion should succeed.
    let spec = compose_to_deployment(&compose, "envfile-opt-test").unwrap();
    let app = &spec.services["app"];
    assert!(app.env.is_empty());
}

#[test]
fn test_env_file_required_missing_fails() {
    let yaml = r"
services:
  app:
    image: myapp:latest
    env_file: /nonexistent/required.env
";
    let compose = parse_compose(yaml).unwrap();
    let result = compose_to_deployment(&compose, "envfile-fail-test");
    assert!(result.is_err());
}

#[test]
fn test_env_file_list_mixed() {
    use std::io::Write;
    let dir = std::env::temp_dir().join("zlayer-test-envfile-mixed");
    let _ = std::fs::create_dir_all(&dir);
    let env_path = dir.join("real.env");
    let mut f = std::fs::File::create(&env_path).unwrap();
    writeln!(f, "REAL_VAR=yes").unwrap();
    drop(f);

    let yaml = format!(
        r#"
services:
  app:
    image: myapp:latest
    env_file:
      - "{}"
      - path: /nonexistent/optional.env
        required: false
"#,
        env_path.display()
    );
    let compose = parse_compose(&yaml).unwrap();
    let spec = compose_to_deployment(&compose, "envfile-mixed-test").unwrap();

    let app = &spec.services["app"];
    assert_eq!(app.env.get("REAL_VAR").unwrap(), "yes");
}

// =========================================================================
// depends_on with conditions
// =========================================================================

#[test]
fn test_depends_on_completed_successfully() {
    let yaml = r"
services:
  app:
    image: myapp:latest
    depends_on:
      migration:
        condition: service_completed_successfully
  migration:
    image: migrate:latest
";
    let compose = parse_compose(yaml).unwrap();
    let spec = compose_to_deployment(&compose, "deps-complete-test").unwrap();

    let app = &spec.services["app"];
    assert_eq!(app.depends.len(), 1);

    let dep = &app.depends[0];
    assert_eq!(dep.service, "migration");
    assert_eq!(dep.condition, DependencyCondition::Ready);
    assert_eq!(dep.timeout, Some(Duration::from_secs(300)));
}

#[test]
fn test_depends_on_simple_list() {
    let yaml = r"
services:
  web:
    image: nginx:latest
    depends_on:
      - db
      - cache
  db:
    image: postgres:16
  cache:
    image: redis:7
";
    let compose = parse_compose(yaml).unwrap();

    let web = &compose.services["web"];
    assert_eq!(web.depends_on.len(), 2);
    assert_eq!(
        web.depends_on["db"].condition,
        DependsOnCondition::ServiceStarted
    );
    assert_eq!(
        web.depends_on["cache"].condition,
        DependsOnCondition::ServiceStarted
    );

    let spec = compose_to_deployment(&compose, "deps-list-test").unwrap();
    let web_spec = &spec.services["web"];
    assert_eq!(web_spec.depends.len(), 2);
    for dep in &web_spec.depends {
        assert_eq!(dep.condition, DependencyCondition::Started);
    }
}

#[test]
fn test_depends_on_with_restart() {
    let yaml = r"
services:
  web:
    image: nginx:latest
    depends_on:
      db:
        condition: service_healthy
        restart: true
  db:
    image: postgres:16
";
    let compose = parse_compose(yaml).unwrap();
    let web = &compose.services["web"];
    assert_eq!(web.depends_on["db"].restart, Some(true));
}

// =========================================================================
// Deploy resources + scale
// =========================================================================

#[test]
fn test_scale_from_top_level_scale_field() {
    let yaml = r"
services:
  app:
    image: myapp:latest
    scale: 5
";
    let compose = parse_compose(yaml).unwrap();
    let spec = compose_to_deployment(&compose, "scale-test").unwrap();

    let app = &spec.services["app"];
    assert_eq!(app.scale, ScaleSpec::Fixed { replicas: 5 });
}

#[test]
fn test_deploy_replicas_overrides_scale() {
    let yaml = r"
services:
  app:
    image: myapp:latest
    scale: 5
    deploy:
      replicas: 3
";
    let compose = parse_compose(yaml).unwrap();
    let spec = compose_to_deployment(&compose, "scale-override-test").unwrap();

    let app = &spec.services["app"];
    assert_eq!(app.scale, ScaleSpec::Fixed { replicas: 3 });
}

// =========================================================================
// Healthcheck variants
// =========================================================================

#[test]
fn test_healthcheck_cmd_shell() {
    let yaml = r#"
services:
  db:
    image: postgres:16
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U user"]
      interval: 10s
      timeout: 5s
      retries: 5
"#;
    let compose = parse_compose(yaml).unwrap();
    let spec = compose_to_deployment(&compose, "health-shell-test").unwrap();

    let db = &spec.services["db"];
    match &db.health.check {
        HealthCheck::Command { command } => {
            assert_eq!(command, "pg_isready -U user");
        }
        other => panic!("expected Command health check, got: {other:?}"),
    }
}

#[test]
fn test_healthcheck_string_form() {
    let yaml = r#"
services:
  web:
    image: nginx:latest
    healthcheck:
      test: "curl -f http://localhost || exit 1"
      interval: 15s
"#;
    let compose = parse_compose(yaml).unwrap();
    let spec = compose_to_deployment(&compose, "health-str-test").unwrap();

    let web = &spec.services["web"];
    match &web.health.check {
        HealthCheck::Command { command } => {
            assert_eq!(command, "curl -f http://localhost || exit 1");
        }
        other => panic!("expected Command health check, got: {other:?}"),
    }
}

#[test]
fn test_healthcheck_disabled() {
    let yaml = r"
services:
  web:
    image: nginx:latest
    healthcheck:
      disable: true
";
    let compose = parse_compose(yaml).unwrap();
    let spec = compose_to_deployment(&compose, "health-disabled-test").unwrap();

    let web = &spec.services["web"];
    match &web.health.check {
        HealthCheck::Tcp { port } => {
            assert_eq!(*port, 0);
        }
        other => panic!("expected Tcp health check (default), got: {other:?}"),
    }
}

#[test]
fn test_healthcheck_none_test() {
    let yaml = r#"
services:
  web:
    image: nginx:latest
    healthcheck:
      test: ["NONE"]
"#;
    let compose = parse_compose(yaml).unwrap();
    let spec = compose_to_deployment(&compose, "health-none-test").unwrap();

    let web = &spec.services["web"];
    match &web.health.check {
        HealthCheck::Tcp { port } => {
            assert_eq!(*port, 0);
        }
        other => panic!("expected Tcp health check for NONE, got: {other:?}"),
    }
}

// =========================================================================
// Anonymous volume
// =========================================================================

#[test]
fn test_anonymous_volume() {
    let yaml = r"
services:
  app:
    image: myapp:latest
    volumes:
      - /app/data
";
    let compose = parse_compose(yaml).unwrap();
    let spec = compose_to_deployment(&compose, "anon-vol-test").unwrap();

    let app = &spec.services["app"];
    assert_eq!(app.storage.len(), 1);
    match &app.storage[0] {
        StorageSpec::Anonymous { target, .. } => {
            assert_eq!(target, "/app/data");
        }
        other => panic!("expected Anonymous storage, got: {other:?}"),
    }
}

// =========================================================================
// Command string form
// =========================================================================

#[test]
fn test_command_string_form() {
    let yaml = r#"
services:
  app:
    image: myapp:latest
    command: "echo hello world"
"#;
    let compose = parse_compose(yaml).unwrap();
    let spec = compose_to_deployment(&compose, "cmd-str-test").unwrap();

    let app = &spec.services["app"];
    assert_eq!(app.command.args, Some(vec!["echo hello world".to_string()]));
}

// =========================================================================
// Error cases
// =========================================================================

#[test]
fn test_invalid_yaml_fails() {
    let yaml = "this is not valid yaml: [[[";
    let result = parse_compose(yaml);
    assert!(result.is_err());
}

// =========================================================================
// Environment variable formats
// =========================================================================

#[test]
fn test_environment_list_format() {
    let yaml = r"
services:
  app:
    image: myapp:latest
    environment:
      - FOO=bar
      - BAZ=qux
      - DEBUG
";
    let compose = parse_compose(yaml).unwrap();
    let app = &compose.services["app"];
    assert_eq!(app.environment.get("FOO").unwrap(), "bar");
    assert_eq!(app.environment.get("BAZ").unwrap(), "qux");
    assert_eq!(app.environment.get("DEBUG").unwrap(), "");
}

#[test]
fn test_environment_numeric_values() {
    let yaml = r"
services:
  app:
    image: myapp:latest
    environment:
      PORT: 3000
      DEBUG: true
      RATIO: 1.5
";
    let compose = parse_compose(yaml).unwrap();
    let app = &compose.services["app"];
    assert_eq!(app.environment.get("PORT").unwrap(), "3000");
    assert_eq!(app.environment.get("DEBUG").unwrap(), "true");
    assert_eq!(app.environment.get("RATIO").unwrap(), "1.5");
}

// =========================================================================
// Labels
// =========================================================================

#[test]
fn test_labels_map_format() {
    let yaml = r#"
services:
  app:
    image: myapp:latest
    labels:
      com.example.description: "My app"
      com.example.team: platform
"#;
    let compose = parse_compose(yaml).unwrap();
    let app = &compose.services["app"];
    assert_eq!(app.labels.get("com.example.description").unwrap(), "My app");
    assert_eq!(app.labels.get("com.example.team").unwrap(), "platform");
}

#[test]
fn test_labels_list_format() {
    let yaml = r#"
services:
  app:
    image: myapp:latest
    labels:
      - "com.example.description=My app"
      - "com.example.team=platform"
"#;
    let compose = parse_compose(yaml).unwrap();
    let app = &compose.services["app"];
    assert_eq!(app.labels.get("com.example.description").unwrap(), "My app");
    assert_eq!(app.labels.get("com.example.team").unwrap(), "platform");
}

// =========================================================================
// Secrets and configs (top-level + service-level)
// =========================================================================

#[test]
fn test_top_level_secrets_and_configs() {
    let yaml = r"
services:
  app:
    image: myapp:latest
    secrets:
      - db_password
    configs:
      - app_config

secrets:
  db_password:
    file: ./secrets/db_password.txt

configs:
  app_config:
    file: ./config/app.json
";
    let compose = parse_compose(yaml).unwrap();
    assert_eq!(compose.secrets.len(), 1);
    assert_eq!(compose.configs.len(), 1);

    let app = &compose.services["app"];
    assert_eq!(app.secrets.len(), 1);
    assert_eq!(app.configs.len(), 1);

    let spec = compose_to_deployment(&compose, "secrets-test").unwrap();
    assert!(spec.services.contains_key("app"));
}

// =========================================================================
// Restart policy (parsed but not mapped)
// =========================================================================

#[test]
fn test_restart_policy_parsed() {
    let yaml = r"
services:
  app:
    image: myapp:latest
    restart: unless-stopped
";
    let compose = parse_compose(yaml).unwrap();
    let app = &compose.services["app"];
    assert_eq!(app.restart.as_deref(), Some("unless-stopped"));

    let spec = compose_to_deployment(&compose, "restart-test").unwrap();
    assert!(spec.services.contains_key("app"));
}

// =========================================================================
// Top-level name and version
// =========================================================================

#[test]
fn test_compose_version_and_name() {
    let yaml = r#"
version: "3.8"
name: my-project

services:
  app:
    image: myapp:latest
"#;
    let compose = parse_compose(yaml).unwrap();
    assert_eq!(compose.version.as_deref(), Some("3.8"));
    assert_eq!(compose.name.as_deref(), Some("my-project"));
}

// =========================================================================
// Extension fields (x-*)
// =========================================================================

#[test]
fn test_extension_fields_parsed() {
    let yaml = r#"
x-labels: &labels
  com.example.managed-by: zlayer

x-version: "internal"

services:
  app:
    image: myapp:latest
    environment:
      LOG_LEVEL: info
      TZ: UTC
"#;
    let compose = parse_compose(yaml).unwrap();
    // Top-level extension fields should be captured in the extensions map.
    assert!(compose.extensions.contains_key("x-labels"));
    assert!(compose.extensions.contains_key("x-version"));

    let spec = compose_to_deployment(&compose, "ext-test").unwrap();
    let app = &spec.services["app"];
    assert_eq!(app.env.get("LOG_LEVEL").unwrap(), "info");
    assert_eq!(app.env.get("TZ").unwrap(), "UTC");
    assert_eq!(app.image.name, "myapp:latest");
}

// =========================================================================
// Miscellaneous service fields
// =========================================================================

#[test]
fn test_container_name_and_hostname() {
    let yaml = r"
services:
  app:
    image: myapp:latest
    container_name: my-custom-container
    hostname: my-host
    domainname: example.com
";
    let compose = parse_compose(yaml).unwrap();
    let app = &compose.services["app"];
    assert_eq!(app.container_name.as_deref(), Some("my-custom-container"));
    assert_eq!(app.hostname.as_deref(), Some("my-host"));
    assert_eq!(app.domainname.as_deref(), Some("example.com"));
}

#[test]
fn test_expose_ports() {
    let yaml = r#"
services:
  app:
    image: myapp:latest
    expose:
      - "3000"
      - 8080
"#;
    let compose = parse_compose(yaml).unwrap();
    let app = &compose.services["app"];
    assert_eq!(app.expose.len(), 2);
}

#[test]
fn test_network_mode() {
    let yaml = r"
services:
  app:
    image: myapp:latest
    network_mode: host
";
    let compose = parse_compose(yaml).unwrap();
    let app = &compose.services["app"];
    assert_eq!(app.network_mode.as_deref(), Some("host"));

    let spec = compose_to_deployment(&compose, "netmode-test").unwrap();
    assert!(spec.services.contains_key("app"));
}

#[test]
fn test_tty_and_stdin() {
    let yaml = r"
services:
  debug:
    image: busybox:latest
    tty: true
    stdin_open: true
";
    let compose = parse_compose(yaml).unwrap();
    let debug = &compose.services["debug"];
    assert!(debug.tty);
    assert!(debug.stdin_open);
}

#[test]
fn test_platform_and_pull_policy() {
    let yaml = r"
services:
  app:
    image: myapp:latest
    platform: linux/amd64
    pull_policy: always
";
    let compose = parse_compose(yaml).unwrap();
    let app = &compose.services["app"];
    assert_eq!(app.platform.as_deref(), Some("linux/amd64"));
    assert_eq!(app.pull_policy.as_deref(), Some("always"));
}

#[test]
fn test_read_only_filesystem() {
    let yaml = r"
services:
  app:
    image: myapp:latest
    read_only: true
";
    let compose = parse_compose(yaml).unwrap();
    let app = &compose.services["app"];
    assert!(app.read_only);
}

// =========================================================================
// Ulimits
// =========================================================================

#[test]
fn test_ulimits_parsing() {
    let yaml = r"
services:
  app:
    image: myapp:latest
    ulimits:
      nofile:
        soft: 20000
        hard: 40000
      nproc: 65535
";
    let compose = parse_compose(yaml).unwrap();
    let app = &compose.services["app"];
    assert_eq!(app.ulimits.len(), 2);

    let spec = compose_to_deployment(&compose, "ulimit-test").unwrap();
    assert!(spec.services.contains_key("app"));
}

// =========================================================================
// Logging
// =========================================================================

#[test]
fn test_logging_parsed() {
    let yaml = r#"
services:
  app:
    image: myapp:latest
    logging:
      driver: json-file
      options:
        max-size: "10m"
        max-file: "3"
"#;
    let compose = parse_compose(yaml).unwrap();
    let app = &compose.services["app"];
    let logging = app.logging.as_ref().unwrap();
    assert_eq!(logging.driver.as_deref(), Some("json-file"));
    assert_eq!(logging.options.get("max-size").unwrap(), "10m");
    assert_eq!(logging.options.get("max-file").unwrap(), "3");
}

// =========================================================================
// Full round-trip: YAML -> ComposeFile -> DeploymentSpec -> verify
// =========================================================================

#[test]
#[allow(clippy::too_many_lines)]
fn test_full_round_trip() {
    let yaml = r#"
version: "3.8"

services:
  nginx:
    image: nginx:alpine
    ports:
      - "80:80"
      - "443:443"
    volumes:
      - ./nginx.conf:/etc/nginx/nginx.conf:ro
    depends_on:
      api:
        condition: service_healthy
    restart: unless-stopped

  api:
    build:
      context: .
      dockerfile: Dockerfile
      args:
        NODE_ENV: production
    image: myapi:latest
    ports:
      - "3000:3000"
    environment:
      DATABASE_URL: "postgres://user:pass@db:5432/myapp"
      REDIS_URL: "redis://redis:6379"
    depends_on:
      db:
        condition: service_healthy
      redis:
        condition: service_started
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:3000/health"]
      interval: 30s
      timeout: 10s
      retries: 3
      start_period: 10s
    deploy:
      replicas: 2
      resources:
        limits:
          cpus: "1.0"
          memory: "512M"

  db:
    image: postgres:16-alpine
    environment:
      POSTGRES_USER: user
      POSTGRES_PASSWORD: pass
      POSTGRES_DB: myapp
    volumes:
      - pgdata:/var/lib/postgresql/data
    ports:
      - "5432:5432"
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U user"]
      interval: 10s
      timeout: 5s
      retries: 5

  redis:
    image: redis:7-alpine
    ports:
      - "6379:6379"
    volumes:
      - redis-data:/data

volumes:
  pgdata:
  redis-data:
"#;
    // Step 1: Parse YAML into ComposeFile.
    let compose = parse_compose(yaml).unwrap();
    assert_eq!(compose.services.len(), 4);
    assert_eq!(compose.volumes.len(), 2);
    assert!(compose.volumes.contains_key("pgdata"));
    assert!(compose.volumes.contains_key("redis-data"));

    // Step 2: Convert to DeploymentSpec.
    let spec = compose_to_deployment(&compose, "webapp").unwrap();
    assert_eq!(spec.version, "v1");
    assert_eq!(spec.deployment, "webapp");
    assert_eq!(spec.services.len(), 4);

    // Step 3: Verify nginx service.
    let nginx = &spec.services["nginx"];
    assert_eq!(nginx.image.name, "nginx:alpine");
    assert_eq!(nginx.endpoints.len(), 2);
    assert_eq!(nginx.storage.len(), 1);
    assert_eq!(nginx.depends.len(), 1);
    assert_eq!(nginx.depends[0].service, "api");
    assert_eq!(nginx.depends[0].condition, DependencyCondition::Healthy);
    match &nginx.storage[0] {
        StorageSpec::Bind {
            source,
            target,
            readonly,
        } => {
            assert_eq!(source, "./nginx.conf");
            assert_eq!(target, "/etc/nginx/nginx.conf");
            assert!(readonly);
        }
        other => panic!("expected Bind storage for nginx, got: {other:?}"),
    }

    // Step 4: Verify api service.
    let api = &spec.services["api"];
    assert_eq!(api.image.name, "myapi:latest");
    assert_eq!(api.endpoints.len(), 1);
    assert_eq!(api.endpoints[0].port, 3000);
    assert_eq!(api.endpoints[0].target_port, Some(3000));
    assert_eq!(api.endpoints[0].protocol, Protocol::Http);
    assert_eq!(
        api.env.get("DATABASE_URL").unwrap(),
        "postgres://user:pass@db:5432/myapp"
    );
    assert_eq!(api.env.get("REDIS_URL").unwrap(), "redis://redis:6379");
    assert_eq!(api.depends.len(), 2);
    let db_dep = api.depends.iter().find(|d| d.service == "db").unwrap();
    assert_eq!(db_dep.condition, DependencyCondition::Healthy);
    let redis_dep = api.depends.iter().find(|d| d.service == "redis").unwrap();
    assert_eq!(redis_dep.condition, DependencyCondition::Started);
    assert_eq!(api.scale, ScaleSpec::Fixed { replicas: 2 });
    assert_eq!(api.resources.cpu, Some(1.0));
    assert_eq!(api.resources.memory, Some("512M".to_string()));
    match &api.health.check {
        HealthCheck::Command { command } => {
            assert_eq!(command, "curl -f http://localhost:3000/health");
        }
        other => panic!("expected Command health check for api, got: {other:?}"),
    }
    assert_eq!(api.health.retries, 3);
    assert_eq!(api.health.interval, Some(Duration::from_secs(30)));
    assert_eq!(api.health.timeout, Some(Duration::from_secs(10)));
    assert_eq!(api.health.start_grace, Some(Duration::from_secs(10)));

    // Step 5: Verify db service.
    let db = &spec.services["db"];
    assert_eq!(db.image.name, "postgres:16-alpine");
    assert_eq!(db.env.get("POSTGRES_USER").unwrap(), "user");
    assert_eq!(db.env.get("POSTGRES_PASSWORD").unwrap(), "pass");
    assert_eq!(db.env.get("POSTGRES_DB").unwrap(), "myapp");
    assert_eq!(db.endpoints.len(), 1);
    assert_eq!(db.endpoints[0].port, 5432);
    assert_eq!(db.storage.len(), 1);
    match &db.storage[0] {
        StorageSpec::Named { name, target, .. } => {
            assert_eq!(name, "pgdata");
            assert_eq!(target, "/var/lib/postgresql/data");
        }
        other => panic!("expected Named storage for db, got: {other:?}"),
    }
    match &db.health.check {
        HealthCheck::Command { command } => {
            assert_eq!(command, "pg_isready -U user");
        }
        other => panic!("expected Command health check for db, got: {other:?}"),
    }
    assert_eq!(db.health.retries, 5);
    assert_eq!(db.health.interval, Some(Duration::from_secs(10)));
    assert_eq!(db.health.timeout, Some(Duration::from_secs(5)));

    // Step 6: Verify redis service.
    let redis = &spec.services["redis"];
    assert_eq!(redis.image.name, "redis:7-alpine");
    assert_eq!(redis.endpoints.len(), 1);
    assert_eq!(redis.endpoints[0].port, 6379);
    assert_eq!(redis.endpoints[0].protocol, Protocol::Tcp);
    assert_eq!(redis.storage.len(), 1);
    match &redis.storage[0] {
        StorageSpec::Named { name, target, .. } => {
            assert_eq!(name, "redis-data");
            assert_eq!(target, "/data");
        }
        other => panic!("expected Named storage for redis, got: {other:?}"),
    }
}

// =========================================================================
// Multi-service full compose file
// =========================================================================

#[test]
fn test_multi_service_full_compose() {
    let yaml = r#"
version: "3.8"
name: fullstack

services:
  frontend:
    image: node:20-alpine
    ports:
      - "3000:3000"
    environment:
      - NODE_ENV=production
      - API_URL=http://backend:4000
    depends_on:
      - backend
    profiles:
      - production

  backend:
    image: python:3.12-slim
    ports:
      - "4000:4000"
    environment:
      DB_HOST: db
      DB_PORT: 5432
    depends_on:
      db:
        condition: service_healthy
    deploy:
      replicas: 2
      resources:
        limits:
          cpus: "2.0"
          memory: "1G"

  db:
    image: postgres:16-alpine
    ports:
      - "5432:5432"
    environment:
      POSTGRES_PASSWORD: secret
    volumes:
      - pgdata:/var/lib/postgresql/data
    healthcheck:
      test: ["CMD-SHELL", "pg_isready"]
      interval: 5s
      timeout: 3s
      retries: 10

  worker:
    image: python:3.12-slim
    command: ["python", "-m", "celery", "worker"]
    depends_on:
      - backend
      - db
    deploy:
      replicas: 4

volumes:
  pgdata:
    driver: local
"#;
    let compose = parse_compose(yaml).unwrap();
    assert_eq!(compose.services.len(), 4);
    assert_eq!(compose.name.as_deref(), Some("fullstack"));

    let spec = compose_to_deployment(&compose, "fullstack").unwrap();
    assert_eq!(spec.services.len(), 4);

    // frontend
    let frontend = &spec.services["frontend"];
    assert_eq!(frontend.image.name, "node:20-alpine");
    assert_eq!(frontend.env.get("NODE_ENV").unwrap(), "production");
    assert_eq!(frontend.env.get("API_URL").unwrap(), "http://backend:4000");
    assert_eq!(frontend.depends.len(), 1);
    assert_eq!(frontend.depends[0].service, "backend");
    assert_eq!(frontend.depends[0].condition, DependencyCondition::Started);

    // backend
    let backend = &spec.services["backend"];
    assert_eq!(backend.scale, ScaleSpec::Fixed { replicas: 2 });
    assert_eq!(backend.resources.cpu, Some(2.0));
    assert_eq!(backend.resources.memory, Some("1G".to_string()));
    assert_eq!(backend.depends.len(), 1);
    assert_eq!(backend.depends[0].service, "db");
    assert_eq!(backend.depends[0].condition, DependencyCondition::Healthy);

    // worker
    let worker = &spec.services["worker"];
    assert_eq!(worker.scale, ScaleSpec::Fixed { replicas: 4 });
    assert_eq!(
        worker.command.args,
        Some(vec![
            "python".to_string(),
            "-m".to_string(),
            "celery".to_string(),
            "worker".to_string(),
        ])
    );
    assert_eq!(worker.depends.len(), 2);
}
