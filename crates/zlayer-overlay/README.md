# zlayer-overlay

Encrypted overlay networking for ZLayer using boringtun (userspace WireGuard) with built-in DNS service discovery.

## Features

- **Encrypted Mesh** - Peer-to-peer networking via boringtun (no kernel WireGuard module required)
- **IP Allocation** - Automatic CIDR-based IP management for overlay nodes
- **DNS Service Discovery** - Auto-register peers with DNS names for easy discovery
- **Health Checking** - Monitor peer connectivity via handshake times and ping
- **Bootstrap Protocol** - Leader/worker initialization with persistent state

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
zlayer-overlay = "0.8"
```

## Quick Start

### Initialize a Leader Node

```rust
use zlayer_overlay::OverlayBootstrap;
use std::path::Path;

// Initialize as cluster leader
let mut bootstrap = OverlayBootstrap::init_leader(
    "10.200.0.0/16",              // Overlay CIDR
    51820,                         // Overlay port
    Path::new("/var/lib/zlayer"),  // State directory
).await?;

// Start the overlay network
bootstrap.start().await?;

println!("Overlay IP: {}", bootstrap.node_ip());      // 10.200.0.1
println!("Public key: {}", bootstrap.public_key());   // Overlay pubkey
```

### Join an Existing Overlay

```rust
let mut bootstrap = OverlayBootstrap::join(
    "10.200.0.0/16",           // Leader's CIDR
    "192.168.1.100:51820",     // Leader's public endpoint
    "leader_public_key",       // Leader's WG public key
    "10.200.0.1".parse()?,     // Leader's overlay IP
    "10.200.0.5".parse()?,     // IP allocated for this node
    51820,                      // Our listen port
    Path::new("/var/lib/zlayer"),
).await?;

bootstrap.start().await?;
```

## DNS Service Discovery

Enable automatic DNS registration for overlay peers:

```rust
let mut bootstrap = OverlayBootstrap::init_leader(cidr, port, data_dir)
    .await?
    .with_dns("overlay.local.", 15353)?;  // Zone and port

bootstrap.start().await?;
```

### Auto-Generated DNS Names

When DNS is enabled, peers are automatically registered:

| IP Address | DNS Name | Description |
|------------|----------|-------------|
| `10.200.0.1` | `node-0-1.overlay.local` | IP-based hostname |
| `10.200.0.1` | `leader.overlay.local` | Leader alias (leader only) |
| `10.200.0.5` | `node-0-5.overlay.local` | Worker node |

### Custom Hostnames

Peers can have custom DNS names in addition to the auto-generated ones:

```rust
let peer = PeerConfig::new(
    "web-server-1".to_string(),
    public_key,
    endpoint,
    overlay_ip,
).with_hostname("web");  // Registers as web.overlay.local

bootstrap.add_peer(peer).await?;
```

### Querying DNS

```bash
# Query from any node in the overlay
dig @10.200.0.1 -p 15353 leader.overlay.local
dig @10.200.0.1 -p 15353 node-0-5.overlay.local
dig @10.200.0.1 -p 15353 web.overlay.local
```

### Default Port

DNS uses port **15353** by default to avoid conflicts with system DNS resolvers (systemd-resolved typically binds to 53).

## Health Checking

Monitor peer connectivity:

```rust
use zlayer_overlay::OverlayHealthChecker;
use std::time::Duration;

let checker = OverlayHealthChecker::new("zl-overlay0", Duration::from_secs(30));

// Single check
let health = checker.check_all().await?;
println!("Healthy: {}/{}", health.healthy_peers, health.total_peers);

// Continuous monitoring with callbacks
checker.run(|public_key, healthy| {
    println!("Peer {} is now {}", public_key, if healthy { "UP" } else { "DOWN" });
}).await;
```

## API Reference

### Core Types

| Type | Description |
|------|-------------|
| `OverlayBootstrap` | Main bootstrap manager for overlay lifecycle |
| `PeerConfig` | Configuration for a peer node |
| `DnsServer` | DNS server for service discovery |
| `DnsHandle` | Handle for managing DNS records after server starts |
| `DnsConfig` | DNS configuration (zone, port, bind address) |
| `OverlayHealthChecker` | Peer health monitoring |
| `IpAllocator` | CIDR-based IP address allocation |

### Constants

| Constant | Value | Description |
|----------|-------|-------------|
| `DEFAULT_WG_PORT` | 51820 | Default overlay listen port |
| `DEFAULT_DNS_PORT` | 15353 | Default DNS server port |
| `DEFAULT_OVERLAY_CIDR` | 10.200.0.0/16 | Default overlay network |
| `DEFAULT_INTERFACE_NAME` | zl-overlay0 | Default overlay interface |

## License

MIT - See [LICENSE](../../LICENSE) for details.
