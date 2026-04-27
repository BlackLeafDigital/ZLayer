//! Cross-platform unit tests for the Windows builder templates (Phase L-8).
//!
//! These tests exercise the L-1 deliverables — the `windows-nanoserver` and
//! `windows-servercore` Dockerfile templates plus the `Runtime` enum variants
//! that map to them — and the detection heuristics that surface the right
//! runtime for a given project layout. None of these tests touch HCS, the
//! registry, or any Windows-specific kernel API, so they run on every host.
//!
//! Scope:
//! - Template parse + base-image + instruction-shape assertions.
//! - `detect_runtime` heuristics for `.sln` / `.csproj` / `.vcxproj` / `.exe`.
//! - `detect_runtime` precedence: a `.exe` next to a `Cargo.toml` must not
//!   demote the project to `Runtime::WindowsNanoserver`.
//! - `resolve_runtime` precedence: an explicit runtime name beats detection
//!   (the `os:` / `--platform` override plumbs through here).
//!
//! Running:
//! ```bash
//! cargo test -p zlayer-builder --test windows_templates
//! ```

use std::fs;

use tempfile::TempDir;
use zlayer_builder::{detect_runtime, resolve_runtime, Dockerfile, Instruction, Runtime};

/// Shared helper — same shape as the private `create_temp_dir` in the
/// detection unit tests, lifted into this integration suite.
fn tmpdir() -> TempDir {
    TempDir::new().expect("create temp dir for detection test")
}

// ---------------------------------------------------------------------------
// Template parse tests
// ---------------------------------------------------------------------------

#[test]
fn windows_nanoserver_template_parses() {
    let template = Runtime::WindowsNanoserver.template();
    let dockerfile = Dockerfile::parse(template).expect("nanoserver template should parse");

    // Single-stage minimal template.
    assert_eq!(
        dockerfile.stages.len(),
        1,
        "nanoserver template is single-stage"
    );
    let stage = &dockerfile.stages[0];

    // FROM must be the pinned ltsc2022 nanoserver on mcr.microsoft.com.
    let base = stage.base_image.to_string();
    assert_eq!(
        base, "mcr.microsoft.com/windows/nanoserver:ltsc2022",
        "nanoserver template must FROM the pinned ltsc2022 base, got {base}"
    );

    // WORKDIR must exist and be a Windows-style drive-rooted path.
    let has_windows_workdir = stage
        .instructions
        .iter()
        .any(|i| matches!(i, Instruction::Workdir(w) if w.trim().starts_with("C:")));
    assert!(
        has_windows_workdir,
        "nanoserver template must set a Windows WORKDIR (C:\\...)"
    );

    // CMD must be present (exec or shell form).
    let has_cmd = stage
        .instructions
        .iter()
        .any(|i| matches!(i, Instruction::Cmd(_)));
    assert!(has_cmd, "nanoserver template must define a CMD");
}

#[test]
fn windows_servercore_template_parses() {
    let template = Runtime::WindowsServerCore.template();
    let dockerfile = Dockerfile::parse(template).expect("servercore template should parse");

    assert_eq!(
        dockerfile.stages.len(),
        1,
        "servercore template is single-stage"
    );
    let stage = &dockerfile.stages[0];

    let base = stage.base_image.to_string();
    assert_eq!(
        base, "mcr.microsoft.com/windows/servercore:ltsc2022",
        "servercore template must FROM the pinned ltsc2022 base, got {base}"
    );

    // SHELL ["powershell", "-Command"] must be present — this is the
    // load-bearing difference vs. nanoserver (which ships no PowerShell).
    let has_powershell_shell = stage.instructions.iter().any(|i| {
        matches!(
            i,
            Instruction::Shell(argv)
                if argv.len() >= 2
                    && argv[0] == "powershell"
                    && argv[1] == "-Command"
        )
    });
    assert!(
        has_powershell_shell,
        "servercore template must switch SHELL to [\"powershell\", \"-Command\"]"
    );

    // WORKDIR + CMD sanity.
    let has_workdir = stage
        .instructions
        .iter()
        .any(|i| matches!(i, Instruction::Workdir(w) if w.trim().starts_with("C:")));
    assert!(
        has_workdir,
        "servercore template must set a Windows WORKDIR (C:\\...)"
    );
    let has_cmd = stage
        .instructions
        .iter()
        .any(|i| matches!(i, Instruction::Cmd(_)));
    assert!(has_cmd, "servercore template must define a CMD");
}

