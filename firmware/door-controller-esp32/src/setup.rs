//! Setting up an empty board, and putting it back to empty (wiki Design: Door Controller, "Setting up
//! an empty board" and "Back to empty only by hand"; choices below by the owner proxy, 2026-11-27):
//!
//! - An empty board opens the hotspot `Maker-Door-XXXX` (last four characters of the serial), WPA2 with
//!   one short password, the same on every board: [`SETUP_PASS`] (owner, 2026-11-27: a random sticker
//!   password is "totaal niet gebruiksvriendelijk voor setup"; chose "kort vast wachtwoord").
//! - Every name the phone looks up answers with the board, so the setup page opens by itself.
//! - The page asks for the Wi-Fi network, its password and the hub's address (host, port, TLS, as the
//!   agent's `[service::connect]`). The board saves them and joins the network, the hotspot staying on
//!   until the registration is done, so the page can show the code.
//! - Back to empty: button S2 (GPIO0, the only button the firmware can read; S1 is EN) held down for
//!   [`RESET_HOLD_MS`]. It erases Wi-Fi, hub and token, and restarts. To be checked
//!   on the board when it is in hand.

use std::net::{Ipv4Addr, UdpSocket};
use std::time::Duration;

use esp_idf_svc::hal::gpio::{Gpio0, PinDriver, Pull};
use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};

use crate::NAMESPACE;

/// How long S2 must be held to put the board back to empty (proxy, 2026-11-27: 10 s, one constant).
pub const RESET_HOLD_MS: u32 = 10_000;
/// The hotspot's own address: ESP-IDF's default for the access point (main.rs sets it).
pub const AP_IP: Ipv4Addr = Ipv4Addr::new(192, 168, 4, 1);

/// The setup hotspot's password, the same on every board (owner, 2026-11-27).
pub const SETUP_PASS: &str = "makerdoor";
const KEYS: [&str; 6] = [
    "wifi_ssid",
    "wifi_pass",
    "hub_host",
    "hub_port",
    "hub_tls",
    "token",
];

#[derive(Clone, Debug, Default)]
pub struct Settings {
    pub ssid: String,
    pub pass: String,
    pub host: String,
    pub port: Option<u16>,
    pub tls: bool,
}

impl Settings {
    pub fn save(&self, nvs: &mut EspNvs<NvsDefault>) -> anyhow::Result<()> {
        nvs.set_str("wifi_ssid", &self.ssid)?;
        nvs.set_str("wifi_pass", &self.pass)?;
        nvs.set_str("hub_host", &self.host)?;
        match self.port {
            Some(p) => nvs.set_str("hub_port", &p.to_string())?,
            None => {
                nvs.remove("hub_port")?;
            }
        }
        nvs.set_str("hub_tls", if self.tls { "True" } else { "False" })?;
        Ok(())
    }

    /// From the setup form (`application/x-www-form-urlencoded`). `Err` is the text the page shows.
    pub fn from_form(body: &str) -> Result<Self, String> {
        let mut s = Settings::default();
        let mut port = String::new();
        for pair in body.split('&') {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            let v = url_decode(v).ok_or("The form could not be read.")?;
            match k {
                "ssid" => s.ssid = v.trim().to_string(),
                "pass" => s.pass = v,
                "host" => s.host = v.trim().to_string(),
                "port" => port = v.trim().to_string(),
                "tls" => s.tls = true,
                _ => {}
            }
        }
        // The ESP32's own limits (wifi_sta_config_t: ssid 32 bytes, password 64).
        if s.ssid.is_empty() || s.ssid.len() > 32 {
            return Err("Fill in the Wi-Fi network (at most 32 characters).".into());
        }
        if s.pass.len() > 64 || (!s.pass.is_empty() && s.pass.len() < 8) {
            return Err(
                "A Wi-Fi password is 8 to 64 characters, or empty for an open network.".into(),
            );
        }
        if s.host.is_empty() || s.host.contains(['/', ' ', ':']) {
            return Err(
                "Fill in the hub's address as a name or IP address, without http:// or a port."
                    .into(),
            );
        }
        if !port.is_empty() {
            s.port = Some(
                port.parse()
                    .map_err(|_| "The port is a number from 1 to 65535.")?,
            );
            if s.port == Some(0) {
                return Err("The port is a number from 1 to 65535.".into());
            }
        }
        Ok(s)
    }
}

