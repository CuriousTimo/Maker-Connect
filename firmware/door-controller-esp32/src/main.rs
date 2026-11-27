//! Maker Connect door-controller firmware for the Kincony KC868-A6.
//!
//! The behaviour is the `door-controller` crate (Part 7 of `docs/design/prusa-sdk-extension.md`, the
//! client ported from Buddy 6.4.0); this binary binds its traits to the board ([`board`]), the clock
//! and NVS ([`identity`]) and the network ([`net`], [`tls`]).
//!
//! The read-only page is [`page`], on port 80; setting up an empty board and putting it back to empty
//! is [`setup`]. The board answers as `door-xxxx.local` (mDNS). Not built yet
//! (wiki Design: Door Controller): OTA through the hub,
//! Secure Boot and flash encryption (these wait for the board in hand). For a development flash, Wi-Fi
//! and hub may also come from the build environment (`DOOR_WIFI_SSID`, `DOOR_WIFI_PASS`,
//! `DOOR_HUB_HOST`, `DOOR_HUB_PORT`, `DOOR_HUB_TLS`); NVS goes first. Never put real values in the repo.

mod board;
mod identity;
mod net;
mod page;
mod setup;
mod tls;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use door_controller::connect::Client;
use door_controller::device::Printer;
use door_controller::traits::Store;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::i2c::{I2cConfig, I2cDriver};
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::units::Hertz;
use esp_idf_svc::http::Method;
use esp_idf_svc::http::server::{Configuration as HttpConfig, EspHttpServer};
use esp_idf_svc::io::Write;
use esp_idf_svc::ipv4::{Configuration as IpConfiguration, Mask, RouterConfiguration, Subnet};
use esp_idf_svc::mdns::EspMdns;
use esp_idf_svc::netif::{EspNetif, NetifConfiguration};
use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};
use esp_idf_svc::ota::EspOta;
use esp_idf_svc::sntp::EspSntp;
use esp_idf_svc::wifi::{
    AccessPointConfiguration, AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi,
};
use setup::Settings;

const NAMESPACE: &str = "door";

fn setting(nvs: &EspNvs<NvsDefault>, key: &str, build: Option<&'static str>) -> Option<String> {
    // A host name is at most 253 characters; a Wi-Fi password 64.
    let mut buf = [0u8; 256];
    match nvs.get_str(key, &mut buf) {
        Ok(Some(v)) if !v.is_empty() => Some(v.to_string()),
        _ => build.filter(|v| !v.is_empty()).map(str::to_string),
    }
}

/// Wi-Fi and hub from NVS, else (development flash only) from the build environment.
fn settings(nvs: &EspNvs<NvsDefault>) -> Option<Settings> {
    Some(Settings {
        ssid: setting(nvs, "wifi_ssid", option_env!("DOOR_WIFI_SSID"))?,
        pass: setting(nvs, "wifi_pass", option_env!("DOOR_WIFI_PASS")).unwrap_or_default(),
        host: setting(nvs, "hub_host", option_env!("DOOR_HUB_HOST"))?,
        port: setting(nvs, "hub_port", option_env!("DOOR_HUB_PORT")).and_then(|p| p.parse().ok()),
        tls: setting(nvs, "hub_tls", option_env!("DOOR_HUB_TLS")).is_some_and(|v| v == "True"),
    })
}

fn client_conf(s: &Settings) -> anyhow::Result<ClientConfiguration> {
    Ok(ClientConfiguration {
        ssid: s
            .ssid
            .as_str()
            .try_into()
            .map_err(|_| anyhow::anyhow!("SSID too long"))?,
        password: s
            .pass
            .as_str()
            .try_into()
            .map_err(|_| anyhow::anyhow!("password too long"))?,
        auth_method: if s.pass.is_empty() {
            AuthMethod::None
        } else {
            AuthMethod::WPA2Personal
        },
        ..Default::default()
    })
}

/// What the page at `/` is.
enum Phase {
    /// An empty board on its hotspot: the setup form, with the last error.
    Form(Option<String>),
    /// The form was sent; the board is joining this network.
    Joining(Settings),
    /// The read-only status page.
    Status,
}

