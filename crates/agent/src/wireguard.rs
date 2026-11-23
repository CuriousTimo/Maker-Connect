//! WireGuard inside the agent (#194, step 1): an optional mode in which the agent reaches the hub
//! through a WireGuard tunnel of its own, with no VPN app on the computer.
//!
//! The tunnel is `maker-connect-wireguard` (boringtun 0.7.1 and smoltcp, the glue ported from
//! onetun v0.3.10): a local TCP port forwarded through the tunnel to the hub's address (the one on
//! the agent's page). While it runs, every device's hub address is that local port
//! ([`local_hub`]), so the devices' HTTP and WebSocket go through the tunnel unchanged.
//!
//! Ours, the agent's UI and its own plumbing only (nothing on the wire beyond onetun's): the local
//! port `127.0.0.1:18460` (the issue's example); the settings file `wireguard/wireguard.ini`; the
//! card; a thread with one current-thread tokio runtime per tunnel; the tunnel's numbers read once a
//! second for the card. The agent's key pair is made once, from the system's random source
//! (`getrandom`), and kept in that file; the private key is never shown.

use maker_connect_wireguard::config::{
    Config, PortForwardConfig, PortProtocol, PublicKey, StaticSecret, default_bind_addr,
    parse_addr, parse_preshared_key, parse_public_key,
};
use maker_connect_wireguard::events::Bus;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Where the tunnel listens on this computer (ours: the example in #194).
pub(crate) const LOCAL: &str = "127.0.0.1:18460";

/// onetun's MTU (`config.rs:111`).
const MTU: usize = 1420;

/// While the tunnel runs: the local address every device uses as the hub.
static LOCAL_HUB: Mutex<Option<String>> = Mutex::new(None);

/// The hub as the devices should reach it now: the tunnel's local port while it runs.
pub(crate) fn local_hub() -> Option<String> {
    LOCAL_HUB.lock().unwrap().clone()
}

/// What the WireGuard card keeps (`wireguard/wireguard.ini`).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct WgSettings {
    pub on: bool,
    /// the agent's private key, base64 (never shown)
    pub private_key: String,
    /// the agent's address in the tunnel, e.g. `10.10.10.2` (`/32` is accepted and left off)
    pub address: String,
    /// the WireGuard server: `host:port` (a DDNS name is looked up once, when the tunnel starts)
    pub endpoint: String,
    /// the server's public key, base64
    pub peer_public_key: String,
    /// optional, base64
    pub preshared_key: String,
    /// optional, seconds (onetun: none unless set)
    pub keepalive: String,
}

impl WgSettings {
    fn path(dir: &Path) -> PathBuf {
        dir.join("wireguard.ini")
    }

    pub(crate) fn load(dir: &Path) -> WgSettings {
        let mut s = WgSettings::default();
        for line in std::fs::read_to_string(Self::path(dir))
            .unwrap_or_default()
            .lines()
        {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let v = v.trim().to_owned();
            match k.trim() {
                "on" => s.on = v == "True",
                "private_key" => s.private_key = v,
                "address" => s.address = v,
                "endpoint" => s.endpoint = v,
                "public_key" => s.peer_public_key = v,
                "preshared_key" => s.preshared_key = v,
                "keepalive" => s.keepalive = v,
                _ => {}
            }
        }
        if s.private_key.is_empty() {
            s.private_key = new_private_key();
            s.save(dir);
        }
        s
    }

    pub(crate) fn save(&self, dir: &Path) {
        let text = format!(
            "[module]\non = {}\n\n[interface]\nprivate_key = {}\naddress = {}\n\n[peer]\nendpoint = {}\npublic_key = {}\npreshared_key = {}\nkeepalive = {}\n",
            if self.on { "True" } else { "False" },
            self.private_key,
            self.address,
            self.endpoint,
            self.peer_public_key,
            self.preshared_key,
            self.keepalive
        );
        let _ = std::fs::create_dir_all(dir);
        let path = Self::path(dir);
        let _ = std::fs::write(&path, text);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
    }

    /// The agent's public key, base64, to put in the server as a peer.
    pub(crate) fn public_key(&self) -> String {
        decode32(&self.private_key)
            .map(|b| base64::encode(PublicKey::from(&StaticSecret::from(b)).as_bytes()))
            .unwrap_or_default()
    }

