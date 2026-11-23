// Ported from onetun v0.3.10 (github.com/aramperes/onetun, `89c3b59`, src/config.rs), MIT licence,
// Copyright (c) 2023 Aram Peres: see LICENSE-onetun. Changes against onetun are listed in lib.rs.

use std::convert::TryFrom;
use std::fmt::{Display, Formatter};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::Arc;

use anyhow::{bail, Context};
pub use boringtun::x25519::{PublicKey, StaticSecret};

pub const DEFAULT_PORT_FORWARD_SOURCE: &str = "127.0.0.1";

#[derive(Clone)]
pub struct Config {
    pub port_forwards: Vec<PortForwardConfig>,
    #[allow(dead_code)]
    pub remote_port_forwards: Vec<PortForwardConfig>,
    pub private_key: Arc<StaticSecret>,
    pub endpoint_public_key: Arc<PublicKey>,
    pub preshared_key: Option<[u8; 32]>,
    pub endpoint_addr: SocketAddr,
    pub endpoint_bind_addr: SocketAddr,
    pub source_peer_ip: IpAddr,
    pub keepalive_seconds: Option<u16>,
    pub max_transmission_unit: usize,
    pub log: String,
    pub warnings: Vec<String>,
    pub pcap_file: Option<String>,
}

// `Config::from_args` (onetun's command line, its `bin` feature) is left out (lib.rs, change 2).

/// The bind address for the UDP socket when none is given: as onetun's `from_args`
/// (`config.rs:254-259`), the IP version of the endpoint (lib.rs, change 4).
pub fn default_bind_addr(endpoint_addr: SocketAddr) -> anyhow::Result<SocketAddr> {
    Ok(match endpoint_addr {
        SocketAddr::V4(_) => parse_addr(Some("0.0.0.0:0"))?,
        SocketAddr::V6(_) => parse_addr(Some("[::]:0"))?,
    })
}

pub fn parse_addr<T: AsRef<str>>(s: Option<T>) -> anyhow::Result<SocketAddr> {
    s.context("Missing address")?
        .as_ref()
        .to_socket_addrs()
        .context("Invalid address")?
        .next()
        .context("Could not lookup address")
}

pub fn parse_private_key(s: &str) -> anyhow::Result<StaticSecret> {
    let decoded = base64::decode(s).context("Failed to decode private key")?;
    if let Ok::<[u8; 32], _>(bytes) = decoded.try_into() {
        Ok(StaticSecret::from(bytes))
    } else {
        bail!("Invalid private key")
    }
}

pub fn parse_public_key(s: Option<&String>) -> anyhow::Result<PublicKey> {
    let encoded = s.context("Missing public key")?;
    let decoded = base64::decode(encoded).context("Failed to decode public key")?;
    if let Ok::<[u8; 32], _>(bytes) = decoded.try_into() {
        Ok(PublicKey::from(bytes))
    } else {
        bail!("Invalid public key")
    }
}

