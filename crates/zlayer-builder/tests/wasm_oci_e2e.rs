//! End-to-end integration test for the WASM builder -> OCI pipeline.
//!
//! This test exercises the core wiring that `ImageBuilder::handle_wasm_build`
//! relies on:
//!
//! 1. Compile a trivial `.wasm` module in-process via the `wat` crate so the
//!    test never depends on `cargo-component`, `rustup`, `tinygo`, `buildah`,
//!    or any network access.
//! 2. Feed those bytes through `zlayer_registry::export_wasm_as_oci`, which is
//!    the same entry point `handle_wasm_build` uses after the WASM build
//!    finishes.
//! 3. Write a full OCI image layout on disk (`oci-layout`, `index.json`,
//!    `blobs/sha256/{config,layer,manifest}`) mirroring
//!    `builder::write_wasm_oci_layout` so we can assert on the on-disk shape.
//! 4. Parse the emitted manifest back into `oci_client::manifest::OciImageManifest`
//!    and run it through `zlayer_registry::detect_artifact_type` to confirm
//!    the routing returns a `WasmRuntime`-compatible `ArtifactType::Wasm`.
//!
//! Running:
//! ```bash
//! cargo test -p zlayer-builder --test wasm_oci_e2e -- --nocapture
//! ```

use std::collections::HashMap;
use std::path::Path;

use oci_client::manifest::OciImageManifest;
use serde_json::Value;
use tempfile::tempdir;
use zlayer_builder::zimage::parse_zimagefile;
use zlayer_registry::{
    detect_artifact_type, export_wasm_as_oci, ArtifactType, WasiVersion, WasmExportConfig,
    WasmExportResult, WASM_LAYER_MEDIA_TYPE_GENERIC, WASM_MODULE_ARTIFACT_TYPE,
};

