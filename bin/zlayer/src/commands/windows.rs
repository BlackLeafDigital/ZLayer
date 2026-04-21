//! `zlayer windows` subcommand handlers (WSL2 maintenance).

#![cfg(all(target_os = "windows", feature = "wsl"))]

use crate::cli::WindowsCommands;

/// Dispatch the `zlayer windows <subcommand>` tree.
///
/// # Errors
///
/// Returns an error if the underlying maintenance routine fails.
pub async fn handle(cmd: &WindowsCommands) -> anyhow::Result<()> {
    match cmd {
        WindowsCommands::Compact { force } => handle_compact(*force).await,
    }
}

async fn handle_compact(force: bool) -> anyhow::Result<()> {
    if force {
        tracing::debug!("`--force` set on `zlayer windows compact` (currently informational)");
    }

    println!("Compacting ZLayer WSL2 distro... (this may take a minute)");

    let report = zlayer_wsl::compact::compact_distro().await?;

    print_report(&report);
    Ok(())
}

fn print_report(report: &zlayer_wsl::compact::CompactReport) {
    println!("\nCompaction complete:");
    println!("  vhdx:   {}", report.vhdx_path.display());
    println!("  method: {:?}", report.method);
    println!(
        "  daemon: {}",
        if report.daemon_was_running {
            "was running (restart with `zlayer serve` to resume)"
        } else {
            "was not running"
        }
    );

    match (report.size_before_bytes, report.size_after_bytes) {
        (Some(before), Some(after)) => {
            let freed = before.saturating_sub(after);
            println!(
                "  size:   {} -> {} ({} freed)",
                fmt_bytes(before),
                fmt_bytes(after),
                fmt_bytes(freed)
            );
        }
        _ => {
            println!("  size:   (stat unavailable)");
        }
    }
}

fn fmt_bytes(n: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}