fn url_decode(v: &str) -> Option<String> {
    let mut out = Vec::with_capacity(v.len());
    let b = v.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' => {
                let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 2;
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8(out).ok()
}

/// Erases Wi-Fi, hub and token.
pub fn erase(nvs: &mut EspNvs<NvsDefault>) {
    for k in KEYS {
        if let Err(e) = nvs.remove(k) {
            log::error!("erasing {k} failed: {e}");
        }
    }
}

/// Watches S2: held down for [`RESET_HOLD_MS`], the board goes back to empty and restarts.
/// GPIO0 is read only after boot (held at power-up it starts the ROM download mode instead).
pub fn watch_reset_button(
    pin: Gpio0<'static>,
    partition: EspDefaultNvsPartition,
) -> anyhow::Result<()> {
    let button = PinDriver::input(pin, Pull::Up)?;
    std::thread::Builder::new()
        .stack_size(4096)
        .spawn(move || {
            let mut held_ms = 0u32;
            loop {
                std::thread::sleep(Duration::from_millis(100));
                if button.is_low() {
                    held_ms += 100;
                    if held_ms >= RESET_HOLD_MS {
                        log::warn!("S2 held {RESET_HOLD_MS} ms: back to empty");
                        match EspNvs::new(partition.clone(), NAMESPACE, true) {
                            Ok(mut nvs) => erase(&mut nvs),
                            Err(e) => log::error!("NVS: {e}"),
                        }
                        unsafe { esp_idf_svc::sys::esp_restart() };
                    }
                } else {
                    held_ms = 0;
                }
            }
        })?;
    Ok(())
}

/// Answers every DNS question with the hotspot's address, so a phone opens the setup page by itself.
pub fn captive_dns() -> anyhow::Result<()> {
    let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 53))?;
    std::thread::Builder::new()
        .stack_size(4096)
        .spawn(move || {
            let mut buf = [0u8; 512];
            loop {
                let Ok((n, from)) = sock.recv_from(&mut buf) else {
                    continue;
                };
                if let Some(answer) = dns_answer(&buf[..n]) {
                    let _ = sock.send_to(&answer, from);
                }
            }
        })?;
    Ok(())
}

/// One A record for the first question (RFC 1035 4.1): the header with QR, AA and RA set, the
/// question as asked, and an answer that points back at the question's name.
fn dns_answer(q: &[u8]) -> Option<Vec<u8>> {
    if q.len() < 12 || q[2] & 0x80 != 0 || u16::from_be_bytes([q[4], q[5]]) == 0 {
        return None;
    }
    // The end of the first question: the name's labels, a zero byte, type and class.
    let mut i = 12;
    while *q.get(i)? != 0 {
        i += 1 + usize::from(q[i]);
    }
    let end = i + 5;
    let question = q.get(12..end)?;
    // Only an A question gets the address; any other type gets no answer records.
    let is_a = question[question.len() - 4..question.len() - 2] == [0, 1];
    let mut r = Vec::with_capacity(end + 16);
    r.extend_from_slice(&q[0..2]);
    r.extend_from_slice(&[0x84 | (q[2] & 0x01), 0x80]);
    r.extend_from_slice(&[0, 1, 0, u8::from(is_a), 0, 0, 0, 0]);
    r.extend_from_slice(question);
    if is_a {
        r.extend_from_slice(&[0xC0, 0x0C, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4]);
        r.extend_from_slice(&AP_IP.octets());
    }
    Some(r)
}
