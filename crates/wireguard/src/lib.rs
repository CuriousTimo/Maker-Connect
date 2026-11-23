//! WireGuard inside the agent's own process (#194): a local TCP port forwarded through a WireGuard
//! tunnel to an address on the far side, with no VPN app, no TUN device and no admin rights.
//!
//! - The WireGuard protocol: **boringtun 0.7.1** (Cloudflare, BSD-3-Clause), used as a dependency.
//! - The TCP/IP stack inside the process: **smoltcp 0.12**, used as a dependency.
//! - The glue between them: **ported from onetun v0.3.10** (github.com/aramperes/onetun, `89c3b59`,
//!   MIT, Copyright (c) 2023 Aram Peres; LICENSE-onetun), file by file: `lib.rs`, `config.rs`,
//!   `events.rs`, `wg.rs`, `virtual_device.rs`, `virtual_iface/{mod,tcp}.rs`, `tunnel/{mod,tcp}.rs`.
//!
//! Owner, 2026-11-23 (#194): boringtun as a dependency ("cloudflare ontwikkelt daar actief aan"),
//! the glue ported from onetun.
//!
//! **Changes against onetun v0.3.10**, each one:
//! 1. boringtun 0.7.1 instead of onetun's 0.6.0: `Tunn::new` returns the tunnel itself, not a
//!    `Result` (boringtun `src/noise/mod.rs:194-201`), so `wg.rs`'s `create_tunnel` wraps it in `Ok`.
//! 2. Left out, the agent talks TCP only: UDP forwarding (`tunnel/udp.rs`, `virtual_iface/udp.rs`,
//!    the UDP port pool and device in `start_tunnels`), packet capture (`pcap.rs`), and the
//!    command line (`main.rs`, `Config::from_args`, which is onetun's `bin` feature only, with the
//!    helpers only it used: `parse_ip`, `parse_keep_alive`, `parse_mtu`,
//!    `is_file_insecurely_readable`).
//! 3. `config.rs`: `parse_addr`, `parse_private_key`, `parse_public_key`, `parse_preshared_key`
//!    and `DEFAULT_PORT_FORWARD_SOURCE` made `pub`, for the agent's settings (same code).
//! 4. `start_tunnels` returns the WireGuard tunnel (onetun returns `()`), and `WireGuardTunnel`
//!    has `stats`, which only calls boringtun's `Tunn::stats` (`src/noise/mod.rs:577`): for the
//!    agent's card. `config.rs` has `default_bind_addr`, the bind address onetun's `from_args`
//!    picks when none is given (`config.rs:254-259`), as a function.
//! 5. Edition 2021 as onetun; formatted with rustfmt (layout only); the lints onetun's own code
//!    trips under our clippy are allowed at the top of this file, none of its code changed for them.
//!
//! 6. `Cargo.toml`: smoltcp without `socket-udp` (onetun has it, for the UDP forwarding left out in
//!    change 2); the rest of its features as onetun's.
//!
//! Not on the agent's card, and why: the allowed IPs (the tunnel carries only onetun's one forward,
//! to the hub's address) and the MTU (onetun's 1420).
//!
//! onetun's defaults hold: MTU 1420 (`config.rs:111`), keepalive only when set (`config.rs:24`),
//! the endpoint's name resolved once when the tunnel starts (`parse_addr`).

// onetun's own code, unchanged, under our lints (lib.rs, change 5)
#![allow(clippy::useless_borrows_in_formatting)]

#[macro_use]
extern crate log;

use std::sync::Arc;

use anyhow::Context;

use crate::config::{Config, PortProtocol};
use crate::events::Bus;
use crate::tunnel::tcp::TcpPortPool;
use crate::virtual_device::VirtualIpDevice;
use crate::virtual_iface::tcp::TcpVirtualInterface;
use crate::virtual_iface::VirtualInterfacePoll;
use crate::wg::WireGuardTunnel;

pub mod config;
pub mod events;
pub mod tunnel;
pub mod virtual_device;
pub mod virtual_iface;
pub mod wg;

/// Starts the onetun tunnels in separate tokio tasks.
///
/// Note: This future completes immediately.
/// Returns the WireGuard tunnel, for its `stats` (lib.rs, change 4).
pub async fn start_tunnels(config: Config, bus: Bus) -> anyhow::Result<Arc<WireGuardTunnel>> {
    // Initialize the port pool for each protocol
    let tcp_port_pool = TcpPortPool::new();

    let wg = WireGuardTunnel::new(&config, bus.clone())
        .await
        .context("Failed to initialize WireGuard tunnel")?;
    let wg = Arc::new(wg);
    let tunnel = wg.clone();

    {
        // Start routine task for WireGuard
        let wg = wg.clone();
        tokio::spawn(async move { wg.routine_task().await });
    }

    {
        // Start consumption task for WireGuard
        let wg = wg.clone();
        tokio::spawn(Box::pin(async move { wg.consume_task().await }));
    }

    {
        // Start production task for WireGuard
        let wg = wg.clone();
        tokio::spawn(async move { wg.produce_task().await });
    }

    if config
        .port_forwards
        .iter()
        .any(|pf| pf.protocol == PortProtocol::Tcp)
    {
        // TCP device
        let bus = bus.clone();
        let device =
            VirtualIpDevice::new(PortProtocol::Tcp, bus.clone(), config.max_transmission_unit);

        // Start TCP Virtual Interface
        let port_forwards = config.port_forwards.clone();
        let iface = TcpVirtualInterface::new(port_forwards, bus, config.source_peer_ip);
        tokio::spawn(async move { iface.poll_loop(device).await });
    }

    {
        let port_forwards = config.port_forwards;
        let source_peer_ip = config.source_peer_ip;

        port_forwards
            .into_iter()
            .map(|pf| (pf, wg.clone(), tcp_port_pool.clone(), bus.clone()))
            .for_each(move |(pf, wg, tcp_port_pool, bus)| {
                tokio::spawn(async move {
                    tunnel::port_forward(pf, source_peer_ip, tcp_port_pool, wg, bus)
                        .await
                        .unwrap_or_else(|e| error!("Port-forward failed for {} : {}", pf, e))
                });
            });
    }

    Ok(tunnel)
}
