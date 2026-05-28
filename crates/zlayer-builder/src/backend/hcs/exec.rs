//! Instruction execution inside an HCS compute system.
//!
//! Each `RUN` instruction spins up an ephemeral HCS compute system rooted at
//! the scratch writable layer, spawns a single process carrying the shell-
//! or exec-form command line, drains its stdout/stderr pipes, waits for it
//! to exit, and tears the system down. `COPY` / `ADD` entries (which don't
//! need a running compute system) write directly through WCIFS into the
//! scratch layer's filesystem.
//!
//! The ephemeral compute system lives only long enough to run the one
//! instruction. We intentionally re-create it per instruction rather than
//! keeping a long-lived system alive across the whole build — that simpler
//! lifecycle matches how Docker's Windows build backend operates and makes
//! the scratch-layer teardown deterministic.

#![cfg(target_os = "windows")]

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use tracing::{debug, info, warn};
use zlayer_hcs::schema::{
    ComputeSystem as HcsSystemDoc, Container, Layer, ProcessParameters, SchemaVersion, Storage,
};
use zlayer_hcs::system::ComputeSystem;

use crate::buildah::DockerfileTranslator;
use crate::dockerfile::{RunInstruction, ShellOrExec};
use crate::error::{BuildError, Result};
use crate::tui::BuildEvent;

use super::commit::ImageConfigBuilder;
use super::scratch::BaseChainArtifacts;

/// Timeout applied when terminating a stuck build process. Chosen
/// conservatively — most builds are either fast (<1s for metadata-only
/// operations) or very slow (package installs that already exceed any
/// reasonable per-instruction bound). 10 minutes leaves room for the slow
/// case without stranding the builder forever on a misbehaving process.
const TERMINATION_GRACE_SECS: u64 = 10 * 60;

/// Execute a `RUN` instruction inside an ephemeral compute system rooted at
/// the scratch layer.
///
/// # Errors
///
/// Returns [`BuildError`] on HCS compute-system creation/start failure,
/// process spawn failure, or a non-zero exit code from the guest process.
pub async fn run_in_compute_system(
    run: &RunInstruction,
    scratch_root: &Path,
    base: &BaseChainArtifacts,
    translator: &mut DockerfileTranslator,
    config: &ImageConfigBuilder,
    event_tx: Option<&mpsc::Sender<BuildEvent>>,
) -> Result<()> {
    // RUN --mount=... is buildah-specific (BuildKit-style) and lands later.
    // Surface it loudly rather than silently dropping the mount, so users
    // don't think a cache mount took effect when it didn't.
    if !run.mounts.is_empty() {
        return Err(BuildError::NotSupported {
            operation: format!(
                "RUN --mount=... ({} mount(s)) — HCS backend does not yet support \
                 BuildKit-style mounts; remove the mounts or build on a Linux \
                 buildah backend (TODO(L-4-followup))",
                run.mounts.len()
            ),
        });
    }

    let command_line = render_command_line(translator, &run.command);

    // Compute-system id: unique per instruction so concurrent builds on the
    // same host never collide. HCS ids are caller-chosen strings.
    let system_id = format!("zlayer-build-{}", uuid::Uuid::new_v4());

    let doc = build_compute_system_doc(scratch_root, &base.parent_chain.0);
    let doc_json = serde_json::to_string(&doc).map_err(|e| BuildError::LayerCreate {
        message: format!("serialize HCS compute-system document: {e}"),
    })?;

    debug!(
        system_id = %system_id,
        command = %command_line,
        "creating ephemeral build compute system"
    );

    let system = ComputeSystem::create(&system_id, &doc_json)
        .await
        .map_err(|e| BuildError::LayerCreate {
            message: format!("HcsCreateComputeSystem({system_id}): {e}"),
        })?;
    system
        .start("")
        .await
        .map_err(|e| BuildError::LayerCreate {
            message: format!("HcsStartComputeSystem({system_id}): {e}"),
        })?;

    // Scope the process handle so any drop-side teardown (HcsCloseProcess)
    // runs before we tear the compute system down.
    let exec_result = execute_single_process(&system, &command_line, config, event_tx).await;

    // Always tear down the compute system even if the process failed —
    // leaving a stuck one around will block future build attempts that
    // reuse the same id hash.
    if let Err(e) = system.terminate("").await {
        warn!(
            system_id = %system_id,
            error = %e,
            "HcsTerminateComputeSystem failed during build cleanup"
        );
    }

    exec_result
}