fn html(
    req: esp_idf_svc::http::server::Request<&mut esp_idf_svc::http::server::EspHttpConnection>,
    status: u16,
    body: &str,
) -> anyhow::Result<()> {
    req.into_response(
        status,
        None,
        &[
            ("Content-Type", "text/html; charset=utf-8"),
            ("Cache-Control", "no-store"),
        ],
    )?
    .write_all(body.as_bytes())?;
    Ok(())
}

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let peripherals = Peripherals::take()?;
    let sysloop = EspSystemEventLoop::take()?;
    let partition = EspDefaultNvsPartition::take()?;

    // Relays off before anything else.
    let i2c = I2cDriver::new(
        peripherals.i2c0,
        peripherals.pins.gpio4,
        peripherals.pins.gpio15,
        &I2cConfig::new().baudrate(Hertz(100_000)),
    )?;
    let bus: board::Bus = Rc::new(RefCell::new(i2c));
    // Nothing runs before every relay is known to be off: a relay left on through a restart holds a
    // lock open (the PCF8574 keeps its outputs). Tried again each second until the expanders answer.
    while board::init(&bus).is_err() {
        log::error!("the I2C expanders do not answer: trying again");
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    setup::watch_reset_button(peripherals.pins.gpio0, partition.clone())?;

    let nvs = EspNvs::new(partition.clone(), NAMESPACE, true)?;
    let mut wifi = BlockingWifi::wrap(
        // No NVS for the Wi-Fi driver: it would keep its own copy of the Wi-Fi details, which "back to
        // empty" (setup::erase) must not leave behind. The settings live only in our `door` keys.
        EspWifi::new(peripherals.modem, sysloop.clone(), None)?,
        sysloop,
    )?;
    // The hotspot at 192.168.4.1 (ESP-IDF's own default; esp-idf-svc's is 192.168.71.1), and its DHCP
    // gives the board itself as DNS server (esp-idf-svc's default gives 8.8.8.8), so a phone asks the
    // captive DNS and the setup page opens by itself.
    wifi.wifi_mut()
        .swap_netif_ap(EspNetif::new_with_conf(&NetifConfiguration {
            ip_configuration: Some(IpConfiguration::Router(RouterConfiguration {
                subnet: Subnet {
                    gateway: setup::AP_IP,
                    mask: Mask(24),
                },
                dhcp_enabled: true,
                dns: Some(setup::AP_IP),
                secondary_dns: None,
            })),
            // Its own key: the default AP netif (`WIFI_AP_DEF`) still exists until the swap.
            key: "door_ap"
                .try_into()
                .map_err(|_| anyhow::anyhow!("netif key"))?,
            ..NetifConfiguration::wifi_default_router()
        })?)?;
    let factory_mac = {
        let mut m = [0u8; 6];
        unsafe { esp_idf_svc::sys::esp_efuse_mac_get_default(m.as_mut_ptr()) };
        m
    };
    let sta_mac = wifi.wifi().sta_netif().get_mac()?;
    let serial = identity::serial(&factory_mac);
    let fingerprint = identity::printer_hash(&factory_mac, &sta_mac, &serial, 50);
    let secure_boot = secure_boot_v2();
    let flash_encryption = unsafe { esp_idf_svc::sys::esp_flash_encryption_enabled() };
    log::info!("serial {serial}");

    let snapshot = Arc::new(Mutex::new(page::Snapshot {
        serial: serial.clone(),
        firmware: env!("CARGO_PKG_VERSION").to_string(),
        secure_boot,
        flash_encryption,
        ..Default::default()
    }));
    let configured = settings(&nvs);
    let phase = Arc::new(Mutex::new(if configured.is_some() {
        Phase::Status
    } else {
        Phase::Form(None)
    }));

    let mut server = EspHttpServer::new(&HttpConfig {
        uri_match_wildcard: true,
        ..Default::default()
    })?;
    {
        let (phase, snapshot, serial) = (phase.clone(), snapshot.clone(), serial.clone());
        server.fn_handler("/", Method::Get, move |req| -> anyhow::Result<()> {
            let body = match &*phase.lock().map_err(|_| anyhow::anyhow!("poisoned"))? {
                Phase::Form(error) => {
                    page::setup_form(&serial, error.as_deref(), secure_boot, flash_encryption)
                }
                Phase::Joining(s) => page::joining(&serial, &s.ssid),
                Phase::Status => {
                    page::render(&*snapshot.lock().map_err(|_| anyhow::anyhow!("poisoned"))?)
                }
            };
            html(req, 200, &body)
        })?;
    }
    {
        let (phase, partition, serial) = (phase.clone(), partition.clone(), serial.clone());
        server.fn_handler(
            "/setup",
            Method::Post,
            move |mut req| -> anyhow::Result<()> {
                let mut p = phase.lock().map_err(|_| anyhow::anyhow!("poisoned"))?;
                // Only an empty board on its hotspot takes the form.
                if !matches!(*p, Phase::Form(_)) {
                    return html(req, 403, "Forbidden");
                }
                let mut body = Vec::new();
                let mut buf = [0u8; 256];
                loop {
                    let n = req.read(&mut buf)?;
                    if n == 0 || body.len() + n > 1024 {
                        break;
                    }
                    body.extend_from_slice(&buf[..n]);
                }
                match Settings::from_form(&String::from_utf8_lossy(&body)) {
                    Ok(s) => {
                        s.save(&mut EspNvs::new(partition.clone(), NAMESPACE, true)?)?;
                        let page = page::joining(&serial, &s.ssid);
                        *p = Phase::Joining(s);
                        html(req, 200, &page)
                    }
                    Err(e) => {
                        let page =
                            page::setup_form(&serial, Some(&e), secure_boot, flash_encryption);
                        *p = Phase::Form(Some(e));
                        html(req, 200, &page)
                    }
                }
            },
        )?;
    }
    {
        // Captive portal: whatever a phone asks for on the hotspot leads to the setup page.
        let phase = phase.clone();
        server.fn_handler("/*", Method::Get, move |req| -> anyhow::Result<()> {
            if matches!(
                *phase.lock().map_err(|_| anyhow::anyhow!("poisoned"))?,
                Phase::Status
            ) {
                return html(req, 404, "Not found");
            }
            let location = format!("http://{}/", setup::AP_IP);
            req.into_response(302, None, &[("Location", location.as_str())])?;
            Ok(())
        })?;
    }

    // Joined to the network; `hotspot` while the setup hotspot is still on.
    let (settings, hotspot) = match configured {
        Some(s) => {
            wifi.set_configuration(&Configuration::Client(client_conf(&s)?))?;
            wifi.start()?;
            wifi.connect()?;
            (s, false)
        }
        None => {
            let ap = AccessPointConfiguration {
                ssid: format!("Maker-Door-{}", &serial[serial.len() - 4..])
                    .as_str()
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("SSID"))?,
                password: setup::SETUP_PASS
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("setup password too long"))?,
                auth_method: AuthMethod::WPA2Personal,
                ..Default::default()
            };
            wifi.set_configuration(&Configuration::AccessPoint(ap.clone()))?;
            wifi.start()?;
            setup::captive_dns()?;
            log::info!("setup hotspot on");
            loop {
                std::thread::sleep(Duration::from_millis(200));
                let sent = match &*phase.lock().map_err(|_| anyhow::anyhow!("poisoned"))? {
                    Phase::Joining(s) => Some(s.clone()),
                    _ => None,
                };
                let Some(s) = sent else { continue };
                wifi.set_configuration(&Configuration::Mixed(client_conf(&s)?, ap.clone()))?;
                match wifi.connect() {
                    Ok(()) => break (s, true),
                    Err(e) => {
                        log::warn!("joining {} failed: {e}", s.ssid);
                        setup::erase(&mut EspNvs::new(partition.clone(), NAMESPACE, true)?);
                        wifi.set_configuration(&Configuration::AccessPoint(ap.clone()))?;
                        *phase.lock().map_err(|_| anyhow::anyhow!("poisoned"))? =
                            Phase::Form(Some(format!(
                                "Could not join {}. Check the network and its password.",
                                s.ssid
                            )));
                    }
                }
            }
        }
    };
    wifi.wait_netif_up()?;
    log::info!("Wi-Fi up");
    // `door-xxxx.local`: the last four characters of the serial, as the hotspot's name.
    let hostname = format!("door-{}", serial[serial.len() - 4..].to_lowercase());
    let mut mdns = EspMdns::take()?;
    mdns.set_hostname(&hostname)?;
    log::info!("{hostname}.local");
    *phase.lock().map_err(|_| anyhow::anyhow!("poisoned"))? = Phase::Status;
    let mut hotspot = hotspot;

    let sntp = EspSntp::new_default()?;
    let store = identity::BoardStore {
        nvs,
        serial,
        fingerprint,
    };
    let has_token = store.token().is_some();
    let mut printer = Printer::new(
        Box::new(board::BoardRelays { bus: bus.clone() }),
        Box::new(board::BoardInputs { bus }),
        Box::new(identity::BoardClock {
            sntp,
            synced: Default::default(),
        }),
        Box::new(store),
        board::CHANNELS,
        env!("CARGO_PKG_VERSION"),
    );
    let mut client = Client::new(&mut printer);
    let port = settings.port.unwrap_or(if settings.tls { 443 } else { 80 });
    let mut net = net::BoardNet::new(net::Server {
        host: settings.host.clone(),
        port,
        tls: settings.tls,
    });
    if !has_token {
        client.request_registration();
    }
    if let Ok(mut s) = snapshot.lock() {
        s.hub = format!("{}:{port}", settings.host);
        s.tls = settings.tls;
    }

    let mut shown = None;
    let mut refreshed = 0u32;
    // Updates over the hub: the build runs on trial until it reached the hub; then it is kept.
    let mut on_trial = true;
    let mut restart_when_sent = false;
    loop {
        client.run_once(&mut printer, &mut net);
        if on_trial && client.websocket_open() {
            on_trial = false;
            match EspOta::new().and_then(|mut ota| ota.mark_running_slot_valid()) {
                Ok(()) => log::info!("this build reached the hub: kept"),
                Err(e) => log::error!("marking the build valid failed: {e}"),
            }
        }
        if client.upgrade_pending() {
            let failed = update(&net, printer.token()).err();
            if let Some(why) = failed {
                log::error!("update failed: {why}");
            }
            restart_when_sent = failed.is_none();
            client.upgrade_done(failed);
        }
        if restart_when_sent && client.idle() {
            log::warn!("update written: restarting into the new build");
            std::thread::sleep(std::time::Duration::from_millis(500));
            unsafe { esp_idf_svc::sys::esp_restart() };
        }
        // Buddy leaves the registration when the person closes its dialog on the printer's screen
        // (`leave_registration`, connect.cpp:937-942); the board has no screen: it leaves once the
        // hub gave the token, and connects with it.
        if client.status() == door_controller::status::ConnectionStatus::RegistrationDone {
            client.leave_registration();
        }
        let code = client.registration_code().map(str::to_string);
        if code != shown {
            if let Some(c) = &code {
                log::info!("registration code {c}");
            }
            shown = code.clone();
        }
        // Registered and connected: the hotspot goes off (the station keeps its connection).
        if hotspot && client.websocket_open() {
            if let Err(e) = esp_idf_svc::sys::esp!(unsafe {
                esp_idf_svc::sys::esp_wifi_set_mode(esp_idf_svc::sys::wifi_mode_t_WIFI_MODE_STA)
            }) {
                log::error!("switching the hotspot off failed: {e}");
            } else {
                log::info!("setup done, hotspot off");
                hotspot = false;
            }
        }
        let now = printer.now();
        if now.wrapping_sub(refreshed) >= 1000 {
            refreshed = now;
            // The page's view; the crate reads the inputs the same way for telemetry.
            let params = printer.params(false);
            if let Ok(mut s) = snapshot.lock() {
                s.connected = client.websocket_open();
                s.code = code;
                s.uptime_s = (unsafe { esp_idf_svc::sys::esp_timer_get_time() } / 1_000_000) as u64;
                s.rssi = rssi();
                s.locks = params
                    .locks
                    .iter()
                    .map(|l| page::Lock {
                        door: l.door,
                        resting_s: l.resting_s,
                    })
                    .collect();
            }
        }
    }
}

