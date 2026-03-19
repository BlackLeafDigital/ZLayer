#![cfg(target_os = "macos")]
//! End-to-end integration tests for the macOS Seatbelt sandbox image builder.
//!
//! These tests verify the `SandboxImageBuilder` and the underlying registry
//! pull pipeline on macOS. They require network access to pull images from
//! Docker Hub.
//!
//! # Running
//!
//! ```bash
//! cargo test --package zlayer-builder-zql --test sandbox_build_e2e --features cache -- --nocapture
//! ```

use std::path::PathBuf;

use zlayer_builder_zql::sandbox_builder::{SandboxImageBuilder, SandboxImageConfig};
use zlayer_builder_zql::Dockerfile;
use zlayer_registry::{BlobCache, ImagePuller, RegistryAuth};

/// Helper: generate a unique temp directory name for test isolation.
fn test_data_dir(prefix: &str) -> PathBuf {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let dir = std::env::temp_dir().join(format!("zlayer-sandbox-e2e-{prefix}-{ts}"));
    std::fs::create_dir_all(&dir).expect("failed to create test data dir");
    dir
}

// =============================================================================
// Test 1: Pull alpine manifest and verify layers
// =============================================================================

/// Pull the OCI manifest for `alpine:3.20` and verify it has at least one layer.
///
/// This validates the platform resolver, which must select the correct
/// architecture manifest from the multi-arch index.
#[tokio::test]
async fn test_pull_alpine_manifest() {
    let cache = BlobCache::new().expect("failed to create blob cache");
    let puller = ImagePuller::new(cache);
    let auth = RegistryAuth::Anonymous;

    let (manifest, digest) = puller
        .pull_manifest("docker.io/library/alpine:3.20", &auth)
        .await
        .expect("failed to pull alpine:3.20 manifest");

    assert!(
        !manifest.layers.is_empty(),
        "alpine:3.20 manifest should have at least 1 layer"
    );
    assert!(!digest.is_empty(), "manifest digest should be non-empty");

    // Alpine is a tiny image -- expect exactly 1 layer (single rootfs tarball)
    println!(
        "alpine:3.20 manifest: {} layer(s), digest={}",
        manifest.layers.len(),
        digest
    );
}

// =============================================================================
// Test 2: Pull alpine layers
// =============================================================================

/// Pull the full image (manifest + layers) for `alpine:3.20` and verify the
/// layer data is non-empty.
#[tokio::test]
async fn test_pull_alpine_layers() {
    let cache = BlobCache::new().expect("failed to create blob cache");
    let puller = ImagePuller::new(cache);
    let auth = RegistryAuth::Anonymous;

    let layers = puller
        .pull_image("docker.io/library/alpine:3.20", &auth)
        .await
        .expect("failed to pull alpine:3.20 image");

    assert!(
        !layers.is_empty(),
        "alpine:3.20 should have at least 1 layer"
    );

    for (i, (data, media_type)) in layers.iter().enumerate() {
        assert!(!data.is_empty(), "layer {i} data should be non-empty");
        assert!(
            !media_type.is_empty(),
            "layer {i} media type should be non-empty"
        );
        println!("layer {i}: {} bytes, media_type={media_type}", data.len());
    }
}

// =============================================================================
// Test 3: Sandbox build with a simple Dockerfile
// =============================================================================

