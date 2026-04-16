//! `zlayer variable` subcommand handlers.
//!
//! Manages plaintext key-value variables for template substitution in
//! deployment specs. Variables are NOT encrypted (unlike secrets). They
//! live either globally (`scope = None`) or under a specific project.
//!
//! Dispatches to the daemon's variable endpoints
//! (`GET/POST/PATCH/DELETE /api/v1/variables[/id]`) via [`DaemonClient`].

use anyhow::{Context, Result};
use zlayer_api::storage::StoredVariable;

use crate::daemon_client::DaemonClient;

/// `zlayer variable list [--scope project_id] [--output table|json]`.
pub async fn list(scope: Option<String>, output: &str) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let vars = client
        .list_variables(scope.as_deref())
        .await
        .context("Failed to list variables")?;

    match output {
        "json" => {
            let json = serde_json::to_string_pretty(&vars)?;
            println!("{json}");
        }
        _ => print_vars_table(&vars),
    }
    Ok(())
}

/// `zlayer variable set <name> <value> [--scope project_id]`.
///
/// Creates a new variable or updates the existing one if a variable with the
/// same `(name, scope)` already exists.
pub async fn set(name: String, value: String, scope: Option<String>) -> Result<()> {
    let client = DaemonClient::connect().await?;

    // Check if a variable with this name+scope already exists
    let existing = client
        .list_variables(scope.as_deref())
        .await
        .context("Failed to list variables")?
        .into_iter()
        .find(|v| v.name == name);

    if let Some(existing_var) = existing {
        // Update
        let var = client
            .update_variable(&existing_var.id, None, Some(&value))
            .await
            .context("Failed to update variable")?;
        let scope_label = var
            .scope
            .as_deref()
            .map_or_else(|| "global".to_string(), |s| format!("scope {s}"));
        println!("Updated variable '{}' ({scope_label})", var.name);
    } else {
        // Create
        let var = client
            .create_variable(&name, &value, scope.as_deref())
            .await
            .context("Failed to create variable")?;
        let scope_label = var
            .scope
            .as_deref()
            .map_or_else(|| "global".to_string(), |s| format!("scope {s}"));
        println!(
            "Created variable '{}' ({scope_label}, id: {})",
            var.name, var.id
        );
    }
    Ok(())
}

/// `zlayer variable get <name> [--scope project_id]`.
///
/// Prints the value of the named variable (just the raw value, no decoration).
pub async fn get(name: String, scope: Option<String>) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let vars = client
        .list_variables(scope.as_deref())
        .await
        .context("Failed to list variables")?;

    let var = vars.into_iter().find(|v| v.name == name).ok_or_else(|| {
        let scope_label = scope
            .as_deref()
            .map_or_else(|| "global".to_string(), |s| format!("scope {s}"));
        anyhow::anyhow!("Variable '{name}' not found ({scope_label})")
    })?;

    println!("{}", var.value);
    Ok(())
}

/// `zlayer variable unset <name> [--scope project_id]`.
///
/// Deletes a variable by name+scope.
pub async fn unset(name: String, scope: Option<String>) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let vars = client
        .list_variables(scope.as_deref())
        .await
        .context("Failed to list variables")?;

    let var = vars.into_iter().find(|v| v.name == name).ok_or_else(|| {
        let scope_label = scope
            .as_deref()
            .map_or_else(|| "global".to_string(), |s| format!("scope {s}"));
        anyhow::anyhow!("Variable '{name}' not found ({scope_label})")
    })?;

    client
        .delete_variable(&var.id)
        .await
        .context("Failed to delete variable")?;

    println!("Deleted variable '{name}'");
    Ok(())
}

/// Pretty-print variables as a plain-text table.
fn print_vars_table(vars: &[StoredVariable]) {
    if vars.is_empty() {
        println!("(no variables)");
        return;
    }
    println!("{:<30} {:<40} {:<20}", "NAME", "VALUE", "SCOPE");
    for var in vars {
        let scope = var.scope.as_deref().unwrap_or("(global)");
        // Truncate long values for table display
        let value = if var.value.len() > 37 {
            format!("{}...", &var.value[..37])
        } else {
            var.value.clone()
        };
        println!("{:<30} {:<40} {:<20}", var.name, value, scope);
    }
}