    /// onetun's `Config` for these settings, forwarding [`LOCAL`] to `hub`.
    fn config(&self, hub: SocketAddr) -> Result<Config, String> {
        let private = decode32(&self.private_key).ok_or("the agent's private key is broken")?;
        let peer = parse_public_key(Some(&self.peer_public_key))
            .map_err(|e| format!("the server's public key: {e}"))?;
        let preshared =
            parse_preshared_key(Some(&self.preshared_key).filter(|p| !p.trim().is_empty()))
                .map_err(|e| format!("the pre-shared key: {e}"))?;
        let endpoint = parse_addr(Some(&self.endpoint))
            .map_err(|e| format!("the endpoint {}: {e:#}", self.endpoint))?;
        let address: IpAddr = self
            .address
            .split('/')
            .next()
            .unwrap_or_default()
            .trim()
            .parse()
            .map_err(|_| format!("the tunnel address {} is not an IP address", self.address))?;
        let keepalive = match self.keepalive.trim() {
            "" => None,
            k => Some(
                k.parse::<u16>()
                    .map_err(|_| format!("keepalive {k}: not a number of seconds"))?,
            ),
        };
        Ok(Config {
            port_forwards: vec![PortForwardConfig {
                source: LOCAL.parse().expect("LOCAL is an address"),
                destination: hub,
                protocol: PortProtocol::Tcp,
                remote: false,
            }],
            remote_port_forwards: vec![],
            private_key: Arc::new(StaticSecret::from(private)),
            endpoint_public_key: Arc::new(peer),
            preshared_key: preshared,
            endpoint_addr: endpoint,
            endpoint_bind_addr: default_bind_addr(endpoint).map_err(|e| e.to_string())?,
            source_peer_ip: address,
            keepalive_seconds: keepalive,
            max_transmission_unit: MTU,
            log: "info".into(),
            warnings: vec![],
            pcap_file: None,
        })
    }
}

fn decode32(b64: &str) -> Option<[u8; 32]> {
    base64::decode(b64.trim()).ok()?.try_into().ok()
}

/// A new private key from the system's random source (x25519 clamps it when used).
fn new_private_key() -> String {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).expect("the system has no random source");
    base64::encode(bytes)
}

/// What the card shows of a running tunnel (boringtun's `Tunn::stats`).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Numbers {
    pub since_handshake: Option<Duration>,
    pub sent: usize,
    pub received: usize,
}

