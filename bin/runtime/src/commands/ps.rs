//! `zlayer ps` command -- list deployments, services, and containers.
//!
//! Connects to the daemon via [`DaemonClient`] and displays a summary table
//! (or JSON/YAML) of all deployments and their services.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::daemon_client::DaemonClient;

/// A row in the ps output table.
#[derive(Debug, Serialize, Deserialize)]
struct PsRow {
    deployment: String,
    service: String,
    replicas: String,
    status: String,
    ports: String,
    age: String,
}

/// A container row for the --containers output.
#[derive(Debug, Serialize, Deserialize)]
struct ContainerRow {
    deployment: String,
    service: String,
    container: String,
    replica: u32,
    state: String,
}

/// Execute the `ps` command.
pub(crate) async fn ps(
    deployment_filter: Option<String>,
    show_containers: bool,
    format: &str,
) -> Result<()> {
    info!(
        deployment = ?deployment_filter,
        containers = show_containers,
        format = %format,
        "Listing deployments and services"
    );

    let client = DaemonClient::connect().await?;

    // Fetch deployments
    let deployments: Vec<serde_json::Value> = if let Some(ref name) = deployment_filter {
        // Single deployment -- wrap in a vec
        match client.get_deployment(name).await {
            Ok(d) => vec![d],
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("404") {
                    bail!("Deployment '{}' not found", name);
                }
                return Err(e);
            }
        }
    } else {
        client.list_deployments().await?
    };

    if deployments.is_empty() {
        println!("No deployments found.");
        return Ok(());
    }

    // Build rows for each deployment's services
    let mut ps_rows: Vec<PsRow> = Vec::new();
    let mut container_rows: Vec<ContainerRow> = Vec::new();

    for deploy in &deployments {
        let deploy_name = deploy["name"]
            .as_str()
            .or_else(|| deploy["deployment"].as_str())
            .unwrap_or("unknown")
            .to_string();

        // Fetch services for this deployment
        let services: Vec<serde_json::Value> =
            client.list_services(&deploy_name).await.unwrap_or_default();

        if services.is_empty() {
            // Show the deployment even if it has no services yet
            ps_rows.push(PsRow {
                deployment: deploy_name.clone(),
                service: "-".to_string(),
                replicas: "0/0".to_string(),
                status: "pending".to_string(),
                ports: "-".to_string(),
                age: "-".to_string(),
            });
            continue;
        }

        for svc in &services {
            let svc_name = svc["name"].as_str().unwrap_or("unknown").to_string();
            let replicas = svc["replicas"].as_u64().unwrap_or(0);
            let desired = svc["desired_replicas"].as_u64().unwrap_or(0);
            let status = svc["status"].as_str().unwrap_or("unknown").to_string();

            // Try to get endpoints/ports from service details
            let ports = extract_ports(svc);

            // Age: try created_at or fall back to "-"
            let age = extract_age(deploy);

            ps_rows.push(PsRow {
                deployment: deploy_name.clone(),
                service: svc_name.clone(),
                replicas: format!("{}/{}", replicas, desired),
                status,
                ports,
                age,
            });

            // If --containers requested, fetch container info
            if show_containers {
                match client.list_containers(&deploy_name, &svc_name).await {
                    Ok(containers) => {
                        for c in &containers {
                            container_rows.push(ContainerRow {
                                deployment: deploy_name.clone(),
                                service: svc_name.clone(),
                                container: c["id"].as_str().unwrap_or("unknown").to_string(),
                                replica: c["replica"].as_u64().unwrap_or(0) as u32,
                                state: c["state"].as_str().unwrap_or("unknown").to_string(),
                            });
                        }
                    }
                    Err(_) => {
                        // Containers endpoint may not be available; silently skip
                    }
                }
            }
        }
    }

    // Output in the requested format
    match format {
        "json" => print_json(&ps_rows, &container_rows, show_containers)?,
        "yaml" => print_yaml(&ps_rows, &container_rows, show_containers)?,
        _ => print_table(&ps_rows, &container_rows, show_containers),
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Output formatters
// ---------------------------------------------------------------------------

fn print_table(rows: &[PsRow], container_rows: &[ContainerRow], show_containers: bool) {
    if rows.is_empty() {
        println!("No deployments found.");
        return;
    }

    // Calculate column widths
    let hdr = [
        "DEPLOYMENT",
        "SERVICE",
        "REPLICAS",
        "STATUS",
        "PORTS",
        "AGE",
    ];
    let mut widths: Vec<usize> = hdr.iter().map(|h| h.len()).collect();

    for r in rows {
        widths[0] = widths[0].max(r.deployment.len());
        widths[1] = widths[1].max(r.service.len());
        widths[2] = widths[2].max(r.replicas.len());
        widths[3] = widths[3].max(r.status.len());
        widths[4] = widths[4].max(r.ports.len());
        widths[5] = widths[5].max(r.age.len());
    }

    // Print header
    println!(
        "{:<w0$}  {:<w1$}  {:<w2$}  {:<w3$}  {:<w4$}  {:<w5$}",
        hdr[0],
        hdr[1],
        hdr[2],
        hdr[3],
        hdr[4],
        hdr[5],
        w0 = widths[0],
        w1 = widths[1],
        w2 = widths[2],
        w3 = widths[3],
        w4 = widths[4],
        w5 = widths[5],
    );

    // Print rows
    for r in rows {
        println!(
            "{:<w0$}  {:<w1$}  {:<w2$}  {:<w3$}  {:<w4$}  {:<w5$}",
            r.deployment,
            r.service,
            r.replicas,
            r.status,
            r.ports,
            r.age,
            w0 = widths[0],
            w1 = widths[1],
            w2 = widths[2],
            w3 = widths[3],
            w4 = widths[4],
            w5 = widths[5],
        );
    }

    // Print containers if requested
    if show_containers && !container_rows.is_empty() {
        println!();

        let chdr = ["DEPLOYMENT", "SERVICE", "CONTAINER", "REPLICA", "STATE"];
        let mut cwidths: Vec<usize> = chdr.iter().map(|h| h.len()).collect();

        for c in container_rows {
            cwidths[0] = cwidths[0].max(c.deployment.len());
            cwidths[1] = cwidths[1].max(c.service.len());
            cwidths[2] = cwidths[2].max(c.container.len());
            cwidths[3] = cwidths[3].max(c.replica.to_string().len());
            cwidths[4] = cwidths[4].max(c.state.len());
        }

        println!(
            "{:<w0$}  {:<w1$}  {:<w2$}  {:<w3$}  {:<w4$}",
            chdr[0],
            chdr[1],
            chdr[2],
            chdr[3],
            chdr[4],
            w0 = cwidths[0],
            w1 = cwidths[1],
            w2 = cwidths[2],
            w3 = cwidths[3],
            w4 = cwidths[4],
        );

        for c in container_rows {
            println!(
                "{:<w0$}  {:<w1$}  {:<w2$}  {:<w3$}  {:<w4$}",
                c.deployment,
                c.service,
                c.container,
                c.replica,
                c.state,
                w0 = cwidths[0],
                w1 = cwidths[1],
                w2 = cwidths[2],
                w3 = cwidths[3],
                w4 = cwidths[4],
            );
        }
    }
}

fn print_json(
    rows: &[PsRow],
    container_rows: &[ContainerRow],
    show_containers: bool,
) -> Result<()> {
    let output = if show_containers {
        serde_json::json!({
            "services": rows,
            "containers": container_rows,
        })
    } else {
        serde_json::json!({
            "services": rows,
        })
    };
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn print_yaml(
    rows: &[PsRow],
    container_rows: &[ContainerRow],
    show_containers: bool,
) -> Result<()> {
    let output = if show_containers {
        serde_json::json!({
            "services": rows,
            "containers": container_rows,
        })
    } else {
        serde_json::json!({
            "services": rows,
        })
    };
    println!("{}", serde_yaml::to_string(&output)?);
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract port information from a service JSON value.
fn extract_ports(svc: &serde_json::Value) -> String {
    if let Some(endpoints) = svc.get("endpoints").and_then(|e| e.as_array()) {
        let ports: Vec<String> = endpoints
            .iter()
            .filter_map(|ep| {
                let port = ep.get("port")?.as_u64()?;
                let proto = ep.get("protocol").and_then(|p| p.as_str()).unwrap_or("tcp");
                Some(format!("{}/{}", port, proto))
            })
            .collect();
        if ports.is_empty() {
            "-".to_string()
        } else {
            ports.join(", ")
        }
    } else {
        "-".to_string()
    }
}

/// Extract age from a deployment JSON value.
fn extract_age(deploy: &serde_json::Value) -> String {
    if let Some(created) = deploy.get("created_at").and_then(|c| c.as_str()) {
        if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(created) {
            let now = chrono::Utc::now();
            let duration = now.signed_duration_since(ts);
            return format_duration(duration);
        }
    }
    "-".to_string()
}

/// Format a chrono::Duration into a human-readable age string.
fn format_duration(d: chrono::Duration) -> String {
    let secs = d.num_seconds();
    if secs < 0 {
        return "0s".to_string();
    }
    if secs < 60 {
        return format!("{}s", secs);
    }
    let mins = d.num_minutes();
    if mins < 60 {
        return format!("{}m", mins);
    }
    let hours = d.num_hours();
    if hours < 24 {
        return format!("{}h", hours);
    }
    let days = d.num_days();
    format!("{}d", days)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_duration_seconds() {
        let d = chrono::Duration::seconds(30);
        assert_eq!(format_duration(d), "30s");
    }

    #[test]
    fn test_format_duration_minutes() {
        let d = chrono::Duration::minutes(5);
        assert_eq!(format_duration(d), "5m");
    }

    #[test]
    fn test_format_duration_hours() {
        let d = chrono::Duration::hours(3);
        assert_eq!(format_duration(d), "3h");
    }

    #[test]
    fn test_format_duration_days() {
        let d = chrono::Duration::days(7);
        assert_eq!(format_duration(d), "7d");
    }

    #[test]
    fn test_format_duration_negative() {
        let d = chrono::Duration::seconds(-10);
        assert_eq!(format_duration(d), "0s");
    }

    #[test]
    fn test_extract_ports_empty() {
        let svc = serde_json::json!({});
        assert_eq!(extract_ports(&svc), "-");
    }

    #[test]
    fn test_extract_ports_with_endpoints() {
        let svc = serde_json::json!({
            "endpoints": [
                {"port": 8080, "protocol": "http"},
                {"port": 5432, "protocol": "tcp"}
            ]
        });
        assert_eq!(extract_ports(&svc), "8080/http, 5432/tcp");
    }

    #[test]
    fn test_extract_ports_empty_array() {
        let svc = serde_json::json!({ "endpoints": [] });
        assert_eq!(extract_ports(&svc), "-");
    }

    #[test]
    fn test_extract_age_no_created_at() {
        let deploy = serde_json::json!({});
        assert_eq!(extract_age(&deploy), "-");
    }

    #[test]
    fn test_extract_age_invalid_date() {
        let deploy = serde_json::json!({"created_at": "not-a-date"});
        assert_eq!(extract_age(&deploy), "-");
    }

    #[test]
    fn test_ps_row_serialization() {
        let row = PsRow {
            deployment: "my-app".to_string(),
            service: "web".to_string(),
            replicas: "3/3".to_string(),
            status: "running".to_string(),
            ports: "8080/http".to_string(),
            age: "5m".to_string(),
        };
        let json = serde_json::to_string(&row).unwrap();
        assert!(json.contains("my-app"));
        assert!(json.contains("running"));
    }

    #[test]
    fn test_container_row_serialization() {
        let row = ContainerRow {
            deployment: "my-app".to_string(),
            service: "web".to_string(),
            container: "web-rep-1".to_string(),
            replica: 1,
            state: "running".to_string(),
        };
        let json = serde_json::to_string(&row).unwrap();
        assert!(json.contains("web-rep-1"));
    }

    #[test]
    fn test_print_table_empty() {
        // Should not panic with empty rows
        print_table(&[], &[], false);
    }

    #[test]
    fn test_print_table_with_rows() {
        let rows = vec![
            PsRow {
                deployment: "my-app".to_string(),
                service: "web".to_string(),
                replicas: "3/3".to_string(),
                status: "running".to_string(),
                ports: "8080/http".to_string(),
                age: "5m".to_string(),
            },
            PsRow {
                deployment: "my-app".to_string(),
                service: "api".to_string(),
                replicas: "2/2".to_string(),
                status: "running".to_string(),
                ports: "3000/http".to_string(),
                age: "5m".to_string(),
            },
        ];
        // Should not panic
        print_table(&rows, &[], false);
    }

    #[test]
    fn test_print_table_with_containers() {
        let rows = vec![PsRow {
            deployment: "my-app".to_string(),
            service: "web".to_string(),
            replicas: "2/2".to_string(),
            status: "running".to_string(),
            ports: "8080/http".to_string(),
            age: "5m".to_string(),
        }];
        let containers = vec![
            ContainerRow {
                deployment: "my-app".to_string(),
                service: "web".to_string(),
                container: "web-rep-1".to_string(),
                replica: 1,
                state: "running".to_string(),
            },
            ContainerRow {
                deployment: "my-app".to_string(),
                service: "web".to_string(),
                container: "web-rep-2".to_string(),
                replica: 2,
                state: "running".to_string(),
            },
        ];
        // Should not panic
        print_table(&rows, &containers, true);
    }

    #[test]
    fn test_print_json_no_containers() {
        let rows = vec![PsRow {
            deployment: "my-app".to_string(),
            service: "web".to_string(),
            replicas: "1/1".to_string(),
            status: "running".to_string(),
            ports: "-".to_string(),
            age: "-".to_string(),
        }];
        assert!(print_json(&rows, &[], false).is_ok());
    }

    #[test]
    fn test_print_yaml_no_containers() {
        let rows = vec![PsRow {
            deployment: "my-app".to_string(),
            service: "web".to_string(),
            replicas: "1/1".to_string(),
            status: "running".to_string(),
            ports: "-".to_string(),
            age: "-".to_string(),
        }];
        assert!(print_yaml(&rows, &[], false).is_ok());
    }
}