/// Part 7 "Updating the firmware": the hub's build into the other app slot, checked by ESP-IDF
/// (`esp_ota_end`: the image and its SHA-256), then that slot boots next.
fn update(net: &net::BoardNet, token: &str) -> Result<(), &'static str> {
    let mut ota = EspOta::new().map_err(|_| "No update slot")?;
    let mut update = ota.initiate_update().map_err(|_| "No update slot")?;
    let fetched = net.download(token, |piece| {
        update.write(piece).map_err(|_| "Writing the build failed")
    });
    match fetched {
        Ok(()) => update
            .complete()
            .map_err(|_| "The build is not a valid image"),
        Err(why) => {
            let _ = update.abort();
            Err(why)
        }
    }
}

/// `esp_secure_boot_enabled()` (`esp_secure_boot.h:110-125`) for the ESP32 with Secure Boot v2: the
/// eFuse `ABS_DONE_1`.
fn secure_boot_v2() -> bool {
    unsafe {
        esp_idf_svc::sys::esp_efuse_read_field_bit(std::ptr::addr_of!(
            esp_idf_svc::sys::ESP_EFUSE_ABS_DONE_1
        )
            as *mut *const esp_idf_svc::sys::esp_efuse_desc_t)
    }
}

fn rssi() -> Option<i8> {
    let mut info = esp_idf_svc::sys::wifi_ap_record_t::default();
    (unsafe { esp_idf_svc::sys::esp_wifi_sta_get_ap_info(&mut info) } == 0).then_some(info.rssi)
}
