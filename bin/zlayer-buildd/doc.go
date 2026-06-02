// Package main implements zlayer-buildd, the buildah sidecar daemon.
//
// zlayer-buildd is a small Go binary that wraps
// `imagebuildah.BuildDockerfiles` behind a gRPC API. It exists because there
// is no Rust-native equivalent of buildah; we shell to a typed gRPC service
// instead of parsing buildah-CLI argv.
//
// Transport is TCP with mTLS by default, bound to 127.0.0.1:<auto-port>. The
// Rust `BuildahSidecarBackend` (under `crates/zlayer-builder/src/backend/
// buildah_sidecar/`) spawns this binary on demand, reads its `LISTENING
// host:port` handshake line from stdout, and dials in.
//
// See `/proto/buildah_sidecar.proto` at the workspace root for the wire
// schema.
package main