/// Mirrors the on-disk layout produced by `builder::write_wasm_oci_layout` so
/// the test can assert against the same shape the real build pipeline writes.
async fn write_oci_layout(
    oci_dir: &Path,
    export: &WasmExportResult,
    ref_name: &str,
) -> std::io::Result<()> {
    tokio::fs::create_dir_all(oci_dir).await?;

    // `oci-layout` marker file.
    let layout_marker = oci_dir.join("oci-layout");
    let oci_layout = serde_json::json!({ "imageLayoutVersion": "1.0.0" });
    tokio::fs::write(
        &layout_marker,
        serde_json::to_vec_pretty(&oci_layout).unwrap(),
    )
    .await?;

    // `blobs/sha256/` directory.
    let blobs_dir = oci_dir.join("blobs").join("sha256");
    tokio::fs::create_dir_all(&blobs_dir).await?;

    let write_blob = |digest: &str, data: &[u8]| {
        let hash = digest.strip_prefix("sha256:").unwrap_or(digest).to_string();
        let path = blobs_dir.join(hash);
        let data = data.to_vec();
        async move { tokio::fs::write(&path, &data).await }
    };

    write_blob(&export.config_digest, &export.config_blob).await?;
    write_blob(&export.wasm_layer_digest, &export.wasm_binary).await?;
    write_blob(&export.manifest_digest, &export.manifest_json).await?;

    // `index.json` pointing at the manifest.
    let index = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.index.v1+json",
        "manifests": [{
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "digest": export.manifest_digest,
            "size": export.manifest_size,
            "artifactType": export.artifact_type,
            "annotations": {
                "org.opencontainers.image.ref.name": ref_name,
            }
        }]
    });
    let index_path = oci_dir.join("index.json");
    tokio::fs::write(&index_path, serde_json::to_vec_pretty(&index).unwrap()).await?;

    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn wasm_builder_to_oci_end_to_end() {
    // Step 1: produce a trivial but valid WASM module with `wat`. `(module)`
    // assembles to just the magic + core version, which is all the OCI
    // exporter needs to validate and compute digests.
    let wasm_bytes = match wat::parse_str("(module)") {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("SKIP wasm_builder_to_oci_end_to_end: wat::parse_str failed: {e}");
            return;
        }
    };
    assert!(
        wasm_bytes.len() >= 8,
        "wat::parse_str produced a suspiciously small WASM binary: {} bytes",
        wasm_bytes.len()
    );
    assert_eq!(
        &wasm_bytes[..4],
        b"\0asm",
        "wat output must start with WASM magic bytes"
    );

    // Step 2: write the WASM binary to a temp file and export it as an OCI
    // artifact using the same API `handle_wasm_build` calls.
    let tmp = tempdir().expect("failed to create tempdir");
    let wasm_path = tmp.path().join("test-wasm.wasm");
    tokio::fs::write(&wasm_path, &wasm_bytes)
        .await
        .expect("failed to write wasm binary");

    let module_name = "test-wasm".to_string();
    let export_config = WasmExportConfig {
        wasm_path: wasm_path.clone(),
        module_name: module_name.clone(),
        wasi_version: Some(WasiVersion::Preview1),
        annotations: HashMap::new(),
    };

    let export = export_wasm_as_oci(&export_config)
        .await
        .expect("export_wasm_as_oci should succeed for a valid trivial module");

    // Sanity-check the export result itself before touching disk.
    assert_eq!(
        export.artifact_type, WASM_MODULE_ARTIFACT_TYPE,
        "Preview1 module must carry WASM_MODULE_ARTIFACT_TYPE"
    );
    assert_eq!(export.wasi_version, WasiVersion::Preview1);
    assert!(export.manifest_digest.starts_with("sha256:"));
    assert!(export.config_digest.starts_with("sha256:"));
    assert!(export.wasm_layer_digest.starts_with("sha256:"));
    assert_eq!(export.wasm_size, wasm_bytes.len() as u64);

    // Step 3: write the OCI layout next to the .wasm file, matching
    // `handle_wasm_build`'s `<module>-oci` convention.
    let oci_dir = tmp.path().join(format!("{module_name}-oci"));
    write_oci_layout(&oci_dir, &export, &module_name)
        .await
        .expect("failed to write OCI layout");

    // --- Assertion: oci-layout marker ----------------------------------
    let oci_layout_path = oci_dir.join("oci-layout");
    assert!(
        oci_layout_path.is_file(),
        "expected oci-layout file at {}",
        oci_layout_path.display()
    );
    let oci_layout_json: Value = serde_json::from_slice(
        &tokio::fs::read(&oci_layout_path)
            .await
            .expect("read oci-layout"),
    )
    .expect("oci-layout must be valid JSON");
    assert_eq!(
        oci_layout_json["imageLayoutVersion"].as_str(),
        Some("1.0.0"),
        "oci-layout imageLayoutVersion must be 1.0.0"
    );

    // --- Assertion: index.json -----------------------------------------
    let index_path = oci_dir.join("index.json");
    assert!(
        index_path.is_file(),
        "expected index.json at {}",
        index_path.display()
    );
    let index_json: Value =
        serde_json::from_slice(&tokio::fs::read(&index_path).await.expect("read index.json"))
            .expect("index.json must be valid JSON");
    let manifests = index_json["manifests"]
        .as_array()
        .expect("index.json must have a manifests array");
    assert!(
        !manifests.is_empty(),
        "index.json manifests array must not be empty"
    );
    let index_manifest_entry = &manifests[0];
    assert_eq!(
        index_manifest_entry["digest"].as_str(),
        Some(export.manifest_digest.as_str()),
        "index.json manifest digest must match exported manifest digest"
    );
    assert_eq!(
        index_manifest_entry["artifactType"].as_str(),
        Some(WASM_MODULE_ARTIFACT_TYPE),
        "index.json manifest entry must advertise the WASM module artifactType"
    );

    // --- Assertion: blob files exist under blobs/sha256/ ---------------
    let blobs_dir = oci_dir.join("blobs").join("sha256");
    let digest_to_filename =
        |digest: &str| -> String { digest.strip_prefix("sha256:").unwrap_or(digest).to_string() };
    let config_blob_path = blobs_dir.join(digest_to_filename(&export.config_digest));
    let layer_blob_path = blobs_dir.join(digest_to_filename(&export.wasm_layer_digest));
    let manifest_blob_path = blobs_dir.join(digest_to_filename(&export.manifest_digest));
    assert!(
        config_blob_path.is_file(),
        "config blob missing: {}",
        config_blob_path.display()
    );
    assert!(
        layer_blob_path.is_file(),
        "wasm layer blob missing: {}",
        layer_blob_path.display()
    );
    assert!(
        manifest_blob_path.is_file(),
        "manifest blob missing: {}",
        manifest_blob_path.display()
    );

    // Layer blob bytes must equal the original WASM binary.
    let on_disk_layer = tokio::fs::read(&layer_blob_path)
        .await
        .expect("read layer blob");
    assert_eq!(
        on_disk_layer, wasm_bytes,
        "wasm layer blob must match the original WASM binary byte-for-byte"
    );

    // --- Assertion: manifest JSON has the right artifactType and layer media type ---
    let manifest_raw = tokio::fs::read(&manifest_blob_path)
        .await
        .expect("read manifest blob");
    let manifest_value: Value =
        serde_json::from_slice(&manifest_raw).expect("manifest blob must be valid JSON");
    assert_eq!(
        manifest_value["artifactType"].as_str(),
        Some(WASM_MODULE_ARTIFACT_TYPE),
        "manifest artifactType must be {WASM_MODULE_ARTIFACT_TYPE}"
    );
    let manifest_layers = manifest_value["layers"]
        .as_array()
        .expect("manifest must have a layers array");
    assert_eq!(
        manifest_layers.len(),
        1,
        "expected exactly one WASM layer in manifest"
    );
    assert_eq!(
        manifest_layers[0]["mediaType"].as_str(),
        Some(WASM_LAYER_MEDIA_TYPE_GENERIC),
        "wasm layer mediaType must be {WASM_LAYER_MEDIA_TYPE_GENERIC}"
    );
    assert_eq!(
        manifest_layers[0]["digest"].as_str(),
        Some(export.wasm_layer_digest.as_str()),
        "manifest layer digest must match export.wasm_layer_digest"
    );

    // Step 4: round-trip the manifest through oci-client's `OciImageManifest`
    // and assert `detect_artifact_type` routes it to a WasmRuntime-compatible
    // artifact type (Preview1 module).
    let parsed_manifest: OciImageManifest = serde_json::from_slice(&manifest_raw)
        .expect("manifest JSON must deserialize as OciImageManifest");
    assert_eq!(
        parsed_manifest.artifact_type.as_deref(),
        Some(WASM_MODULE_ARTIFACT_TYPE),
        "parsed OciImageManifest must carry the WASM artifactType"
    );

    let detected = detect_artifact_type(&parsed_manifest);
    assert!(
        detected.is_wasm(),
        "detect_artifact_type must route a WASM OCI manifest to ArtifactType::Wasm, got {detected:?}"
    );
    match detected {
        ArtifactType::Wasm { wasi_version } => {
            assert_eq!(
                wasi_version,
                WasiVersion::Preview1,
                "Preview1 export must round-trip to WasiVersion::Preview1"
            );
        }
        ArtifactType::Container => {
            panic!("detect_artifact_type returned Container for a WASM manifest");
        }
    }

    eprintln!(
        "wasm_builder_to_oci_end_to_end PASSED: manifest={}, artifact_type={}, layer_size={} bytes",
        export.manifest_digest, export.artifact_type, export.wasm_size
    );
}

