//! Thin tonic-client wrappers for the sidecar's post-build RPCs
//! (`push` / `tag` / `manifest_create` / `manifest_add` /
//! `manifest_push`).
//!
//! These mirror the `BuildBackend` trait's same-named methods and
//! delegate to the gRPC channel managed by
//! [`super::SidecarLifecycle`]. The translation is intentionally
//! minimal — the wire protocol is the contract; the sidecar handles
//! all buildah-level work.

use super::proto;
use super::BuildahSidecarBackend;
use crate::builder::RegistryAuth;
use crate::error::{BuildError, Result};

impl BuildahSidecarBackend {
    /// gRPC equivalent of `buildah push`.
    pub(super) async fn push_image_impl(
        &self,
        tag: &str,
        auth: Option<&RegistryAuth>,
    ) -> Result<()> {
        let live = self.lifecycle.ensure().await?;
        let mut client = live.client();

        // Convention: source image = supplied tag. Destination defaults
        // to the same tag canonicalised to docker:// when no transport
        // scheme is present. This matches BuildahBackend's CLI surface.
        let destination = if tag.contains("://") {
            tag.to_string()
        } else {
            format!("docker://{tag}")
        };

        let req = proto::PushRequest {
            image: tag.to_string(),
            destination,
            format: String::new(),
            remove_signatures: false,
            auth: auth.map(push_auth_from),
        };
        client
            .push(tonic::Request::new(req))
            .await
            .map_err(|s| grpc_err(&s))?;
        Ok(())
    }

    /// gRPC equivalent of `buildah tag`.
    pub(super) async fn tag_image_impl(&self, image: &str, new_tag: &str) -> Result<()> {
        let live = self.lifecycle.ensure().await?;
        let mut client = live.client();
        let req = proto::TagRequest {
            image: image.into(),
            new_tag: new_tag.into(),
        };
        client
            .tag(tonic::Request::new(req))
            .await
            .map_err(|s| grpc_err(&s))?;
        Ok(())
    }

    /// gRPC equivalent of `buildah manifest create`.
    pub(super) async fn manifest_create_impl(&self, name: &str) -> Result<()> {
        let live = self.lifecycle.ensure().await?;
        let mut client = live.client();
        let req = proto::ManifestCreateRequest { name: name.into() };
        client
            .manifest_create(tonic::Request::new(req))
            .await
            .map_err(|s| grpc_err(&s))?;
        Ok(())
    }

    /// gRPC equivalent of `buildah manifest add`.
    pub(super) async fn manifest_add_impl(&self, manifest: &str, image: &str) -> Result<()> {
        let live = self.lifecycle.ensure().await?;
        let mut client = live.client();
        let req = proto::ManifestAddRequest {
            manifest: manifest.into(),
            image: image.into(),
        };
        client
            .manifest_add(tonic::Request::new(req))
            .await
            .map_err(|s| grpc_err(&s))?;
        Ok(())
    }

    /// gRPC equivalent of `buildah manifest push --all`.
    pub(super) async fn manifest_push_impl(&self, name: &str, destination: &str) -> Result<()> {
        let live = self.lifecycle.ensure().await?;
        let mut client = live.client();
        let req = proto::ManifestPushRequest {
            name: name.into(),
            destination: destination.into(),
            all: true,
            auth: None,
        };
        client
            .manifest_push(tonic::Request::new(req))
            .await
            .map_err(|s| grpc_err(&s))?;
        Ok(())
    }
}

/// Translate `RegistryAuth` (username + password) into the wire
/// `PushAuth`. The sidecar treats empty identity/registry tokens as
/// "not set" so we leave them blank.
fn push_auth_from(auth: &RegistryAuth) -> proto::PushAuth {
    proto::PushAuth {
        username: auth.username.clone(),
        password: auth.password.clone(),
        identity_token: String::new(),
        registry_token: String::new(),
    }
}

/// Map a tonic `Status` into the closest available `BuildError`.
/// `BuildahExecution` is the only variant carrying enough room for both
/// a gRPC code and a human message, and matches the rest of the
/// builder crate's error taxonomy where sidecar-side errors are
/// classified as "buildah failed."
fn grpc_err(status: &tonic::Status) -> BuildError {
    BuildError::BuildahExecution {
        command: format!("zlayer-buildd:{:?}", status.code()),
        exit_code: status.code() as i32,
        stderr: status.message().to_string(),
    }
}
