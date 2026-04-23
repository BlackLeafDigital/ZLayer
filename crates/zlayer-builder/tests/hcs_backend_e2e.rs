//! End-to-end integration test for the HCS-backed Windows builder.
//!
//! This test only compiles and runs on Windows hosts. Most assertions are
//! `#[ignore]`'d by default because they require:
//!   - A live Windows host with HCS available
//!   - Network access to `mcr.microsoft.com` for the base image pull
//!   - `SeBackupPrivilege` + `SeRestorePrivilege` on the running token
//!     (typically only granted to elevated processes)
//!
//! Run them locally on a Windows machine with `cargo test -p zlayer-builder
//! --test hcs_backend_e2e -- --ignored` once those preconditions are met.

#![cfg(target_os = "windows")]

use zlayer_builder::backend::hcs::{
    build_image_config_bytes, build_manifest_bytes, ImageConfigBuilder,
    OCI_IMAGE_CONFIG_MEDIA_TYPE, OCI_IMAGE_MANIFEST_MEDIA_TYPE, OCI_WINDOWS_LAYER_MEDIA_TYPE,
};

#[test]
fn image_config_serializes_with_windows_amd64_metadata() {
    let mut config = ImageConfigBuilder::new();
    config.set_working_dir("C:\\app");
    config.push_env("FOO", "bar");
    config.set_os_version(Some("10.0.20348.2700".to_string()));

    let bytes = build_image_config_bytes(&config, &[], "sha256:0123456789abcdef")
        .expect("serialize image config");
    let parsed: serde_json::Value =
        serde_json::from_slice(&bytes).expect("re-parse image config JSON");

    assert_eq!(parsed["os"], "windows");
    assert_eq!(parsed["architecture"], "amd64");
    assert_eq!(parsed["os.version"], "10.0.20348.2700");
    assert_eq!(parsed["config"]["WorkingDir"], "C:\\app");
    assert_eq!(parsed["config"]["Env"][0], "FOO=bar");
    assert_eq!(parsed["rootfs"]["type"], "layers");
    assert_eq!(parsed["rootfs"]["diff_ids"][0], "sha256:0123456789abcdef");
}

#[test]
fn manifest_uses_oci_image_manifest_v1_media_types() {
    let bytes = build_manifest_bytes("sha256:cfg", 256, &[], "sha256:layer", 1024)
        .expect("serialize manifest");
    let parsed: serde_json::Value = serde_json::from_slice(&bytes).expect("re-parse manifest");

    assert_eq!(parsed["schemaVersion"], 2);
    assert_eq!(parsed["mediaType"], OCI_IMAGE_MANIFEST_MEDIA_TYPE);
    assert_eq!(parsed["config"]["mediaType"], OCI_IMAGE_CONFIG_MEDIA_TYPE);
    assert_eq!(
        parsed["layers"][0]["mediaType"],
        OCI_WINDOWS_LAYER_MEDIA_TYPE
    );
    assert_eq!(parsed["layers"][0]["digest"], "sha256:layer");
    assert_eq!(parsed["layers"][0]["size"], 1024);
}

/// Full end-to-end build against a real Nanoserver base. Requires a Windows
/// host with HCS, elevated privileges, and network access to MCR.
#[tokio::test]
#[ignore = "requires Windows host with HCS, elevated privileges, and MCR access"]
async fn build_trivial_dockerfile_against_nanoserver() {
    use zlayer_builder::backend::hcs::HcsBackend;
    use zlayer_builder::backend::BuildBackend;
    use zlayer_builder::{BuildOptions, Dockerfile};

    let tmp = tempfile::tempdir().expect("tmpdir for build context");
    let context = tmp.path();

    // Drop a fake "executable" into the context so COPY has something to do.
    std::fs::write(context.join("hello.exe"), b"MZ\x00\x00fake-pe").unwrap();

    let dockerfile = Dockerfile::parse(
        r#"
FROM mcr.microsoft.com/windows/nanoserver:ltsc2022
COPY hello.exe C:\hello.exe
ENV ZLAYER_BUILT_BY=hcs-builder
WORKDIR C:\app
CMD ["C:\\hello.exe"]
"#,
    )
    .expect("parse Dockerfile");

    let storage = std::env::temp_dir().join("zlayer-hcs-e2e");
    let backend = HcsBackend::with_storage_root(storage)
        .await
        .expect("construct HCS backend");

    let opts = BuildOptions {
        tags: vec!["zlayer-windows-e2e:latest".to_string()],
        ..BuildOptions::default()
    };

    let result = backend
        .build_image(context, &dockerfile, &opts, None)
        .await
        .expect("HCS build to succeed");

    assert!(result.image_id.starts_with("sha256:"));
    assert!(result.layer_count >= 2, "expected base + new layer");
    assert!(!result.tags.is_empty());
}
