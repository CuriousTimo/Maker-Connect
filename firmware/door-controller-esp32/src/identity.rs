//! Clock and storage: monotonic ticks, SNTP time, the token in NVS, serial and fingerprint.

use std::time::{SystemTime, UNIX_EPOCH};

use door_controller::traits::{Clock, Store};
use esp_idf_svc::nvs::{EspNvs, NvsDefault};
use esp_idf_svc::sntp::{EspSntp, SyncStatus};
use sha2::{Digest, Sha256};

pub struct BoardClock {
    pub sntp: EspSntp<'static>,
    /// `sntp_get_sync_status` answers `Completed` once and then resets itself to `Reset`, so the
    /// first sync is kept here; the system clock stays set after it.
    pub synced: std::cell::Cell<bool>,
}

impl Clock for BoardClock {
    fn now_ms(&self) -> u32 {
        // esp_timer counts microseconds since boot; Buddy's ticks_ms() wraps at u32.
        (unsafe { esp_idf_svc::sys::esp_timer_get_time() } / 1000) as u32
    }

    fn unix_time(&self) -> Option<u64> {
        if !self.synced.get() {
            if self.sntp.get_sync_status() != SyncStatus::Completed {
                return None;
            }
            self.synced.set(true);
        }
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs())
    }

    fn sleep_ms(&mut self, ms: u32) {
        std::thread::sleep(std::time::Duration::from_millis(u64::from(ms)));
    }
}

const TOKEN_KEY: &str = "token";

pub struct BoardStore {
    pub nvs: EspNvs<NvsDefault>,
    pub serial: String,
    pub fingerprint: String,
}

impl Store for BoardStore {
    fn token(&self) -> Option<String> {
        let mut buf = [0u8; 64];
        match self.nvs.get_str(TOKEN_KEY, &mut buf) {
            Ok(Some(t)) if !t.is_empty() => Some(t.to_string()),
            _ => None,
        }
    }

    fn set_token(&mut self, token: &str) {
        if let Err(e) = self.nvs.set_str(TOKEN_KEY, token) {
            log::error!("storing the token failed: {e}");
        }
    }

    fn serial(&self) -> String {
        self.serial.clone()
    }

    fn fingerprint(&self) -> String {
        self.fingerprint.clone()
    }
}

/// The serial number: the board's factory MAC in hex. Buddy reads it from OTP; this board has none.
pub fn serial(factory_mac: &[u8; 6]) -> String {
    factory_mac.iter().map(|b| format!("{b:02X}")).collect()
}

/// Buddy's `printerHash(str, size, false)` (`src/common/support_utils.cpp:22-60`): SHA-256 over
/// CPU id, MAC and serial, written as `size` symbols of 5 bits (`to32`,
/// `src/common/support_utils_lib.cpp:37-60`). Buddy's CPU id is the STM32 UID; the ESP32's is its
/// factory MAC in eFuse, so `cpu_id` is that and `mac` the Wi-Fi station MAC.
pub fn printer_hash(cpu_id: &[u8], mac: &[u8; 6], serial: &str, size: usize) -> String {
    let mut h = Sha256::new();
    h.update(cpu_id);
    h.update(mac);
    h.update(serial.as_bytes());
    let hash: [u8; 32] = h.finalize().into();
    assert!(hash.len() * 8 >= size * 5);
    (0..size).map(|i| to32(&hash, i * 5)).collect()
}

/// `to32` (`support_utils_lib.cpp:37-60`).
fn to32(number: &[u8; 32], start_bit: usize) -> char {
    let byte = start_bit / 8;
    let bit = (start_bit % 8) as u32;
    let mut val: u32 = (0b1111_1000u32 >> bit) & u32::from(number[byte]);
    if bit > 3 {
        val <<= bit - 3;
        let next = number.get(byte + 1).copied().unwrap_or(0);
        val |= ((0b1111_1000u32 << (8 - bit)) & u32::from(next)) >> (8 - bit + 3);
    } else {
        val >>= 3 - bit;
    }
    match val {
        0..=9 => char::from(b'0' + val as u8),
        10..=31 => char::from(b'A' + (val - 10) as u8),
        _ => '!',
    }
}
