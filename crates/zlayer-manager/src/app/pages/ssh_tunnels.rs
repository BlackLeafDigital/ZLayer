//! SSH Tunnels page
//!
//! Displays and manages SSH tunnels with local/remote forwarding.

use leptos::prelude::*;

/// SSH tunnel type
#[derive(Clone, Copy, PartialEq)]
enum TunnelType {
    Local,
    Remote,
}

impl TunnelType {
    fn as_str(self) -> &'static str {
        match self {
            TunnelType::Local => "Local",
            TunnelType::Remote => "Remote",
        }
    }
}

/// Mock tunnel data for display
struct TunnelInfo {
    name: &'static str,
    tunnel_type: TunnelType,
    local_port: u16,
    remote_host: &'static str,
    remote_port: u16,
    status: &'static str,
}

/// Get mock tunnel data
fn mock_tunnels() -> Vec<TunnelInfo> {
    vec![
        TunnelInfo {
            name: "database-tunnel",
            tunnel_type: TunnelType::Local,
            local_port: 5432,
            remote_host: "db.internal",
            remote_port: 5432,
            status: "Active",
        },
        TunnelInfo {
            name: "redis-cache",
            tunnel_type: TunnelType::Local,
            local_port: 6379,
            remote_host: "cache.internal",
            remote_port: 6379,
            status: "Active",
        },
        TunnelInfo {
            name: "dev-server-expose",
            tunnel_type: TunnelType::Remote,
            local_port: 3000,
            remote_host: "0.0.0.0",
            remote_port: 8080,
            status: "Inactive",
        },
        TunnelInfo {
            name: "metrics-forward",
            tunnel_type: TunnelType::Local,
            local_port: 9090,
            remote_host: "prometheus.internal",
            remote_port: 9090,
            status: "Active",
        },
    ]
}

/// SSH Tunnels page component
#[component]
pub fn SshTunnels() -> impl IntoView {
    let tunnels = mock_tunnels();

    view! {
        <div class="container mx-auto p-6">
            <div class="flex justify-between items-center mb-6">
                <h1 class="text-3xl font-bold">SSH Tunnels</h1>
                <button class="btn btn-primary">Create Tunnel</button>
            </div>

            <div class="overflow-x-auto">
                <table class="table table-zebra w-full">
                    <thead>
                        <tr>
                            <th>Name</th>
                            <th>Type</th>
                            <th>Local Port</th>
                            <th>Remote Host:Port</th>
                            <th>Status</th>
                            <th>Actions</th>
                        </tr>
                    </thead>
                    <tbody>
                        {tunnels
                            .into_iter()
                            .map(|tunnel| {
                                let status_class = if tunnel.status == "Active" {
                                    "badge badge-success"
                                } else {
                                    "badge badge-warning"
                                };
                                let type_class = if tunnel.tunnel_type == TunnelType::Local {
                                    "badge badge-info"
                                } else {
                                    "badge badge-secondary"
                                };
                                let remote_addr = format!(
                                    "{}:{}",
                                    tunnel.remote_host,
                                    tunnel.remote_port,
                                );
                                view! {
                                    <tr>
                                        <td class="font-medium">{tunnel.name}</td>
                                        <td>
                                            <span class=type_class>{tunnel.tunnel_type.as_str()}</span>
                                        </td>
                                        <td class="font-mono">{tunnel.local_port}</td>
                                        <td class="font-mono">{remote_addr}</td>
                                        <td>
                                            <span class=status_class>{tunnel.status}</span>
                                        </td>
                                        <td>
                                            <button class="btn btn-sm btn-ghost">Edit</button>
                                            <button class="btn btn-sm btn-ghost text-error">Delete</button>
                                        </td>
                                    </tr>
                                }
                            })
                            .collect::<Vec<_>>()}
                    </tbody>
                </table>
            </div>
        </div>
    }
}
