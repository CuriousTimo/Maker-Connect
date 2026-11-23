// Ported from onetun v0.3.10 (github.com/aramperes/onetun, `89c3b59`, src/tunnel/mod.rs), MIT licence,
// Copyright (c) 2023 Aram Peres: see LICENSE-onetun. Changes against onetun are listed in lib.rs.

use std::net::IpAddr;
use std::sync::Arc;

use crate::config::{PortForwardConfig, PortProtocol};
use crate::events::Bus;
use crate::tunnel::tcp::TcpPortPool;
use crate::wg::WireGuardTunnel;

pub mod tcp;

pub async fn port_forward(
    port_forward: PortForwardConfig,
    source_peer_ip: IpAddr,
    tcp_port_pool: TcpPortPool,
    wg: Arc<WireGuardTunnel>,
    bus: Bus,
) -> anyhow::Result<()> {
    info!(
        "Tunneling {} [{}]->[{}] (via [{}] as peer {})",
        port_forward.protocol,
        port_forward.source,
        port_forward.destination,
        &wg.endpoint,
        source_peer_ip
    );

    match port_forward.protocol {
        PortProtocol::Tcp => tcp::tcp_proxy_server(port_forward, tcp_port_pool, bus).await,
        // UDP forwarding is left out (lib.rs, change 2)
        PortProtocol::Udp => anyhow::bail!("UDP forwarding is not part of this port"),
    }
}