pub fn parse_preshared_key(s: Option<&String>) -> anyhow::Result<Option<[u8; 32]>> {
    if let Some(s) = s {
        let decoded = base64::decode(s).context("Failed to decode preshared key")?;
        if let Ok::<[u8; 32], _>(bytes) = decoded.try_into() {
            Ok(Some(bytes))
        } else {
            bail!("Invalid preshared key")
        }
    } else {
        Ok(None)
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct PortForwardConfig {
    /// The source IP and port where the local server will run.
    pub source: SocketAddr,
    /// The destination IP and port to which traffic will be forwarded.
    pub destination: SocketAddr,
    /// The transport protocol to use for the port (Layer 4).
    pub protocol: PortProtocol,
    /// Whether this is a remote port forward.
    pub remote: bool,
}

impl PortForwardConfig {
    /// Converts a string representation into `PortForwardConfig`.
    ///
    /// Sample formats:
    ///  - `127.0.0.1:8080:192.168.4.1:8081:TCP,UDP`
    ///  - `127.0.0.1:8080:192.168.4.1:8081:TCP`
    ///  - `0.0.0.0:8080:192.168.4.1:8081`
    ///  - `[::1]:8080:192.168.4.1:8081`
    ///  - `8080:192.168.4.1:8081`
    ///  - `8080:192.168.4.1:8081:TCP`
    ///  - `localhost:8080:192.168.4.1:8081:TCP`
    ///  - `localhost:8080:peer.intranet:8081:TCP`
    ///
    /// Implementation Notes:
    ///  - The format is formalized as `[src_host:]<src_port>:<dst_host>:<dst_port>[:PROTO1,PROTO2,...]`
    ///  - `src_host` is optional and defaults to `127.0.0.1`.
    ///  - `src_host` and `dst_host` may be specified as IPv4, IPv6, or a FQDN to be resolved by DNS.
    ///  - IPv6 addresses must be prefixed with `[` and suffixed with `]`. Example: `[::1]`.
    ///  - Any `u16` is accepted as `src_port` and `dst_port`
    ///  - Specifying protocols (`PROTO1,PROTO2,...`) is optional and defaults to `TCP`. Values must be separated by commas.
    pub fn from_notation(s: &str, default_source: &str) -> anyhow::Result<Vec<PortForwardConfig>> {
        mod parsers {
            use nom::branch::alt;
            use nom::bytes::complete::is_not;
            use nom::character::complete::{alpha1, char, digit1};
            use nom::combinator::{complete, map, opt, success};
            use nom::error::ErrorKind;
            use nom::multi::separated_list1;
            use nom::sequence::{delimited, preceded, separated_pair, tuple};
            use nom::IResult;

            fn ipv6(s: &str) -> IResult<&str, &str> {
                delimited(char('['), is_not("]"), char(']'))(s)
            }

            fn ipv4_or_fqdn(s: &str) -> IResult<&str, &str> {
                let s = is_not(":")(s)?;
                if s.1.chars().all(|c| c.is_ascii_digit()) {
                    // If ipv4 or fqdn is all digits, it's not valid.
                    Err(nom::Err::Error(nom::error::ParseError::from_error_kind(
                        s.1,
                        ErrorKind::Fail,
                    )))
                } else {
                    Ok(s)
                }
            }

            fn port(s: &str) -> IResult<&str, &str> {
                digit1(s)
            }

            fn ip_or_fqdn(s: &str) -> IResult<&str, &str> {
                alt((ipv6, ipv4_or_fqdn))(s)
            }

            fn no_ip(s: &str) -> IResult<&str, Option<&str>> {
                success(None)(s)
            }

            fn src_addr(s: &str) -> IResult<&str, (Option<&str>, &str)> {
                let with_ip = separated_pair(map(ip_or_fqdn, Some), char(':'), port);
                let without_ip = tuple((no_ip, port));
                alt((with_ip, without_ip))(s)
            }

            fn dst_addr(s: &str) -> IResult<&str, (&str, &str)> {
                separated_pair(ip_or_fqdn, char(':'), port)(s)
            }

            fn protocol(s: &str) -> IResult<&str, &str> {
                alpha1(s)
            }

            fn protocols(s: &str) -> IResult<&str, Option<Vec<&str>>> {
                opt(preceded(char(':'), separated_list1(char(','), protocol)))(s)
            }

            #[allow(clippy::type_complexity)]
            pub fn port_forward(
                s: &str,
            ) -> IResult<&str, ((Option<&str>, &str), (), (&str, &str), Option<Vec<&str>>)>
            {
                complete(tuple((
                    src_addr,
                    map(char(':'), |_| ()),
                    dst_addr,
                    protocols,
                )))(s)
            }
        }

        // TODO: Could improve error management with custom errors, so that the messages are more helpful.
        let (src_addr, _, dst_addr, protocols) = parsers::port_forward(s)
            .map_err(|e| anyhow::anyhow!("Invalid port-forward definition: {}", e))?
            .1;

        let source = (
            src_addr.0.unwrap_or(default_source),
            src_addr.1.parse::<u16>().context("Invalid source port")?,
        )
            .to_socket_addrs()
            .context("Invalid source address")?
            .next()
            .context("Could not resolve source address")?;

        let destination = (
            dst_addr.0,
            dst_addr.1.parse::<u16>().context("Invalid source port")?,
        )
            .to_socket_addrs() // TODO: Pass this as given and use DNS config instead (issue #15)
            .context("Invalid destination address")?
            .next()
            .context("Could not resolve destination address")?;

        // Parse protocols
        let protocols = if let Some(protocols) = protocols {
            let protocols: anyhow::Result<Vec<PortProtocol>> =
                protocols.into_iter().map(PortProtocol::try_from).collect();
            protocols
        } else {
            Ok(vec![PortProtocol::Tcp])
        }
        .context("Failed to parse protocols")?;

        // Returns an config for each protocol
        Ok(protocols
            .into_iter()
            .map(|protocol| Self {
                source,
                destination,
                protocol,
                remote: false,
            })
            .collect())
    }
}

impl Display for PortForwardConfig {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if self.remote {
            write!(
                f,
                "(remote){}:{}:{}",
                self.source, self.destination, self.protocol
            )
        } else {
            write!(f, "{}:{}:{}", self.source, self.destination, self.protocol)
        }
    }
}

/// Layer 7 protocols for ports.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum PortProtocol {
    /// TCP
    Tcp,
    /// UDP
    Udp,
}

impl TryFrom<&str> for PortProtocol {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> anyhow::Result<Self> {
        match value.to_uppercase().as_str() {
            "TCP" => Ok(Self::Tcp),
            "UDP" => Ok(Self::Udp),
            _ => Err(anyhow::anyhow!("Invalid protocol specifier: {}", value)),
        }
    }
}