/// The module: its settings, its status, and the tunnel while it runs.
pub(crate) struct Wg {
    dir: PathBuf,
    pub settings: Mutex<WgSettings>,
    status: Arc<Mutex<String>>,
    numbers: Arc<Mutex<Option<Numbers>>>,
    stop: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl Wg {
    /// In `<agent dir>/wireguard/`.
    pub(crate) fn load(agent_dir: &Path) -> Wg {
        let dir = agent_dir.join("wireguard");
        let settings = WgSettings::load(&dir);
        Wg {
            dir,
            settings: Mutex::new(settings),
            status: Arc::new(Mutex::new("off".into())),
            numbers: Arc::new(Mutex::new(None)),
            stop: Mutex::new(None),
        }
    }

    pub(crate) fn settings(&self) -> WgSettings {
        self.settings.lock().unwrap().clone()
    }

    pub(crate) fn save(&self, s: WgSettings) {
        s.save(&self.dir);
        *self.settings.lock().unwrap() = s;
    }

    pub(crate) fn status(&self) -> String {
        self.status.lock().unwrap().clone()
    }

    pub(crate) fn numbers(&self) -> Option<Numbers> {
        *self.numbers.lock().unwrap()
    }

    /// Starts the tunnel to `hub` (`host:port` as on the agent's page); the status says why not.
    pub(crate) fn start(&self, hub: &str) {
        self.stop();
        let set = |t: String| {
            log::info!("WireGuard: {t}");
            *self.status.lock().unwrap() = t;
        };
        let Some(hub_addr) = hub.to_socket_addrs().ok().and_then(|mut a| a.next()) else {
            set(format!(
                "the hub's address {hub} cannot be looked up here: give its IP address on the far side"
            ));
            return;
        };
        let config = match self.settings().config(hub_addr) {
            Ok(c) => c,
            Err(e) => {
                set(e);
                return;
            }
        };
        let endpoint = config.endpoint_addr;
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        *self.stop.lock().unwrap() = Some(tx);
        let status = self.status.clone();
        let numbers = self.numbers.clone();
        let numbers_after = self.numbers.clone();
        set(format!("starting, to {endpoint}"));
        std::thread::spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    *status.lock().unwrap() = format!("no runtime: {e}");
                    return;
                }
            };
            rt.block_on(async move {
                let tunnel = match maker_connect_wireguard::start_tunnels(config, Bus::new()).await
                {
                    Ok(t) => t,
                    Err(e) => {
                        *status.lock().unwrap() = format!("{e:#}");
                        return;
                    }
                };
                *LOCAL_HUB.lock().unwrap() = Some(LOCAL.to_owned());
                *status.lock().unwrap() =
                    format!("running: {LOCAL} → {hub_addr} through {endpoint}");
                log::info!("WireGuard: running: {LOCAL} → {hub_addr} through {endpoint}");
                let watch = async {
                    loop {
                        let (since, sent, received, _, _) = tunnel.stats().await;
                        *numbers.lock().unwrap() = Some(Numbers {
                            since_handshake: since,
                            sent,
                            received,
                        });
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                };
                tokio::select! {
                    _ = rx => {}
                    _ = watch => {}
                }
            });
            *LOCAL_HUB.lock().unwrap() = None;
            *numbers_after.lock().unwrap() = None;
            log::info!("WireGuard: stopped");
        });
    }

    /// Stops the tunnel (its runtime ends, and with it every task and socket of the tunnel).
    pub(crate) fn stop(&self) {
        if let Some(tx) = self.stop.lock().unwrap().take() {
            let _ = tx.send(());
        }
        *LOCAL_HUB.lock().unwrap() = None;
        *self.numbers.lock().unwrap() = None;
        *self.status.lock().unwrap() = "off".into();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The key pair is made once and kept; the public key is the private key's.
    #[test]
    fn the_key_pair_is_kept() {
        let dir = std::env::temp_dir().join(format!("mc-wg-{}", std::process::id()));
        let a = WgSettings::load(&dir);
        assert_eq!(decode32(&a.private_key).map(|_| ()), Some(()));
        let b = WgSettings::load(&dir);
        assert_eq!(a.private_key, b.private_key);
        assert_eq!(a.public_key(), b.public_key());
        assert_eq!(base64::decode(a.public_key()).unwrap().len(), 32);
        let mut c = b.clone();
        c.on = true;
        c.endpoint = "vpn.example:13231".into();
        c.address = "10.10.10.2/32".into();
        c.peer_public_key = a.public_key();
        c.keepalive = "25".into();
        c.save(&dir);
        assert_eq!(WgSettings::load(&dir), c);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// onetun's `Config` from the settings: the local port to the hub, MTU 1420, `/32` left off,
    /// keepalive only when set; a bad key says which.
    #[test]
    fn the_config_is_onetuns() {
        let dir = std::env::temp_dir().join(format!("mc-wg-cfg-{}", std::process::id()));
        let mut s = WgSettings::load(&dir);
        s.peer_public_key = s.public_key();
        s.endpoint = "127.0.0.1:13231".into();
        s.address = "10.10.10.2/32".into();
        let hub: SocketAddr = "192.168.1.67:8460".parse().unwrap();
        let c = s.config(hub).unwrap();
        assert_eq!(
            c.port_forwards[0].source,
            LOCAL.parse::<SocketAddr>().unwrap()
        );
        assert_eq!(c.port_forwards[0].destination, hub);
        assert_eq!(c.source_peer_ip, "10.10.10.2".parse::<IpAddr>().unwrap());
        assert_eq!(c.max_transmission_unit, 1420);
        assert_eq!(c.keepalive_seconds, None);
        assert_eq!(
            c.endpoint_bind_addr,
            "0.0.0.0:0".parse::<SocketAddr>().unwrap()
        );
        s.peer_public_key = "not base64".into();
        match s.config(hub) {
            Err(e) => assert!(e.starts_with("the server's public key"), "{e}"),
            Ok(_) => panic!("a bad key was taken"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