/// Build a minimal Dockerfile using `SandboxImageBuilder` and verify the
/// output rootfs and config.json are created.
#[tokio::test]
async fn test_sandbox_build_simple() {
    let tmp = tempfile::TempDir::new().expect("failed to create temp dir");
    let context_dir = tmp.path().join("context");
    std::fs::create_dir_all(&context_dir).expect("failed to create context dir");

    // Write a minimal Dockerfile.
    // Use relative path because sandbox builder runs on host (no chroot).
    // cwd is set to rootfs, so `> test.txt` writes to rootfs/test.txt.
    let dockerfile_content =
        "FROM alpine:3.20\nRUN echo hello > test.txt\nCMD [\"cat\", \"test.txt\"]";
    std::fs::write(context_dir.join("Dockerfile"), dockerfile_content)
        .expect("failed to write Dockerfile");

    let data_dir = test_data_dir("simple");

    let builder = SandboxImageBuilder::new(context_dir.clone(), data_dir.clone());

    let dockerfile = Dockerfile::parse(dockerfile_content).expect("failed to parse Dockerfile");

    let tags = vec!["test-simple:latest".to_string()];
    let result = builder
        .build(&dockerfile, &tags)
        .await
        .expect("sandbox build failed");

    // Verify rootfs directory was created and has content
    assert!(
        result.rootfs_dir.exists(),
        "rootfs directory should exist at {}",
        result.rootfs_dir.display()
    );
    let rootfs_entries: Vec<_> = std::fs::read_dir(&result.rootfs_dir)
        .expect("failed to read rootfs dir")
        .collect();
    assert!(
        !rootfs_entries.is_empty(),
        "rootfs should contain files (got 0 entries)"
    );

    // Verify config.json was created
    assert!(
        result.config_path.exists(),
        "config.json should exist at {}",
        result.config_path.display()
    );
    let config_raw =
        std::fs::read_to_string(&result.config_path).expect("failed to read config.json");
    let config: SandboxImageConfig =
        serde_json::from_str(&config_raw).expect("config.json should be valid JSON");

    // CMD should be set
    assert_eq!(
        config.cmd,
        Some(vec!["cat".to_string(), "test.txt".to_string()]),
        "CMD should match Dockerfile"
    );

    // Verify the RUN instruction wrote test.txt into the rootfs
    let test_file = result.rootfs_dir.join("test.txt");
    assert!(
        test_file.exists(),
        "test.txt should exist in rootfs at {}",
        test_file.display()
    );

    println!(
        "Build completed in {}ms, image_id={}, rootfs={}",
        result.build_time_ms,
        result.image_id,
        result.rootfs_dir.display()
    );

    // Cleanup
    let _ = std::fs::remove_dir_all(&data_dir);
}

// =============================================================================
// Test 4: Sandbox build with ENV and WORKDIR
// =============================================================================

/// Build a Dockerfile with `ENV` and `WORKDIR` instructions and verify the
/// resulting `config.json` captures them correctly.
#[tokio::test]
async fn test_sandbox_build_env_workdir() {
    let tmp = tempfile::TempDir::new().expect("failed to create temp dir");
    let context_dir = tmp.path().join("context");
    std::fs::create_dir_all(&context_dir).expect("failed to create context dir");

    let dockerfile_content = "\
FROM alpine:3.20
ENV APP_NAME=myapp
ENV APP_VERSION=1.0.0
WORKDIR /opt/myapp
RUN echo \"setup done\"
CMD [\"echo\", \"hello\"]
";
    std::fs::write(context_dir.join("Dockerfile"), dockerfile_content)
        .expect("failed to write Dockerfile");

    let data_dir = test_data_dir("env-workdir");

    let builder = SandboxImageBuilder::new(context_dir.clone(), data_dir.clone());

    let dockerfile = Dockerfile::parse(dockerfile_content).expect("failed to parse Dockerfile");

    let tags = vec!["test-env-workdir:latest".to_string()];
    let result = builder
        .build(&dockerfile, &tags)
        .await
        .expect("sandbox build failed");

    // Read and parse config.json
    let config_raw =
        std::fs::read_to_string(&result.config_path).expect("failed to read config.json");
    let config: SandboxImageConfig =
        serde_json::from_str(&config_raw).expect("config.json should be valid JSON");

    // Verify ENV variables are present
    assert!(
        config.env.contains(&"APP_NAME=myapp".to_string()),
        "config.env should contain APP_NAME=myapp, got: {:?}",
        config.env
    );
    assert!(
        config.env.contains(&"APP_VERSION=1.0.0".to_string()),
        "config.env should contain APP_VERSION=1.0.0, got: {:?}",
        config.env
    );

    // Verify WORKDIR
    assert_eq!(
        config.working_dir, "/opt/myapp",
        "working_dir should be /opt/myapp"
    );

    // Verify the workdir was actually created in the rootfs
    let workdir_in_rootfs = result.rootfs_dir.join("opt/myapp");
    assert!(
        workdir_in_rootfs.exists(),
        "WORKDIR /opt/myapp should exist in rootfs at {}",
        workdir_in_rootfs.display()
    );

    // Verify CMD
    assert_eq!(
        config.cmd,
        Some(vec!["echo".to_string(), "hello".to_string()]),
        "CMD should match Dockerfile"
    );

    println!(
        "Build completed in {}ms, env={:?}, workdir={}",
        result.build_time_ms, config.env, config.working_dir
    );

    // Cleanup
    let _ = std::fs::remove_dir_all(&data_dir);
}