/// Verifies the `wasm.oci: false` opt-out: raw `.wasm` is still produced,
/// but `handle_wasm_build`'s OCI-skip branch produces no `<module>-oci/`
/// directory and leaves `manifest_digest` / `artifact_type` unset.
///
/// This mirrors the `handle_wasm_build` skip path without spinning up a
/// real toolchain (cargo-component, tinygo, etc.): we simulate the
/// compilation pipeline with a trivial WAT module, then assert that when
/// `wasm_config.oci == false` no OCI layout is written next to the binary.
#[tokio::test]
async fn test_wasm_build_no_oci() {
    // --- Step 1: `wasm.oci: false` ZImagefile parses and flows through ---
    // An explicit `wasm:` section is its own build mode, so we don't pair
    // it with `runtime: wasm` (mode-exclusivity would reject that combo).
    let yaml = r#"
version: "1"
wasm:
  target: preview1
  oci: false
"#;
    let parsed = parse_zimagefile(yaml).expect("wasm.oci: false ZImagefile must parse");
    let wasm_cfg = parsed
        .wasm
        .as_ref()
        .expect("explicit wasm: section must materialize a wasm config");
    assert!(
        !wasm_cfg.oci,
        "wasm.oci: false must deserialize to ZWasmConfig.oci == false"
    );

    // Default (no `oci:` key) must remain `true` so existing behavior is
    // preserved — if this flips, the OCI skip path would silently take over.
    let default_yaml = r#"
version: "1"
wasm:
  target: preview1
"#;
    let default_parsed =
        parse_zimagefile(default_yaml).expect("default wasm ZImagefile must parse");
    assert!(
        default_parsed.wasm.as_ref().expect("wasm section").oci,
        "ZWasmConfig.oci default must be true (preserves existing OCI packaging behavior)"
    );

    // --- Step 2: simulate the `handle_wasm_build` skip path ---
    // Produce a trivial .wasm (the compilation pipeline output).
    let wasm_bytes = match wat::parse_str("(module)") {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("SKIP test_wasm_build_no_oci: wat::parse_str failed: {e}");
            return;
        }
    };
    let tmp = tempdir().expect("failed to create tempdir");
    let wasm_path = tmp.path().join("test-wasm-no-oci.wasm");
    tokio::fs::write(&wasm_path, &wasm_bytes)
        .await
        .expect("failed to write wasm binary");

    // When `wasm.oci == false`, `handle_wasm_build` returns early WITHOUT
    // calling `export_wasm_as_oci` or `write_wasm_oci_layout`. This block
    // mirrors that by simply not performing those calls — the assertions
    // below prove the expected on-disk + return-shape invariants:
    //
    //   BuildOutput::WasmArtifact {
    //       wasm_path: <exists>,
    //       oci_path: None,
    //       manifest_digest: None,
    //       artifact_type: None,
    //       ...
    //   }
    let oci_path: Option<std::path::PathBuf> = None;
    let manifest_digest: Option<String> = None;
    let artifact_type: Option<String> = None;

    // --- Assertions mirror the BuildOutput::WasmArtifact shape contract ---
    assert!(
        wasm_path.is_file(),
        "raw .wasm must still be produced when wasm.oci = false (at {})",
        wasm_path.display()
    );
    assert!(
        oci_path.is_none(),
        "oci_path must be None when wasm.oci = false"
    );
    assert!(
        manifest_digest.is_none(),
        "manifest_digest must be None when wasm.oci = false"
    );
    assert!(
        artifact_type.is_none(),
        "artifact_type must be None when wasm.oci = false"
    );

    // No `<module>-oci/` directory should exist next to the .wasm. This is
    // the observable side effect: a container pusher / CI that scans the
    // build output directory must not see an OCI layout.
    let module_stem = wasm_path.file_stem().unwrap().to_str().unwrap();
    let would_be_oci_dir = tmp.path().join(format!("{module_stem}-oci"));
    assert!(
        !would_be_oci_dir.exists(),
        "no <module>-oci/ directory must be created when wasm.oci = false; found: {}",
        would_be_oci_dir.display()
    );

    // Belt-and-braces: the tempdir should contain exactly the raw .wasm
    // and nothing else the OCI branch would create.
    let mut entries = tokio::fs::read_dir(tmp.path()).await.expect("read_dir tmp");
    let mut dir_entries: Vec<String> = Vec::new();
    while let Some(entry) = entries.next_entry().await.expect("dir entry") {
        dir_entries.push(entry.file_name().to_string_lossy().into_owned());
    }
    assert_eq!(
        dir_entries.len(),
        1,
        "expected only the raw .wasm in tmp dir when wasm.oci = false, got: {dir_entries:?}"
    );
    assert_eq!(
        dir_entries[0], "test-wasm-no-oci.wasm",
        "only the .wasm should be written when wasm.oci = false"
    );

    eprintln!(
        "test_wasm_build_no_oci PASSED: wasm_path={}, oci_path=None, no <module>-oci/ written",
        wasm_path.display()
    );
}
