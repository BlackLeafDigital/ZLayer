//! `zlayer task` subcommand handlers.
//!
//! Manages named runnable scripts (tasks) that can be executed on demand.
//! Tasks can be global or project-scoped. When run, the task body is
//! executed as a subprocess and stdout/stderr are captured.
//!
//! Dispatches to the daemon's task endpoints
//! (`GET/POST/DELETE /api/v1/tasks[/id[/run|runs]]`) via [`DaemonClient`].

use anyhow::{Context, Result};
use zlayer_api::storage::{StoredTask, TaskRun};

use crate::daemon_client::DaemonClient;

/// `zlayer task list [--project project_id] [--output table|json]`.
pub async fn list(project_id: Option<String>, output: &str) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let tasks = client
        .list_tasks(project_id.as_deref())
        .await
        .context("Failed to list tasks")?;

    match output {
        "json" => {
            let json = serde_json::to_string_pretty(&tasks)?;
            println!("{json}");
        }
        _ => print_tasks_table(&tasks),
    }
    Ok(())
}

/// `zlayer task create --name <name> --kind bash --body <script> [--project <id>]`.
pub async fn create(
    name: String,
    kind: String,
    body: String,
    project_id: Option<String>,
) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let task = client
        .create_task(&name, &kind, &body, project_id.as_deref())
        .await
        .context("Failed to create task")?;

    let scope = task
        .project_id
        .as_deref()
        .map_or_else(|| "global".to_string(), |p| format!("project {p}"));
    println!(
        "Created task '{}' ({scope}, kind: {}, id: {})",
        task.name, task.kind, task.id
    );
    Ok(())
}

/// `zlayer task run <id>`.
///
/// Executes the task synchronously and prints stdout/stderr + exit code.
pub async fn run(id: String) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let task_run = client.run_task(&id).await.context("Failed to run task")?;

    print_run_output(&task_run);
    Ok(())
}

/// `zlayer task logs <id>`.
///
/// Lists past runs and prints the last run's output.
pub async fn logs(id: String) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let runs = client
        .list_task_runs(&id)
        .await
        .context("Failed to list task runs")?;

    if runs.is_empty() {
        println!("(no runs recorded for task {id})");
        return Ok(());
    }

    // runs are ordered most-recent-first
    let last = &runs[0];
    print_run_output(last);
    Ok(())
}

/// `zlayer task delete <id> [-y]`.
pub async fn delete(id: String, yes: bool) -> Result<()> {
    if !yes {
        eprint!("Delete task {id}? [y/N] ");
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !answer.trim().eq_ignore_ascii_case("y") {
            println!("Cancelled");
            return Ok(());
        }
    }

    let client = DaemonClient::connect().await?;
    client
        .delete_task(&id)
        .await
        .context("Failed to delete task")?;

    println!("Deleted task {id}");
    Ok(())
}

/// Pretty-print tasks as a plain-text table.
fn print_tasks_table(tasks: &[StoredTask]) {
    if tasks.is_empty() {
        println!("(no tasks)");
        return;
    }
    println!(
        "{:<36} {:<20} {:<8} {:<20}",
        "ID", "NAME", "KIND", "PROJECT"
    );
    for task in tasks {
        let project = task.project_id.as_deref().unwrap_or("(global)");
        // Truncate name for display
        let name = if task.name.len() > 18 {
            format!("{}...", &task.name[..18])
        } else {
            task.name.clone()
        };
        println!(
            "{:<36} {:<20} {:<8} {:<20}",
            task.id, name, task.kind, project
        );
    }
}

/// Print a task run's output to stdout.
fn print_run_output(run: &TaskRun) {
    if !run.stdout.is_empty() {
        print!("{}", run.stdout);
        if !run.stdout.ends_with('\n') {
            println!();
        }
    }
    if !run.stderr.is_empty() {
        eprint!("{}", run.stderr);
        if !run.stderr.ends_with('\n') {
            eprintln!();
        }
    }
    match run.exit_code {
        Some(code) => println!("[exit code: {code}]"),
        None => println!("[exit code: unknown]"),
    }
}