impl Display for PortProtocol {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Tcp => "TCP",
                Self::Udp => "UDP",
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    /// Tests the parsing of `PortForwardConfig`.
    #[test]
    fn test_parse_port_forward_config_1() {
        assert_eq!(
            PortForwardConfig::from_notation(
                "192.168.0.1:8080:192.168.4.1:8081:TCP,UDP",
                DEFAULT_PORT_FORWARD_SOURCE
            )
            .expect("Failed to parse"),
            vec![
                PortForwardConfig {
                    source: SocketAddr::from_str("192.168.0.1:8080").unwrap(),
                    destination: SocketAddr::from_str("192.168.4.1:8081").unwrap(),
                    protocol: PortProtocol::Tcp,
                    remote: false,
                },
                PortForwardConfig {
                    source: SocketAddr::from_str("192.168.0.1:8080").unwrap(),
                    destination: SocketAddr::from_str("192.168.4.1:8081").unwrap(),
                    protocol: PortProtocol::Udp,
                    remote: false,
                }
            ]
        );
    }
    /// Tests the parsing of `PortForwardConfig`.
    #[test]
    fn test_parse_port_forward_config_2() {
        assert_eq!(
            PortForwardConfig::from_notation(
                "192.168.0.1:8080:192.168.4.1:8081:TCP",
                DEFAULT_PORT_FORWARD_SOURCE
            )
            .expect("Failed to parse"),
            vec![PortForwardConfig {
                source: SocketAddr::from_str("192.168.0.1:8080").unwrap(),
                destination: SocketAddr::from_str("192.168.4.1:8081").unwrap(),
                protocol: PortProtocol::Tcp,
                remote: false,
            }]
        );
    }
    /// Tests the parsing of `PortForwardConfig`.
    #[test]
    fn test_parse_port_forward_config_3() {
        assert_eq!(
            PortForwardConfig::from_notation(
                "0.0.0.0:8080:192.168.4.1:8081",
                DEFAULT_PORT_FORWARD_SOURCE
            )
            .expect("Failed to parse"),
            vec![PortForwardConfig {
                source: SocketAddr::from_str("0.0.0.0:8080").unwrap(),
                destination: SocketAddr::from_str("192.168.4.1:8081").unwrap(),
                protocol: PortProtocol::Tcp,
                remote: false,
            }]
        );
    }
    /// Tests the parsing of `PortForwardConfig`.
    #[test]
    fn test_parse_port_forward_config_4() {
        assert_eq!(
            PortForwardConfig::from_notation(
                "[::1]:8080:192.168.4.1:8081",
                DEFAULT_PORT_FORWARD_SOURCE
            )
            .expect("Failed to parse"),
            vec![PortForwardConfig {
                source: SocketAddr::from_str("[::1]:8080").unwrap(),
                destination: SocketAddr::from_str("192.168.4.1:8081").unwrap(),
                protocol: PortProtocol::Tcp,
                remote: false,
            }]
        );
    }
    /// Tests the parsing of `PortForwardConfig`.
    #[test]
    fn test_parse_port_forward_config_5() {
        assert_eq!(
            PortForwardConfig::from_notation("8080:192.168.4.1:8081", DEFAULT_PORT_FORWARD_SOURCE)
                .expect("Failed to parse"),
            vec![PortForwardConfig {
                source: SocketAddr::from_str("127.0.0.1:8080").unwrap(),
                destination: SocketAddr::from_str("192.168.4.1:8081").unwrap(),
                protocol: PortProtocol::Tcp,
                remote: false,
            }]
        );
    }
    /// Tests the parsing of `PortForwardConfig`.
    #[test]
    fn test_parse_port_forward_config_6() {
        assert_eq!(
            PortForwardConfig::from_notation(
                "8080:192.168.4.1:8081:TCP",
                DEFAULT_PORT_FORWARD_SOURCE
            )
            .expect("Failed to parse"),
            vec![PortForwardConfig {
                source: SocketAddr::from_str("127.0.0.1:8080").unwrap(),
                destination: SocketAddr::from_str("192.168.4.1:8081").unwrap(),
                protocol: PortProtocol::Tcp,
                remote: false,
            }]
        );
    }
    /// Tests the parsing of `PortForwardConfig`.
    #[test]
    fn test_parse_port_forward_config_7() {
        assert_eq!(
            PortForwardConfig::from_notation(
                "localhost:8080:192.168.4.1:8081",
                DEFAULT_PORT_FORWARD_SOURCE
            )
            .expect("Failed to parse"),
            vec![PortForwardConfig {
                source: "localhost:8080".to_socket_addrs().unwrap().next().unwrap(),
                destination: SocketAddr::from_str("192.168.4.1:8081").unwrap(),
                protocol: PortProtocol::Tcp,
                remote: false,
            }]
        );
    }
    /// Tests the parsing of `PortForwardConfig`.
    #[test]
    fn test_parse_port_forward_config_8() {
        assert_eq!(
            PortForwardConfig::from_notation(
                "localhost:8080:localhost:8081:TCP",
                DEFAULT_PORT_FORWARD_SOURCE
            )
            .expect("Failed to parse"),
            vec![PortForwardConfig {
                source: "localhost:8080".to_socket_addrs().unwrap().next().unwrap(),
                destination: "localhost:8081".to_socket_addrs().unwrap().next().unwrap(),
                protocol: PortProtocol::Tcp,
                remote: false,
            }]
        );
    }
}
