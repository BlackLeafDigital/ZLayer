//! Build script for zlayer-builder.
//!
//! Compiles the buildah-sidecar gRPC schema at `proto/buildah_sidecar.proto`
//! (workspace root) into `${OUT_DIR}/zlayer.buildah_sidecar.v1.rs`, which
//! the crate `include!`s from `backend/buildah_sidecar/mod.rs`.

use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    // crates/zlayer-builder/ → workspace root is two levels up.
    let workspace_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .ok_or("could not locate workspace root from CARGO_MANIFEST_DIR")?;

    let proto_path = workspace_root.join("proto").join("buildah_sidecar.proto");
    let proto_dir = workspace_root.join("proto");

    println!("cargo:rerun-if-changed={}", proto_path.display());

    // tonic-build 0.14 split prost code generation into `tonic-prost-build`.
    // The runtime crate `tonic-build` now only exposes the trait-based
    // codegen plumbing; the high-level `configure()` builder lives in
    // `tonic-prost-build`.
    tonic_prost_build::configure()
        .build_server(false) // Rust side is a client only.
        .build_client(true)
        .compile_protos(&[proto_path], &[proto_dir])?;

    Ok(())
}
