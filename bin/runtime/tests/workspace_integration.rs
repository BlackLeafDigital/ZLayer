//! Integration tests for the zlayer runtime binary
//!
//! Uses the pre-compiled binary via CARGO_BIN_EXE to avoid re-compilation at test time.

use std::process::Command;

/// Test that the runtime binary has expected commands
#[test]
fn test_runtime_help() {
    let output = Command::new(env!("CARGO_BIN_EXE_zlayer-runtime"))
        .args(["--help"])
        .output()
        .expect("Failed to run zlayer");

    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(stdout.contains("status"), "Should have status command");
    assert!(stdout.contains("validate"), "Should have validate command");
}

/// Test that runtime has feature-gated commands when built with full features
#[test]
fn test_runtime_full_features_help() {
    let output = Command::new(env!("CARGO_BIN_EXE_zlayer-runtime"))
        .args(["--help"])
        .output()
        .expect("Failed to run zlayer");

    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.contains("serve"),
        "Should have serve command with full features"
    );
    assert!(
        stdout.contains("deploy"),
        "Should have deploy command with full features"
    );
    assert!(
        stdout.contains("join"),
        "Should have join command with full features"
    );
}

/// Test that runtime status command works
#[test]
fn test_runtime_status() {
    let output = Command::new(env!("CARGO_BIN_EXE_zlayer-runtime"))
        .args(["status"])
        .output()
        .expect("Failed to run zlayer status");

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should show runtime status info
    assert!(
        stdout.contains("ZLayer Runtime Status") || stdout.contains("Runtime"),
        "Should display runtime status information"
    );
}

/// Test that zlayer token info command works
#[test]
fn test_zlayer_token_info() {
    let output = Command::new(env!("CARGO_BIN_EXE_zlayer-runtime"))
        .args(["token", "info"])
        .output()
        .expect("Failed to run zlayer token info");

    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(stdout.contains("Token"), "Should display token info");
    assert!(stdout.contains("Roles"), "Should show available roles");
}

/// Test that zlayer can create tokens
#[test]
fn test_zlayer_token_create() {
    let output = Command::new(env!("CARGO_BIN_EXE_zlayer-runtime"))
        .args([
            "token",
            "create",
            "--quiet",
            "--subject",
            "test-user",
            "--hours",
            "1",
            "--roles",
            "reader",
        ])
        .output()
        .expect("Failed to run zlayer token create");

    assert!(output.status.success(), "Token creation should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Get the last non-empty line (the JWT token) - observability logs may precede it
    let token = stdout
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('{'))
        .next_back()
        .expect("Should have token output");
    // JWT format: header.payload.signature
    let parts: Vec<&str> = token.split('.').collect();
    assert_eq!(parts.len(), 3, "Token should be a valid JWT with 3 parts");
}

/// Test that zlayer can generate join tokens
#[test]
fn test_zlayer_generate_join_token() {
    let output = Command::new(env!("CARGO_BIN_EXE_zlayer-runtime"))
        .args([
            "node",
            "generate-join-token",
            "-d",
            "test-deployment",
            "-a",
            "http://localhost:8080",
        ])
        .output()
        .expect("Failed to run zlayer node generate-join-token");

    assert!(
        output.status.success(),
        "Join token generation should succeed"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Join Token Generated"),
        "Should show generation success"
    );
    assert!(
        stdout.contains("test-deployment"),
        "Should include deployment name"
    );
}
