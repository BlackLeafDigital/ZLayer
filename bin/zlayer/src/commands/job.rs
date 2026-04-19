//! `zlayer job` subcommand handlers.
//!
//! Dispatches to the daemon's job and cron management endpoints via
//! [`DaemonClient`].

use anyhow::Result;

use crate::cli::{Cli, CronCommands, JobCommands};
use zlayer_client::DaemonClient;

/// Entry point for `zlayer job <subcommand>`.
pub(crate) async fn handle_job(_cli: &Cli, cmd: &JobCommands) -> Result<()> {
    match cmd {
        JobCommands::Ls { output } => list_jobs(output).await,
        JobCommands::Trigger { deployment, job } => trigger_job(deployment.as_deref(), job).await,
        JobCommands::Status { deployment, job } => get_job_status(deployment.as_deref(), job).await,
        JobCommands::Cron(cron_cmd) => handle_cron(cron_cmd).await,
    }
}

async fn list_jobs(output: &str) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let jobs = client.list_jobs().await?;

    if output == "json" {
        let json = serde_json::to_string_pretty(&jobs)?;
        println!("{json}");
    } else {
        // Table output
        if let Some(arr) = jobs.as_array() {
            println!("{:<30} {:<20} {:<15}", "NAME", "STATUS", "TRIGGER");
            for job in arr {
                let name = job["job_name"]
                    .as_str()
                    .unwrap_or(job["name"].as_str().unwrap_or("-"));
                let status = job["status"].as_str().unwrap_or("-");
                let trigger = job["trigger"].as_str().unwrap_or("-");
                println!("{name:<30} {status:<20} {trigger:<15}");
            }
            if arr.is_empty() {
                println!("(no jobs)");
            }
        } else {
            let json = serde_json::to_string_pretty(&jobs)?;
            println!("{json}");
        }
    }
    Ok(())
}

async fn trigger_job(deployment: Option<&str>, job: &str) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let deployment = crate::commands::resolver::resolve_deployment(&client, deployment).await?;
    let resp = client.trigger_job(&deployment, job).await?;

    if let Some(exec_id) = resp.get("execution_id").and_then(|v| v.as_str()) {
        println!("Triggered job '{job}' in deployment '{deployment}'");
        println!("Execution ID: {exec_id}");
    } else {
        let json = serde_json::to_string_pretty(&resp)?;
        println!("{json}");
    }
    Ok(())
}

async fn get_job_status(deployment: Option<&str>, job: &str) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let deployment = crate::commands::resolver::resolve_deployment(&client, deployment).await?;
    let resp = client.get_job_status(&deployment, job).await?;
    let json = serde_json::to_string_pretty(&resp)?;
    println!("{json}");
    Ok(())
}

async fn handle_cron(cmd: &CronCommands) -> Result<()> {
    match cmd {
        CronCommands::Ls { output } => list_cron_jobs(output).await,
        CronCommands::Status { deployment, cron } => {
            get_cron_status(deployment.as_deref(), cron).await
        }
    }
}

async fn list_cron_jobs(output: &str) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let crons = client.list_cron_jobs().await?;

    if output == "json" {
        let json = serde_json::to_string_pretty(&crons)?;
        println!("{json}");
    } else if let Some(arr) = crons.as_array() {
        println!(
            "{:<30} {:<25} {:<10} {:<25} {:<25}",
            "NAME", "SCHEDULE", "ENABLED", "LAST RUN", "NEXT RUN"
        );
        for cron in arr {
            let name = cron["name"].as_str().unwrap_or("-");
            let schedule = cron["schedule"].as_str().unwrap_or("-");
            let enabled = if cron["enabled"].as_bool().unwrap_or(false) {
                "yes"
            } else {
                "no"
            };
            let last_run = cron["last_run"].as_str().unwrap_or("-");
            let next_run = cron["next_run"].as_str().unwrap_or("-");
            println!("{name:<30} {schedule:<25} {enabled:<10} {last_run:<25} {next_run:<25}");
        }
        if arr.is_empty() {
            println!("(no cron jobs)");
        }
    } else {
        let json = serde_json::to_string_pretty(&crons)?;
        println!("{json}");
    }
    Ok(())
}

async fn get_cron_status(deployment: Option<&str>, cron: &str) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let deployment = crate::commands::resolver::resolve_deployment(&client, deployment).await?;
    let resp = client.get_cron_status(&deployment, cron).await?;
    let json = serde_json::to_string_pretty(&resp)?;
    println!("{json}");
    Ok(())
}
