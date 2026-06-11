//! Shared blob-cache prune helper.
//!
//! Garbage-collects orphaned OCI blobs from any [`BlobCacheBackend`] by
//! walking the cached `manifest:` keyspace to build the set of still-referenced
//! digests, then deleting every `sha256:` blob not in that set.
//!
//! This is the backend-agnostic core extracted from the agent's image-prune
//! path so that any caller holding a `&dyn BlobCacheBackend` can run the same
//! garbage collection without depending on the agent crate.

use crate::cache::BlobCacheBackend;
use crate::error::CacheError;
use oci_client::manifest::OciImageManifest;

/// Delete blobs not referenced by any cached `manifest:` entry.
///
/// Walks every `manifest:` key to collect the config and layer digests that are
/// still referenced, then deletes each `sha256:` blob absent from that set.
/// Blob sizes are summed best-effort before deletion to report reclaimed bytes;
/// a delete failure for an individual blob is logged and skipped rather than
/// aborting the prune.
///
/// Returns `(deleted_digests, bytes_reclaimed)`.
///
/// # Errors
///
/// Returns [`CacheError`] if enumerating the `manifest:` or `sha256:` keyspaces
/// fails. Per-blob `get`/`delete` failures are non-fatal and do not propagate.
pub async fn prune_dangling_blobs(
    cache: &dyn BlobCacheBackend,
) -> Result<(Vec<String>, u64), CacheError> {
    // 1. Collect all digests referenced by remaining manifests.
    let manifest_keys = cache.keys_with_prefix("manifest:").await?;

    let mut referenced: std::collections::HashSet<String> = std::collections::HashSet::new();
    for key in &manifest_keys {
        let Ok(Some(bytes)) = cache.get(key).await else {
            continue;
        };
        let Ok(manifest) = serde_json::from_slice::<OciImageManifest>(&bytes) else {
            continue;
        };
        referenced.insert(manifest.config.digest.clone());
        for layer in &manifest.layers {
            referenced.insert(layer.digest.clone());
        }
    }

    // 2. Walk all sha256:* blob keys and delete those not referenced.
    let all_blob_keys = cache.keys_with_prefix("sha256:").await?;

    let mut deleted = Vec::new();
    let mut reclaimed: u64 = 0;

    for key in all_blob_keys {
        if referenced.contains(&key) {
            continue;
        }
        // Grab the blob size before deleting (best-effort).
        if let Ok(Some(bytes)) = cache.get(&key).await {
            reclaimed = reclaimed.saturating_add(bytes.len() as u64);
        }
        if let Err(e) = cache.delete(&key).await {
            tracing::warn!(
                digest = %key,
                error = %e,
                "failed to delete orphaned blob during prune"
            );
            continue;
        }
        deleted.push(key);
    }

    Ok((deleted, reclaimed))
}

#[cfg(all(test, feature = "persistent"))]
mod tests {
    use super::*;
    use crate::cache::compute_digest;
    use crate::persistent_cache::PersistentBlobCache;
    use oci_client::manifest::OciDescriptor;
    use tempfile::TempDir;

    #[tokio::test]
    async fn prune_dangling_blobs_deletes_only_orphans() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join("prune_cache.sqlite");
        let cache = PersistentBlobCache::open(&cache_path).await.unwrap();

        // Two referenced blobs: a config and a layer. Their cache keys must be
        // the real sha256 of their contents, because the persistent backend
        // verifies `sha256:` keys against their data on `put`.
        let config_data = b"referenced config blob";
        let layer_data = b"referenced layer blob";
        let config_digest = compute_digest(config_data);
        let layer_digest = compute_digest(layer_data);

        cache.put(&config_digest, config_data).await.unwrap();
        cache.put(&layer_digest, layer_data).await.unwrap();

        // One orphan blob with a known byte length, referenced by no manifest.
        let orphan_data = b"orphaned dangling blob with known length";
        let orphan_digest = compute_digest(orphan_data);
        let orphan_len = orphan_data.len() as u64;
        cache.put(&orphan_digest, orphan_data).await.unwrap();

        // A real manifest referencing the config and layer digests above.
        let manifest = OciImageManifest {
            config: OciDescriptor {
                digest: config_digest.clone(),
                ..OciDescriptor::default()
            },
            layers: vec![OciDescriptor {
                digest: layer_digest.clone(),
                ..OciDescriptor::default()
            }],
            ..OciImageManifest::default()
        };
        let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
        cache
            .put("manifest:example/image:latest", &manifest_bytes)
            .await
            .unwrap();

        let (deleted, reclaimed) = prune_dangling_blobs(&cache).await.unwrap();

        // Only the orphan should have been deleted.
        assert_eq!(deleted, vec![orphan_digest.clone()]);
        assert_eq!(reclaimed, orphan_len);

        // Referenced blobs survive; orphan is gone.
        assert!(cache.get(&config_digest).await.unwrap().is_some());
        assert!(cache.get(&layer_digest).await.unwrap().is_some());
        assert!(cache.get(&orphan_digest).await.unwrap().is_none());
    }
}
