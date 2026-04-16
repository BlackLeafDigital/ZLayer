//! `zlayer workflow` subcommand handlers.
//!
//! Manages workflows — named DAGs of steps that compose tasks, project builds,
//! deploys, and sync applies. Steps execute sequentially; if a step fails,
//! the optional `on_failure` handler runs before aborting.
//!
//! Dispatches to the daemon's workflow endpoints
//! (`GET/POST/DELETE /api/v1/workflows[/id[/run|runs]]`) via [`DaemonClient`].

use anyhow::{Context, Result};
use zlayer_api::storage::{StoredWorkflow, WorkflowRun};

use crate::daemon_client::DaemonClient;

/// `zlayer workflow list [--output table|json]`.
pub async fn list(output: &str) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let workflows = client
        .list_workflows()
        .await
        .context("Failed to list workflows")?;

    match output {
        "json" => {
            let json = serde_json::to_string_pretty(&workflows)?;
            println!("{json}");
        }
        _ => print_workflows_table(&workflows),
    }
    Ok(())
}

/// `zlayer workflow create --name <name> --steps <json> [--project <id>]`.
pub async fn create(name: String, steps_json: String, project_id: Option<String>) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let workflow = client
        .create_workflow(&name, &steps_json, project_id.as_deref())
        .await
        .context("Failed to create workflow")?;

    let scope = workflow
        .project_id
        .as_deref()
        .map_or_else(|| "global".to_string(), |p| format!("project {p}"));
    println!(
        "Created workflow '{}' ({scope}, {} step(s), id: {})",
        workflow.name,
        workflow.steps.len(),
        workflow.id
    );
    Ok(())
}

/// `zlayer workflow run <id>`.
///
/// Executes the workflow synchronously and prints step results.
pub async fn run(id: String) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let wf_run = client
        .run_workflow(&id)
        .await
        .context("Failed to run workflow")?;

    print_run_output(&wf_run);
    Ok(())
}

/// `zlayer workflow logs <id>`.
///
/// Lists past runs and prints the last run's step results.
pub async fn logs(id: String) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let runs = client
        .list_workflow_runs(&id)
        .await
        .context("Failed to list workflow runs")?;

    if runs.is_empty() {
        println!("(no runs recorded for workflow {id})");
        return Ok(());
    }

    // runs are ordered most-recent-first
    let last = &runs[0];
    print_run_output(last);
    Ok(())
}

/// `zlayer workflow delete <id> [-y]`.
pub async fn delete(id: String, yes: bool) -> Result<()> {
    if !yes {
        eprint!("Delete workflow {id}? [y/N] ");
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !answer.trim().eq_ignore_ascii_case("y") {
            println!("Cancelled");
            return Ok(());
        }
    }

    let client = DaemonClient::connect().await?;
    client
        .delete_workflow(&id)
        .await
        .context("Failed to delete workflow")?;

    println!("Deleted workflow {id}");
    Ok(())
}

/// Pretty-print workflows as a plain-text table.
fn print_workflows_table(workflows: &[StoredWorkflow]) {
    if workflows.is_empty() {
        println!("(no workflows)");
        return;
    }
    println!(
        "{:<36} {:<24} {:<6} {:<20}",
        "ID", "NAME", "STEPS", "PROJECT"
    );
    for wf in workflows {
        let project = wf.project_id.as_deref().unwrap_or("(global)");
        // Truncate name for display
        let name = if wf.name.len() > 22 {
            format!("{}...", &wf.name[..22])
        } else {
            wf.name.clone()
        };
        println!(
            "{:<36} {:<24} {:<6} {:<20}",
            wf.id,
            name,
            wf.steps.len(),
            project
        );
    }
}

/// Print a workflow run's step results to stdout.
fn print_run_output(run: &WorkflowRun) {
    println!("Run {} — status: {}", run.id, run.status);
    println!();
    for result in &run.step_results {
        let output_str = result.output.as_deref().unwrap_or("");
        let truncated = if output_str.len() > 80 {
            format!("{}...", &output_str[..80])
        } else {
            output_str.to_string()
        };
        println!(
            "  [{:>7}] {}: {}",
            result.status, result.step_name, truncated
        );
    }
    if let Some(ref finished) = run.finished_at {
        let duration = *finished - run.started_at;
        println!();
        println!("Duration: {}ms", duration.num_milliseconds());
    }
}
