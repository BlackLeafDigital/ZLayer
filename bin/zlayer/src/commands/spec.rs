use anyhow::{Context, Result};

use crate::cli::SpecCommands;

/// Handle spec commands
pub(crate) async fn handle_spec(action: &SpecCommands) -> Result<()> {
    match action {
        SpecCommands::Dump { spec, format } => {
            let spec = spec.clone();
            let format = format.clone();
            let content =
                std::fs::read_to_string(&spec).context("Failed to read specification file")?;

            let parsed_spec =
                zlayer_spec::from_yaml_str(&content).context("Failed to parse specification")?;

            match format.to_lowercase().as_str() {
                "json" => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&parsed_spec)
                            .context("Failed to serialize as JSON")?
                    );
                }
                _ => {
                    println!(
                        "{}",
                        serde_yaml::to_string(&parsed_spec)
                            .context("Failed to serialize as YAML")?
                    );
                }
            }
            Ok(())
        }
    }
}