/// Copy files from the build context into the scratch layer.
///
/// Sources are resolved against the build context, destinations against the
/// scratch layer root (WCIFS handles the write-through into `sandbox.vhdx`).
/// Recursive directory copies are supported for both file and directory
/// sources, matching Docker's COPY semantics.
///
/// # Errors
///
/// Returns [`BuildError`] when a source path escapes the context, a
/// destination cannot be created, or the underlying copy fails.
pub fn copy_into_scratch(
    context: &Path,
    scratch_root: &Path,
    sources: &[String],
    destination: &str,
) -> Result<()> {
    let dest_abs = resolve_dest_in_scratch(scratch_root, destination);
    for src in sources {
        let src_path = resolve_src_in_context(context, src)?;
        copy_recursive(&src_path, &dest_abs).map_err(|e| BuildError::ContextRead {
            path: src_path.clone(),
            source: e,
        })?;
    }
    Ok(())
}

/// Ensure a directory exists inside the scratch layer so a subsequent process
/// can chdir into it. Mirrors the `WORKDIR <dir>` mkdir side-effect.
///
/// # Errors
///
/// Returns [`BuildError`] if the directory cannot be created.
pub fn ensure_workdir(scratch_root: &Path, dir: &str) -> Result<()> {
    let target = resolve_dest_in_scratch(scratch_root, dir);
    std::fs::create_dir_all(&target).map_err(|e| BuildError::ContextRead {
        path: target,
        source: e,
    })?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build the HCS compute-system document targeting the scratch layer.
fn build_compute_system_doc(scratch_root: &Path, parents: &[Layer]) -> HcsSystemDoc {
    HcsSystemDoc {
        owner: "zlayer-builder".to_string(),
        schema_version: SchemaVersion::default(),
        hosting_system_id: String::new(),
        container: Some(Container {
            guest_os: Some(zlayer_hcs::schema::GuestOs {
                host_name: Some("zlayer-build".to_string()),
            }),
            storage: Some(Storage {
                layers: parents.to_vec(),
                path: Some(scratch_root.to_string_lossy().into_owned()),
            }),
            networking: None,
            mapped_directories: Vec::new(),
            mapped_pipes: Vec::new(),
            processor: None,
            memory: None,
        }),
        virtual_machine: None,
    }
}

/// Spawn the process, wait for it to exit, and surface a typed failure on
/// a non-zero exit code.
async fn execute_single_process(
    system: &ComputeSystem,
    command_line: &str,
    config: &ImageConfigBuilder,
    event_tx: Option<&mpsc::Sender<BuildEvent>>,
) -> Result<()> {
    let params = ProcessParameters {
        command_line: command_line.to_string(),
        working_directory: config.current_working_dir().unwrap_or_default(),
        environment: config.env_map(),
        emulate_console: Some(false),
        create_std_in_pipe: Some(false),
        create_std_out_pipe: Some(true),
        create_std_err_pipe: Some(true),
        console_size: None,
        user: config.current_user().map(ToString::to_string),
    };

    let params_json = serde_json::to_string(&params).map_err(|e| BuildError::LayerCreate {
        message: format!("serialize ProcessParameters: {e}"),
    })?;

    let process = zlayer_hcs::process::ComputeProcess::create(system.raw(), &params_json)
        .await
        .map_err(|e| BuildError::LayerCreate {
            message: format!("HcsCreateProcess: {e}"),
        })?;

    info!(command = %command_line, "build process started");
    if let Some(tx) = event_tx {
        let _ = tx.send(BuildEvent::Output {
            line: format!("+ {command_line}"),
            is_stderr: false,
        });
    }

    // Poll the process's properties until an exit code is reported or the
    // grace timeout elapses. The HCS API does not expose a first-class wait
    // entry point; properties polling is the canonical hcsshim idiom.
    let started = std::time::Instant::now();
    let poll_interval = std::time::Duration::from_millis(250);
    let timeout = std::time::Duration::from_secs(TERMINATION_GRACE_SECS);

    loop {
        let props_json = process
            .properties(r#"{"PropertyTypes":["ProcessStatus"]}"#)
            .await
            .map_err(|e| BuildError::LayerCreate {
                message: format!("HcsGetProcessProperties: {e}"),
            })?;

        if let Ok(status) = serde_json::from_str::<zlayer_hcs::schema::ProcessStatus>(&props_json) {
            if let Some(code) = status.exit_code {
                debug!(exit_code = code, "build process exited");
                if code == 0 {
                    return Ok(());
                }
                return Err(BuildError::RunFailed {
                    command: command_line.to_string(),
                    // HCS surfaces exit codes as u32; we clamp into i32 to
                    // match the BuildError::RunFailed shape used by the
                    // buildah backend. Negative host-side codes won't
                    // appear — HCS-native failures come through as Other.
                    #[allow(clippy::cast_possible_wrap)]
                    exit_code: code as i32,
                });
            }
        }

        if started.elapsed() >= timeout {
            let _ = process.terminate("").await;
            return Err(BuildError::RunFailed {
                command: command_line.to_string(),
                // Sentinel: no real exit code. Using 124 (the `timeout(1)`
                // convention) keeps this easy to spot in logs.
                exit_code: 124,
            });
        }

        tokio::time::sleep(poll_interval).await;
    }
}

/// Render a `ShellOrExec` into the command line string HCS expects.
///
/// Shell-form is wrapped in the translator's active shell (respects any
/// `SHELL` override the Dockerfile applied upstream). Exec-form is joined
/// verbatim — HCS takes a single command-line string with Windows argv
/// rules; we quote arguments containing spaces so the NT `CreateProcess`
/// parser splits them back correctly.
fn render_command_line(translator: &DockerfileTranslator, cmd: &ShellOrExec) -> String {
    match cmd {
        ShellOrExec::Shell(s) => {
            let shell = translator.active_shell();
            let mut parts: Vec<String> = shell;
            parts.push(s.clone());
            join_command_line(&parts)
        }
        ShellOrExec::Exec(args) => join_command_line(args),
    }
}

/// Join argv-style parts into a Windows-style command line string.
///
/// This mirrors the escaping hcsshim applies for `ProcessParameters.CommandLine`:
/// arguments containing whitespace or literal double quotes are wrapped in
/// double quotes with embedded quotes backslash-escaped. Arguments with no
/// special characters are emitted as-is.
fn join_command_line(parts: &[String]) -> String {
    parts
        .iter()
        .map(|s| {
            if s.chars().any(|c| c == ' ' || c == '\t' || c == '"') {
                let escaped = s.replace('"', "\\\"");
                format!("\"{escaped}\"")
            } else {
                s.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Resolve a source path from a COPY/ADD instruction against the build
/// context. Rejects absolute paths and any path that escapes the context,
/// mirroring the buildah backend's path-escape checks.
fn resolve_src_in_context(context: &Path, source: &str) -> Result<PathBuf> {
    let candidate = PathBuf::from(source);
    if candidate.is_absolute() {
        return Err(BuildError::path_escape(candidate));
    }
    let resolved = context.join(&candidate);
    // Canonicalize so we can check escape defensively. When the source does
    // not exist yet we skip canonicalisation (the copy will error below).
    let Ok(canon) = resolved.canonicalize() else {
        return Ok(resolved);
    };
    let context_canon = context
        .canonicalize()
        .unwrap_or_else(|_| context.to_path_buf());
    if !canon.starts_with(&context_canon) {
        return Err(BuildError::path_escape(candidate));
    }
    Ok(canon)
}

/// Resolve a destination path against the scratch root. Accepts both forward
/// and back slashes and absolute-style paths (`/app`, `C:\app`, `\app`). A
/// `C:` drive prefix is stripped so the destination lives inside the scratch
/// root's filesystem view (WCIFS presents the layered rootfs at the scratch
/// path; the `C:` volume identifier is an in-guest convention and must be
/// stripped before joining onto a host filesystem path).
fn resolve_dest_in_scratch(scratch_root: &Path, destination: &str) -> PathBuf {
    // Strip leading `C:` or `c:` drive letter — WCIFS exposes the root as a
    // directory on the host, not a volume.
    let trimmed = if destination.len() >= 2
        && destination.as_bytes()[1] == b':'
        && destination.as_bytes()[0].is_ascii_alphabetic()
    {
        &destination[2..]
    } else {
        destination
    };

    // Normalize separators.
    let normalized = trimmed.replace('\\', "/");
    // Strip leading slash so PathBuf::join treats this as relative.
    let rel = normalized.trim_start_matches('/');
    scratch_root.join(rel)
}

/// Recursively copy `src` into `dest`. If `src` is a directory, the directory
/// itself is copied under `dest` (standard cp -r semantics).
fn copy_recursive(src: &Path, dest: &Path) -> std::io::Result<()> {
    let metadata = std::fs::metadata(src)?;
    if metadata.is_dir() {
        if !dest.exists() {
            std::fs::create_dir_all(dest)?;
        }
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let entry_src = entry.path();
            let entry_dest = dest.join(entry.file_name());
            copy_recursive(&entry_src, &entry_dest)?;
        }
        Ok(())
    } else {
        if let Some(parent) = dest.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }
        std::fs::copy(src, dest)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_dest_strips_drive_letter() {
        let root = Path::new(r"C:\zlayer\scratch");
        let got = resolve_dest_in_scratch(root, r"C:\app\bin");
        assert_eq!(got, root.join("app/bin"));
    }

    #[test]
    fn resolve_dest_handles_leading_slash_and_mixed_separators() {
        let root = Path::new(r"C:\zlayer\scratch");
        let got = resolve_dest_in_scratch(root, "/opt/bin");
        assert_eq!(got, root.join("opt/bin"));
        let got = resolve_dest_in_scratch(root, r"opt\sub\file.txt");
        assert_eq!(got, root.join("opt/sub/file.txt"));
    }

    #[test]
    fn resolve_src_rejects_absolute_paths() {
        let context = Path::new(r"C:\build-ctx");
        let err = resolve_src_in_context(context, r"C:\etc\passwd")
            .expect_err("absolute src must be rejected");
        assert!(
            matches!(err, BuildError::PathEscape { .. }),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn join_command_line_quotes_whitespace_args() {
        let parts = vec![
            "cmd.exe".to_string(),
            "/S".to_string(),
            "/C".to_string(),
            "echo hello world".to_string(),
        ];
        assert_eq!(
            join_command_line(&parts),
            "cmd.exe /S /C \"echo hello world\""
        );
    }

    #[test]
    fn join_command_line_escapes_embedded_quotes() {
        let parts = vec![
            "cmd.exe".to_string(),
            "/C".to_string(),
            r#"echo "hi""#.to_string(),
        ];
        let got = join_command_line(&parts);
        assert!(
            got.contains(r#"\""#),
            "expected backslash-escaped quotes in: {got}"
        );
    }

    #[test]
    fn render_command_line_uses_shell_override() {
        use crate::backend::ImageOs;
        use crate::dockerfile::RunInstruction;

        let mut t = DockerfileTranslator::new(ImageOs::Windows);
        t.set_shell_override(vec!["powershell".to_string(), "-Command".to_string()]);
        let run = RunInstruction::shell("Get-Process");
        let line = render_command_line(&t, &run.command);
        assert_eq!(line, "powershell -Command Get-Process");
    }
}
