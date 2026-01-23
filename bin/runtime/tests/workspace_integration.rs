//! Workspace-level integration tests
//!
//! These tests verify that all crates work together correctly.

use std::process::Command;

/// Test that all crates compile together
#[test]
fn test_workspace_compiles() {
    let output = Command::new("cargo")
        .args(["check", "--workspace"])
        .output()
        .expect("Failed to run cargo check");

    assert!(
        output.status.success(),
        "Workspace check failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Test that the runtime binary has expected commands
#[test]
fn test_runtime_help() {
    let output = Command::new("cargo")
        .args(["run", "--package", "runtime", "--", "--help"])
        .output()
        .expect("Failed to run runtime");

    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(stdout.contains("status"), "Should have status command");
    assert!(stdout.contains("validate"), "Should have validate command");
}

/// Test that devctl binary has expected commands
#[test]
fn test_devctl_help() {
    let output = Command::new("cargo")
        .args(["run", "--package", "devctl", "--", "--help"])
        .output()
        .expect("Failed to run devctl");

    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(stdout.contains("token"), "Should have token command");
    assert!(stdout.contains("validate"), "Should have validate command");
}

/// Test that runtime has feature-gated commands when built with full features
#[test]
fn test_runtime_full_features_help() {
    let output = Command::new("cargo")
        .args([
            "run",
            "--package",
            "runtime",
            "--features",
            "full",
            "--",
            "--help",
        ])
        .output()
        .expect("Failed to run runtime with full features");

    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(stdout.contains("serve"), "Should have serve command with full features");
    assert!(stdout.contains("deploy"), "Should have deploy command with full features");
    assert!(stdout.contains("join"), "Should have join command with full features");
}

/// Test that runtime status command works
#[test]
fn test_runtime_status() {
    let output = Command::new("cargo")
        .args(["run", "--package", "runtime", "--", "status"])
        .output()
        .expect("Failed to run runtime status");

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should show runtime status info
    assert!(
        stdout.contains("ZLayer Runtime Status") || stdout.contains("Runtime"),
        "Should display runtime status information"
    );
}

/// Test that devctl token info command works
#[test]
fn test_devctl_token_info() {
    let output = Command::new("cargo")
        .args(["run", "--package", "devctl", "--", "token", "info"])
        .output()
        .expect("Failed to run devctl token info");

    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(stdout.contains("Token"), "Should display token info");
    assert!(stdout.contains("Roles"), "Should show available roles");
}

/// Test that devctl can create tokens
#[test]
fn test_devctl_token_create() {
    let output = Command::new("cargo")
        .args([
            "run",
            "--package",
            "devctl",
            "--",
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
        .expect("Failed to run devctl token create");

    assert!(output.status.success(), "Token creation should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    // JWT format: header.payload.signature
    let parts: Vec<&str> = stdout.trim().split('.').collect();
    assert_eq!(parts.len(), 3, "Token should be a valid JWT with 3 parts");
}

/// Test that devctl can generate join tokens
#[test]
fn test_devctl_generate_join_token() {
    let output = Command::new("cargo")
        .args([
            "run",
            "--package",
            "devctl",
            "--",
            "generate",
            "join-token",
            "-d",
            "test-deployment",
            "-a",
            "http://localhost:8080",
        ])
        .output()
        .expect("Failed to run devctl generate join-token");

    assert!(output.status.success(), "Join token generation should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Join Token Generated"), "Should show generation success");
    assert!(stdout.contains("test-deployment"), "Should include deployment name");
}