// ---------------------------------------------------------------------------
// Detection heuristic tests
// ---------------------------------------------------------------------------

#[test]
fn detect_runtime_sln_is_servercore() {
    let dir = tmpdir();
    // An otherwise empty .sln — classify_base_image in L-5 is substring-only,
    // we just need the extension to trip the heuristic.
    fs::write(dir.path().join("MyApp.sln"), "").unwrap();

    let runtime = detect_runtime(dir.path());
    assert_eq!(
        runtime,
        Some(Runtime::WindowsServerCore),
        "a bare .sln should map to WindowsServerCore"
    );
}

#[test]
fn detect_runtime_csproj_is_servercore() {
    let dir = tmpdir();
    fs::write(
        dir.path().join("MyApp.csproj"),
        r#"<Project Sdk="Microsoft.NET.Sdk"></Project>"#,
    )
    .unwrap();

    let runtime = detect_runtime(dir.path());
    assert_eq!(
        runtime,
        Some(Runtime::WindowsServerCore),
        "a bare .csproj should map to WindowsServerCore"
    );
}

#[test]
fn detect_runtime_vcxproj_is_servercore() {
    let dir = tmpdir();
    fs::write(dir.path().join("Native.vcxproj"), "").unwrap();

    let runtime = detect_runtime(dir.path());
    assert_eq!(
        runtime,
        Some(Runtime::WindowsServerCore),
        "a bare .vcxproj should map to WindowsServerCore"
    );
}

#[test]
fn detect_runtime_standalone_exe_is_nanoserver() {
    let dir = tmpdir();
    // Minimal PE magic header just to keep the file recognisably "executable".
    // The detector only looks at the extension, but making the content even
    // slightly legitimate protects against a future content-sniffing change.
    fs::write(dir.path().join("app.exe"), b"MZ\x90\x00").unwrap();

    let runtime = detect_runtime(dir.path());
    assert_eq!(
        runtime,
        Some(Runtime::WindowsNanoserver),
        "a bare .exe with no Linux-ecosystem indicators should map to \
         WindowsNanoserver (smallest self-contained wrapper)"
    );
}

#[test]
fn detect_runtime_exe_does_not_override_cargo_toml() {
    // A .exe next to a Cargo.toml is almost certainly a helper artifact
    // (test tool, prebuilt asset). The Rust project signal must win — we do
    // NOT want `zlayer build` to silently wrap a Rust project as a Windows
    // self-contained binary image.
    let dir = tmpdir();
    fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"demo\"").unwrap();
    fs::write(dir.path().join("helper.exe"), b"MZ").unwrap();

    let runtime = detect_runtime(dir.path());
    assert_eq!(
        runtime,
        Some(Runtime::Rust),
        "Rust (Cargo.toml) must outrank a stray .exe at the context root"
    );
}

#[test]
fn resolve_runtime_explicit_nanoserver_overrides_detect() {
    // An explicit runtime name (what `--runtime` / `os:` / `--platform` wire
    // through) always beats auto-detection — even if the project layout looks
    // like a perfectly valid Rust project.
    let dir = tmpdir();
    fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"demo\"").unwrap();

    let resolved = resolve_runtime(Some("windows-nanoserver"), dir.path(), false);
    assert_eq!(
        resolved,
        Some(Runtime::WindowsNanoserver),
        "explicit --runtime windows-nanoserver must win over Cargo.toml detection"
    );

    // Sanity: omitting the explicit name falls back to detection, which picks
    // Rust for this layout — proving the override (not the detection) is what
    // flips the verdict above.
    let detected = resolve_runtime(None, dir.path(), false);
    assert_eq!(
        detected,
        Some(Runtime::Rust),
        "with no explicit runtime, detection still picks Rust"
    );
}
