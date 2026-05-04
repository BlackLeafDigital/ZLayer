//! Docker `Binds` / `Mounts` / `Tmpfs` -> `ZLayer` `StorageSpec[]`.
//!
//! Three independent input shapes share a single output type:
//! - `HostConfig.Binds`: legacy `"<source>:<target>[:options]"` strings,
//!   parsed by [`parse_bind_string`].
//! - `HostConfig.Mounts`: object-form mounts with `Type: "bind"|"volume"|
//!   "tmpfs"` (npipe is mapped to bind for translation purposes), handled by
//!   [`translate_mount`].
//! - `HostConfig.Tmpfs`: map of `target -> "size=64m,mode=1777"`-style option
//!   strings, parsed by [`parse_tmpfs_options`].

use zlayer_types::spec::StorageSpec;

use crate::socket::types::container_create::{HostConfig, MountSpec};

/// Translate an entire [`HostConfig`] into a flat `Vec<StorageSpec>`.
///
/// The order is: legacy `Binds` first, then object-form `Mounts`, then
/// `Tmpfs`. Unparseable inputs are silently dropped (consistent with the
/// permissive parsing approach across `translate::*`).
#[must_use]
pub fn translate(host_config: &HostConfig) -> Vec<StorageSpec> {
    let mut out = Vec::new();

    for bind in &host_config.binds {
        if let Some(spec) = parse_bind_string(bind) {
            out.push(spec);
        }
    }

    for mount in &host_config.mounts {
        out.push(translate_mount(mount));
    }

    for (target, opts) in &host_config.tmpfs {
        let (size, mode) = parse_tmpfs_options(opts);
        out.push(StorageSpec::Tmpfs {
            target: target.clone(),
            size,
            mode,
        });
    }

    out
}

/// Parse a legacy bind string (`docker run -v` form):
///
/// - `<source>:<target>`
/// - `<source>:<target>:ro`
/// - `<source>:<target>:rw`
/// - `<name>:<target>[:ro]` where `<name>` is not an absolute path -> named volume
///
/// Returns `None` when the input doesn't have at least a source/target pair
/// (e.g. an empty string).
#[must_use]
pub fn parse_bind_string(s: &str) -> Option<StorageSpec> {
    let parts: Vec<&str> = s.splitn(3, ':').collect();
    if parts.len() < 2 {
        return None;
    }
    let source = parts[0];
    let target = parts[1];
    if source.is_empty() || target.is_empty() {
        return None;
    }
    let readonly = parts.get(2).is_some_and(|opts| {
        opts.split(',')
            .any(|tok| tok.eq_ignore_ascii_case("ro") || tok.eq_ignore_ascii_case("readonly"))
    });

    // Docker treats absolute paths as bind mounts and bare names as named
    // volumes. Match that distinction so downstream agents can pick the right
    // backing storage.
    if is_absolute_path(source) {
        Some(StorageSpec::Bind {
            source: source.to_string(),
            target: target.to_string(),
            readonly,
        })
    } else {
        Some(StorageSpec::Named {
            name: source.to_string(),
            target: target.to_string(),
            readonly,
            tier: zlayer_types::spec::StorageTier::default(),
            size: None,
        })
    }
}

/// Translate one object-form [`MountSpec`] entry into a [`StorageSpec`].
///
/// `npipe` (Windows named pipes) maps to `Bind` since `ZLayer` doesn't have a
/// dedicated variant — the agent surface is identical at the OCI level.
#[must_use]
pub fn translate_mount(m: &MountSpec) -> StorageSpec {
    let readonly = m.read_only.unwrap_or(false);
    match m.mount_type.as_str() {
        "bind" | "npipe" => StorageSpec::Bind {
            source: m.source.clone(),
            target: m.target.clone(),
            readonly,
        },
        "tmpfs" => {
            let (size, mode) = m.tmpfs_options.as_ref().map_or((None, None), |o| {
                (o.size_bytes.map(|n| format!("{n}b")), o.mode)
            });
            StorageSpec::Tmpfs {
                target: m.target.clone(),
                size,
                mode,
            }
        }
        _ => {
            // "volume" or unknown -> named volume. An empty source means an
            // anonymous volume, matching Docker semantics.
            if m.source.is_empty() {
                StorageSpec::Anonymous {
                    target: m.target.clone(),
                    tier: zlayer_types::spec::StorageTier::default(),
                }
            } else {
                StorageSpec::Named {
                    name: m.source.clone(),
                    target: m.target.clone(),
                    readonly,
                    tier: zlayer_types::spec::StorageTier::default(),
                    size: None,
                }
            }
        }
    }
}

/// Parse a Docker `--tmpfs` options string, e.g. `"size=64m,mode=1777"`.
///
/// Recognised keys: `size` (returned as-is, e.g. `"64m"`) and `mode` (octal
/// or decimal integer). Unknown tokens are ignored.
#[must_use]
pub fn parse_tmpfs_options(opts: &str) -> (Option<String>, Option<u32>) {
    let mut size: Option<String> = None;
    let mut mode: Option<u32> = None;

    for tok in opts.split(',').filter(|t| !t.is_empty()) {
        if let Some(rest) = tok.strip_prefix("size=") {
            size = Some(rest.to_string());
        } else if let Some(rest) = tok.strip_prefix("mode=") {
            // Accept both octal (leading `0`) and decimal forms; Docker's
            // CLI passes octal strings.
            let parsed = if rest.starts_with('0') && rest.len() > 1 {
                u32::from_str_radix(rest, 8).ok()
            } else {
                rest.parse::<u32>().ok()
            };
            if let Some(m) = parsed {
                mode = Some(m);
            }
        }
    }

    (size, mode)
}

