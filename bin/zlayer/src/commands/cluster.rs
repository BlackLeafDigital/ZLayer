//! Top-level `zlayer cluster <…>` command dispatcher.
//!
//! Wave 5B.1+5B.2 introduced this module to promote cluster-administration
//! commands out of `zlayer node <…>`. Every variant of [`ClusterCommands`]
//! delegates to the same handler the corresponding [`NodeCommands`] variant
//! would call — there is no duplicated business logic, only an alias
//! namespace at the CLI surface. Existing `zlayer node …` invocations
//! continue to dispatch through [`crate::commands::node::handle_node`]
//! unchanged for at least one release (Wave 5B.5 will mark them as
//! deprecated).
//!
//! [`ClusterCommands::RotateSigningKey`] was introduced alongside the
//! Ed25519 join-token rework. Wave 5B.3 added the matching
//! [`NodeCommands::RotateSigningKey`] alias and wired both through
//! [`node::handle_node_rotate_signing_key`], so today both surfaces share
//! one implementation.
//!
//! [`ClusterCommands`]: crate::cli::ClusterCommands
//! [`NodeCommands`]: crate::cli::NodeCommands

use anyhow::Result;

use crate::cli::ClusterCommands;
use crate::commands::node;

/// Dispatch a `zlayer cluster <…>` subcommand to the appropriate handler.
///
/// The handler set is shared with `zlayer node <…>`: each arm forwards to
/// the same `node::handle_node_*` function the equivalent [`NodeCommands`]
/// arm would. Keeping the surfaces aligned this way means there is exactly
/// one implementation of each operation, just two CLI entry points.
///
/// [`NodeCommands`]: crate::cli::NodeCommands
pub(crate) async fn handle_cluster(
    cluster_cmd: &ClusterCommands,
    cli_data_dir: &std::path::Path,
) -> Result<()> {
    match cluster_cmd {
        ClusterCommands::List { output } => {
            node::handle_node_list(output.clone(), cli_data_dir).await
        }
        ClusterCommands::Status { node_id } => {
            node::handle_node_status(node_id.clone(), cli_data_dir).await
        }
        ClusterCommands::Remove { node_id, force } => {
            node::handle_node_remove(node_id.clone(), *force, cli_data_dir).await
        }
        ClusterCommands::SetMode {
            node_id,
            mode,
            services,
        } => {
            node::handle_node_set_mode(
                node_id.clone(),
                mode.clone(),
                services.clone(),
                cli_data_dir,
            )
            .await
        }
        ClusterCommands::Label { node_id, label } => {
            node::handle_node_label(node_id.clone(), label.clone(), cli_data_dir).await
        }
        ClusterCommands::ForceLeader { api_addr } => {
            node::handle_node_force_leader(api_addr.clone()).await
        }
        ClusterCommands::Upgrade {
            version,
            cooldown_secs,
            strict,
            yes,
            skip_leader,
        } => {
            node::handle_node_upgrade(
                cli_data_dir.to_path_buf(),
                version.clone(),
                *cooldown_secs,
                *strict,
                *yes,
                *skip_leader,
            )
            .await
        }
        ClusterCommands::RotateSigningKey { grace } => {
            node::handle_node_rotate_signing_key(*grace).await
        }
        ClusterCommands::RevokeToken {
            token_or_hash,
            reason,
        } => node::handle_cluster_revoke_token(token_or_hash.clone(), reason.clone()).await,
        ClusterCommands::ListRevocations {} => node::handle_cluster_list_revocations().await,
    }
}