fn is_absolute_path(s: &str) -> bool {
    // Linux/macOS: starts with `/`. Windows: `<drive>:\` or `<drive>:/`.
    if s.starts_with('/') {
        return true;
    }
    let mut chars = s.chars();
    let drive = chars.next();
    let colon = chars.next();
    let sep = chars.next();
    matches!(
        (drive, colon, sep),
        (Some(c), Some(':'), Some('/' | '\\')) if c.is_ascii_alphabetic()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::socket::types::container_create::{
        BindOptions, HostConfig, MountSpec, TmpfsOptions, VolumeOptions,
    };

    #[test]
    fn legacy_bind_string_with_readonly() {
        let out = parse_bind_string("/host:/container:ro").unwrap();
        match out {
            StorageSpec::Bind {
                source,
                target,
                readonly,
            } => {
                assert_eq!(source, "/host");
                assert_eq!(target, "/container");
                assert!(readonly);
            }
            other => panic!("expected Bind, got {other:?}"),
        }
    }

    #[test]
    fn legacy_bind_string_without_options_is_rw() {
        let out = parse_bind_string("/host:/container").unwrap();
        match out {
            StorageSpec::Bind { readonly, .. } => assert!(!readonly),
            other => panic!("expected Bind, got {other:?}"),
        }
    }

    #[test]
    fn legacy_bind_string_with_named_volume_source() {
        let out = parse_bind_string("my-vol:/data").unwrap();
        match out {
            StorageSpec::Named { name, target, .. } => {
                assert_eq!(name, "my-vol");
                assert_eq!(target, "/data");
            }
            other => panic!("expected Named, got {other:?}"),
        }
    }

    #[test]
    fn malformed_bind_string_returns_none() {
        assert!(parse_bind_string("").is_none());
        assert!(parse_bind_string("just-a-source").is_none());
        assert!(parse_bind_string(":/target").is_none());
    }

    #[test]
    fn object_mount_bind_translates() {
        let m = MountSpec {
            target: "/container".to_string(),
            source: "/host".to_string(),
            mount_type: "bind".to_string(),
            read_only: Some(true),
            consistency: None,
            bind_options: Some(BindOptions {
                propagation: Some("rprivate".to_string()),
                ..BindOptions::default()
            }),
            volume_options: None,
            tmpfs_options: None,
        };
        match translate_mount(&m) {
            StorageSpec::Bind {
                source,
                target,
                readonly,
            } => {
                assert_eq!(source, "/host");
                assert_eq!(target, "/container");
                assert!(readonly);
            }
            other => panic!("expected Bind, got {other:?}"),
        }
    }

    #[test]
    fn object_mount_volume_named_translates() {
        let m = MountSpec {
            target: "/data".to_string(),
            source: "vol1".to_string(),
            mount_type: "volume".to_string(),
            read_only: Some(false),
            consistency: None,
            bind_options: None,
            volume_options: Some(VolumeOptions::default()),
            tmpfs_options: None,
        };
        match translate_mount(&m) {
            StorageSpec::Named { name, target, .. } => {
                assert_eq!(name, "vol1");
                assert_eq!(target, "/data");
            }
            other => panic!("expected Named, got {other:?}"),
        }
    }

    #[test]
    fn object_mount_volume_anonymous_translates() {
        let m = MountSpec {
            target: "/cache".to_string(),
            source: String::new(),
            mount_type: "volume".to_string(),
            ..MountSpec::default()
        };
        match translate_mount(&m) {
            StorageSpec::Anonymous { target, .. } => assert_eq!(target, "/cache"),
            other => panic!("expected Anonymous, got {other:?}"),
        }
    }

    #[test]
    fn object_mount_tmpfs_translates() {
        let m = MountSpec {
            target: "/tmp".to_string(),
            source: String::new(),
            mount_type: "tmpfs".to_string(),
            tmpfs_options: Some(TmpfsOptions {
                size_bytes: Some(67_108_864),
                mode: Some(0o1777),
            }),
            ..MountSpec::default()
        };
        match translate_mount(&m) {
            StorageSpec::Tmpfs { target, size, mode } => {
                assert_eq!(target, "/tmp");
                assert_eq!(size.as_deref(), Some("67108864b"));
                assert_eq!(mode, Some(0o1777));
            }
            other => panic!("expected Tmpfs, got {other:?}"),
        }
    }

    #[test]
    fn parse_tmpfs_options_size_and_mode() {
        let (size, mode) = parse_tmpfs_options("size=64m,mode=1777");
        assert_eq!(size.as_deref(), Some("64m"));
        // `1777` (no leading zero) parses as decimal — confirm we accept it.
        assert_eq!(mode, Some(1777));
    }

    #[test]
    fn parse_tmpfs_options_octal_mode() {
        let (_size, mode) = parse_tmpfs_options("mode=01777");
        assert_eq!(mode, Some(0o1777));
    }

    #[test]
    fn translate_combines_all_sources() {
        let mut hc = HostConfig {
            binds: vec!["/host:/container:ro".to_string()],
            mounts: vec![MountSpec {
                target: "/data".to_string(),
                source: "vol1".to_string(),
                mount_type: "volume".to_string(),
                ..MountSpec::default()
            }],
            ..HostConfig::default()
        };
        hc.tmpfs.insert("/tmp".to_string(), "size=64m".to_string());

        let out = translate(&hc);
        assert_eq!(out.len(), 3);
        assert!(matches!(out[0], StorageSpec::Bind { .. }));
        assert!(matches!(out[1], StorageSpec::Named { .. }));
        assert!(matches!(out[2], StorageSpec::Tmpfs { .. }));
    }
}
