//! A Stream Deck for quick management and overview, as a local module of the agent (#190; idea, work in progress).
//!
//! Through deckhand, our fork of `elgato-streamdeck` (CLAUDE.md "Stream Deck: our fork `deckhand`"):
//! deckhand does all I/O with the deck; this module only enumerates decks, puts images on keys and
//! reads key presses. It sets nothing else (no brightness).
//!
//! **Not at the hub:** the protocol has no input device yet (part 10 of the SDK extension, to be
//! written), so the deck does not register and sends nothing to the hub. It shows what the agent's own
//! page shows of its devices, read-only: no key acts on a machine.
//!
//! Ours, the agent's UI only (owner, 2026-11-23: "akkoord met 1-3", #190): the layout (the top two rows the content, the
//! bottom row always navigation, as the demo the owner approved), the texts, the system's UI font
//! (mc.css `--font: system-ui`), and looking for a deck once a second while none is connected
//! (enumeration only, nothing sent to a deck). Also ours, in the agent only (owner, 2026-11-23: "akkoord met 1-4"): the
//! loop's pace (keys read with a 100 ms timeout, as deckhand's `examples/key_test.rs`; the keys
//! redrawn every 500 ms, and only the keys that changed sent; 500 ms between looks while the module
//! is off; 1 s after an error), the font weights (500 and 680 on macOS's variable SF), a held key
//! lit with the selected tint (as the demo), and the keys cleared on Turn off
//! (`clear_all_button_images`).
//!
//! **hidapi:** the deck opens its own `hidapi::HidApi`. The scale port's `real::Hidapi`
//! (`crates/engine/src/ports/usbscale/hid.rs`) says one `HidApi` lives in a process at a time;
//! hidapi 2.6.7 (`src/lib.rs:151`) says "Each instance has its own device list cache". Not checked
//! which holds: the scale is not on the agent yet (#131); check it before both run.
//!
//! Tried only on a Stream Deck Original V2 (firmware 1.03.000) on macOS. Windows, Linux (udev rules
//! needed, deckhand's `40-streamdeck.rules`), other models and unplugging: not tried.

use ab_glyph::{Font, FontVec, PxScale, ScaleFont, VariableFont};
use deckhand::{DeviceStateUpdate, StreamDeck, list_devices, new_hidapi};
use image::{DynamicImage, Rgb, RgbImage};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// The house style, dark (the deck itself is black): mc.css's dark tokens.
const BLACK: Rgb<u8> = Rgb([0, 0, 0]);
const SURFACE: Rgb<u8> = Rgb([0x1B, 0x1C, 0x1F]);
const INK: Rgb<u8> = Rgb([0xF2, 0xF1, 0xEE]);
const MUTED: Rgb<u8> = Rgb([0xA6, 0xA8, 0xA1]);
const ACCENT: Rgb<u8> = Rgb([0x93, 0xA6, 0x82]);
const ON_ACCENT: Rgb<u8> = Rgb([0x1D, 0x1D, 0x1B]);
const ST_BUSY: Rgb<u8> = Rgb([0x93, 0xA6, 0x82]);
const ST_WAIT: Rgb<u8> = Rgb([0x62, 0xC7, 0x7E]);
const ST_WARN: Rgb<u8> = Rgb([0xE3, 0xA9, 0x48]);
const ST_BAD: Rgb<u8> = Rgb([0xF2, 0x85, 0x7A]);
const ST_OFF: Rgb<u8> = Rgb([0x9A, 0x9C, 0x95]);
/// The cut corner of a card (house style: cards 12 px).
const CUT: u32 = 12;

/// The navigation row (keys 10-14 of a 5 x 3 deck).
const BACK: u8 = 10;
const HOME: u8 = 12;

/// What one device shows, as the agent's page has it (`card_live`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DeviceSnap {
    /// short, for a key
    pub short: &'static str,
    pub title: &'static str,
    pub state: &'static str,
    /// the page's pill class: `idle`, `busy`, `wait`, `warn`, `bad`
    pub class: &'static str,
    pub status: String,
    pub hub: String,
    pub registered: String,
}

/// What the deck shows of the agent, read from the agent's page each time it draws.
#[derive(Clone, Debug, PartialEq, Default)]
pub(crate) struct Snapshot {
    pub host: String,
    pub devices: Vec<DeviceSnap>,
    /// the hub's pill text and class, and "n of m connected"
    pub hub: (String, &'static str, String),
}

/// Which screen the deck is on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum View {
    Overview,
    Device(usize),
}

/// What one key shows.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Key {
    Empty,
    /// a card: a small label on top, a value in a state's colour, a line under it
    Tile {
        label: String,
        value: String,
        class: &'static str,
        sub: String,
    },
    Nav {
        arrow: &'static str,
        label: &'static str,
    },
    /// where you are, on the house colour
    Logo {
        title: String,
        sub: String,
    },
}

/// The 15 keys of a view (the Original V2 and the MK.2: 5 x 3).
pub(crate) fn keys(view: View, snap: &Snapshot) -> Vec<Key> {
    let mut keys = vec![Key::Empty; 15];
    match view {
        View::Overview => {
            for (i, d) in snap.devices.iter().take(4).enumerate() {
                keys[i] = Key::Tile {
                    label: d.short.to_owned(),
                    value: d.state.to_owned(),
                    class: d.class,
                    sub: d.status.clone(),
                };
            }
            keys[4] = Key::Tile {
                label: "Hub".into(),
                value: snap.hub.0.clone(),
                class: snap.hub.1,
                sub: snap.hub.2.clone(),
            };
            keys[HOME as usize] = Key::Logo {
                title: "Agent".into(),
                sub: snap.host.clone(),
            };
        }
        View::Device(i) => {
            if let Some(d) = snap.devices.get(i) {
                keys[0] = Key::Tile {
                    label: "Device".into(),
                    value: d.short.to_owned(),
                    class: "",
                    sub: d.title.to_owned(),
                };
                keys[1] = Key::Tile {
                    label: "State".into(),
                    value: d.state.to_owned(),
                    class: d.class,
                    sub: String::new(),
                };
                // the page's hub line, "connected for 3 min · last contact now": its two halves
                let (value, sub) = d.hub.split_once(" · ").unwrap_or((d.hub.as_str(), ""));
                keys[2] = Key::Tile {
                    label: "Hub".into(),
                    value: value.to_owned(),
                    class: "",
                    sub: sub.to_owned(),
                };
                keys[3] = Key::Tile {
                    label: "Status".into(),
                    value: String::new(),
                    class: "",
                    sub: d.status.clone(),
                };
                keys[4] = Key::Tile {
                    label: "Registered".into(),
                    value: d.registered.clone(),
                    class: "",
                    sub: String::new(),
                };
                keys[BACK as usize] = Key::Nav {
                    arrow: "←",
                    label: "Back",
                };
                keys[HOME as usize] = Key::Logo {
                    title: d.short.to_owned(),
                    sub: "device".into(),
                };
            }
        }
    }
    keys
}

/// A key released on a view: the next view.
pub(crate) fn press(view: View, key: u8, snap: &Snapshot) -> View {
    match (view, key) {
        (_, HOME) | (View::Device(_), BACK) => View::Overview,
        (View::Overview, k) if (k as usize) < snap.devices.len().min(4) => View::Device(k as usize),
        (v, _) => v,
    }
}

/// The module: on or off (`[module] on` in its own folder), and what the agent's page says of it.
pub(crate) struct Deck {
    dir: PathBuf,
    on: Mutex<bool>,
    status: Mutex<String>,
    connected: Mutex<bool>,
}

impl Deck {
    /// In `<agent dir>/streamdeck/deck.ini`.
    pub(crate) fn load(agent_dir: &Path) -> Deck {
        let dir = agent_dir.join("streamdeck");
        let text = std::fs::read_to_string(dir.join("deck.ini")).unwrap_or_default();
        let on = text.lines().any(|l| {
            l.split_once('=')
                .is_some_and(|(k, v)| k.trim() == "on" && v.trim() == "True")
        });
        Deck {
            dir,
            on: Mutex::new(on),
            status: Mutex::new("the module is off".into()),
            connected: Mutex::new(false),
        }
    }

    pub(crate) fn on(&self) -> bool {
        *self.on.lock().unwrap()
    }

    pub(crate) fn set_on(&self, on: bool) {
        *self.on.lock().unwrap() = on;
        let _ = std::fs::create_dir_all(&self.dir);
        let _ = std::fs::write(
            self.dir.join("deck.ini"),
            format!("[module]\non = {}\n", if on { "True" } else { "False" }),
        );
    }

    pub(crate) fn status(&self) -> String {
        self.status.lock().unwrap().clone()
    }

    pub(crate) fn connected(&self) -> bool {
        *self.connected.lock().unwrap()
    }

    fn set(&self, connected: bool, status: impl Into<String>) {
        *self.connected.lock().unwrap() = connected;
        let status = status.into();
        let mut s = self.status.lock().unwrap();
        if *s != status {
            log::info!("Stream Deck: {status}");
            *s = status;
        }
    }
}

/// The deck's thread: while on, look for a deck, connect, draw what `snapshot` says and follow the
/// keys; an error drops the deck and the looking starts again.
pub(crate) fn run(deck: Arc<Deck>, snapshot: impl Fn() -> Snapshot) {
    let fonts = match Fonts::system() {
        Ok(f) => f,
        Err(e) => {
            deck.set(false, e);
            return;
        }
    };
    let mut hid = None;
    loop {
        if !deck.on() {
            deck.set(false, "the module is off");
            std::thread::sleep(Duration::from_millis(500));
            continue;
        }
        let api = match hid.as_mut() {
            Some(api) => api,
            None => match new_hidapi() {
                Ok(api) => hid.insert(api),
                Err(e) => {
                    deck.set(false, format!("hidapi: {e}"));
                    std::thread::sleep(Duration::from_secs(1));
                    continue;
                }
            },
        };
        let _ = api.refresh_devices();
        let Some((kind, serial)) = list_devices(api).into_iter().next() else {
            deck.set(false, "no Stream Deck found");
            std::thread::sleep(Duration::from_secs(1));
            continue;
        };
        let sd = match StreamDeck::connect(api, kind, &serial) {
            Ok(sd) => sd,
            Err(e) => {
                deck.set(false, format!("{kind:?} found, not opened: {e}"));
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        let firmware = sd.firmware_version().unwrap_or_default();
        deck.set(true, format!("{kind:?}, firmware {firmware}"));
        if let Err(e) = drive(&deck, sd, &fonts, &snapshot) {
            deck.set(false, format!("lost: {e}"));
        }
    }
}

/// One connected deck, until it is turned off or an error comes.
fn drive(
    deck: &Deck,
    sd: StreamDeck,
    fonts: &Fonts,
    snapshot: &impl Fn() -> Snapshot,
) -> Result<(), deckhand::StreamDeckError> {
    let (w, h) = sd.kind().key_image_format().size;
    let count = sd.kind().key_count() as usize;
    #[allow(clippy::arc_with_non_send_sync)]
    let sd = Arc::new(sd);
    let reader = sd.get_reader();
    let mut view = View::Overview;
    let mut shown: Vec<Option<(Key, bool)>> = vec![None; count];
    let mut held: Option<u8> = None;
    let mut drawn = Instant::now() - Duration::from_secs(1);
    loop {
        if !deck.on() {
            sd.clear_all_button_images()?;
            sd.flush()?;
            return Ok(());
        }
        let mut changed = false;
        for update in reader.read(Some(Duration::from_millis(100)))? {
            match update {
                DeviceStateUpdate::ButtonDown(k) => {
                    held = Some(k);
                    changed = true;
                }
                DeviceStateUpdate::ButtonUp(k) => {
                    held = None;
                    view = press(view, k, &snapshot());
                    changed = true;
                }
                _ => {}
            }
        }
        if changed || drawn.elapsed() >= Duration::from_millis(500) {
            drawn = Instant::now();
            let keys = keys(view, &snapshot());
            let mut sent = false;
            for (i, key) in keys.into_iter().enumerate().take(count) {
                let now = (key, held == Some(i as u8));
                if shown[i].as_ref() != Some(&now) {
                    let img = draw(&now.0, now.1, w as u32, h as u32, fonts);
                    sd.set_button_image(i as u8, DynamicImage::ImageRgb8(img))?;
                    shown[i] = Some(now);
                    sent = true;
                }
            }
            if sent {
                sd.flush()?;
            }
        }
    }
}

/// The system's UI font (mc.css `--font: system-ui`), regular and bold. Only macOS's was tried.
struct Fonts {
    regular: FontVec,
    bold: FontVec,
}

impl Fonts {
    fn system() -> Result<Fonts, String> {
        // macOS: SF, one variable font
        let sf = "/System/Library/Fonts/SFNS.ttf";
        if let Ok(data) = std::fs::read(sf) {
            let mut regular = FontVec::try_from_vec(data.clone()).map_err(|e| e.to_string())?;
            let mut bold = FontVec::try_from_vec(data).map_err(|e| e.to_string())?;
            regular.set_variation(b"wght", 500.0);
            bold.set_variation(b"wght", 680.0);
            return Ok(Fonts { regular, bold });
        }
        // Windows: Segoe UI; Linux: DejaVu Sans or Noto Sans (not tried)
        let pairs = [
            (
                r"C:\Windows\Fonts\segoeui.ttf",
                r"C:\Windows\Fonts\segoeuib.ttf",
            ),
            (
                "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
                "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
            ),
            (
                "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
                "/usr/share/fonts/truetype/noto/NotoSans-Bold.ttf",
            ),
        ];
        for (r, b) in pairs {
            if let (Ok(r), Ok(b)) = (std::fs::read(r), std::fs::read(b)) {
                return Ok(Fonts {
                    regular: FontVec::try_from_vec(r).map_err(|e| e.to_string())?,
                    bold: FontVec::try_from_vec(b).map_err(|e| e.to_string())?,
                });
            }
        }
        Err(format!(
            "no system font found (looked for {sf}, {})",
            pairs.map(|p| p.0).join(", ")
        ))
    }
}

/// A state's colour, as the agent page's pill classes have them (mc.css `.st.<class>`).
fn class_colour(class: &str) -> Rgb<u8> {
    match class {
        "busy" => ST_BUSY,
        "wait" => ST_WAIT,
        "warn" => ST_WARN,
        "bad" => ST_BAD,
        "idle" => ST_OFF,
        _ => INK,
    }
}

/// A key as a picture: a card with one 45° cut corner top right (house style); held: the selected
/// tint (house colour 18 %).
fn draw(key: &Key, held: bool, w: u32, h: u32, fonts: &Fonts) -> RgbImage {
    let fill = if held {
        mix(SURFACE, ACCENT, 0.18)
    } else {
        SURFACE
    };
    let p = Pen { w, h, fonts };
    match key {
        Key::Empty => RgbImage::from_pixel(w, h, BLACK),
        Key::Tile {
            label,
            value,
            class,
            sub,
        } => {
            let mut img = p.tile(fill);
            p.text_fit(&mut img, label, 7, 17, 11.0, false, MUTED);
            if !value.is_empty() {
                p.text_fit(&mut img, value, 7, 40, 15.0, true, class_colour(class));
            }
            if !sub.is_empty() {
                // with no value, the line has the key to itself: higher up and larger
                let (y, size) = if value.is_empty() {
                    (40, 11.0)
                } else {
                    (57, 10.0)
                };
                p.text_fit(&mut img, sub, 7, y, size, false, MUTED);
            }
            img
        }
        Key::Nav { arrow, label } => {
            let mut img = p.tile(fill);
            p.text_centre(&mut img, arrow, 40, 24.0, true, INK);
            p.text_centre(&mut img, label, 58, 10.0, false, MUTED);
            img
        }
        Key::Logo { title, sub } => {
            let mut img = p.tile(ACCENT);
            p.text_fit(&mut img, "MC", 8, 24, 22.0, true, ON_ACCENT);
            p.text_fit(&mut img, title, 8, 44, 11.0, true, ON_ACCENT);
            p.text_fit(&mut img, sub, 8, 60, 10.0, false, ON_ACCENT);
            img
        }
    }
}

struct Pen<'a> {
    w: u32,
    h: u32,
    fonts: &'a Fonts,
}

impl Pen<'_> {
    /// A card: square corners, one 45° cut top right where the deck's black shows.
    fn tile(&self, fill: Rgb<u8>) -> RgbImage {
        let mut img = RgbImage::from_pixel(self.w, self.h, fill);
        for y in 0..CUT.min(self.h) {
            for x in (self.w.saturating_sub(CUT) + y)..self.w {
                img.put_pixel(x, y, BLACK);
            }
        }
        img
    }

    fn font(&self, bold: bool) -> &FontVec {
        if bold {
            &self.fonts.bold
        } else {
            &self.fonts.regular
        }
    }

    fn width(&self, s: &str, size: f32, bold: bool) -> f32 {
        let sf = self.font(bold).as_scaled(PxScale::from(size));
        s.chars().map(|c| sf.h_advance(sf.glyph_id(c))).sum()
    }

    /// Text with its baseline at `y`.
    #[allow(clippy::too_many_arguments)]
    fn text(&self, img: &mut RgbImage, s: &str, x: i32, y: i32, size: f32, bold: bool, c: Rgb<u8>) {
        let font = self.font(bold);
        let sf = font.as_scaled(PxScale::from(size));
        let mut pen = x as f32;
        for ch in s.chars() {
            let id = sf.glyph_id(ch);
            let g = id.with_scale_and_position(PxScale::from(size), ab_glyph::point(pen, y as f32));
            pen += sf.h_advance(id);
            if let Some(o) = font.outline_glyph(g) {
                let b = o.px_bounds();
                o.draw(|gx, gy, cov| {
                    let (px, py) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
                    if px >= 0 && py >= 0 && (px as u32) < self.w && (py as u32) < self.h {
                        let bg = *img.get_pixel(px as u32, py as u32);
                        img.put_pixel(px as u32, py as u32, mix(bg, c, cov));
                    }
                });
            }
        }
    }

    /// Text that shrinks to fit the key's width.
    #[allow(clippy::too_many_arguments)]
    fn text_fit(
        &self,
        img: &mut RgbImage,
        s: &str,
        x: i32,
        y: i32,
        size: f32,
        bold: bool,
        c: Rgb<u8>,
    ) {
        let room = self.w as f32 - 2.0 * x as f32;
        let mut size = size;
        while size > 7.0 && self.width(s, size, bold) > room {
            size -= 0.5;
        }
        self.text(img, s, x, y, size, bold, c);
    }

    fn text_centre(&self, img: &mut RgbImage, s: &str, y: i32, size: f32, bold: bool, c: Rgb<u8>) {
        let x = ((self.w as f32 - self.width(s, size, bold)) / 2.0).max(2.0) as i32;
        self.text(img, s, x, y, size, bold, c);
    }
}

fn mix(a: Rgb<u8>, b: Rgb<u8>, t: f32) -> Rgb<u8> {
    let f = |i: usize| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t).round() as u8;
    Rgb([f(0), f(1), f(2)])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap() -> Snapshot {
        let d = |short, title, state, class, status: &str| DeviceSnap {
            short,
            title,
            state,
            class,
            status: status.into(),
            hub: "connected for 3 min · last contact now".into(),
            registered: "yes".into(),
        };
        Snapshot {
            host: "office-mac".into(),
            devices: vec![
                d(
                    "PnP",
                    "DYMO LabelManager PnP",
                    "Off",
                    "idle",
                    "the module is off",
                ),
                d(
                    "LW450",
                    "DYMO LabelWriter 450",
                    "Connected",
                    "wait",
                    "ready",
                ),
                d(
                    "Marlin",
                    "Marlin printer",
                    "Not connected",
                    "warn",
                    "no hub address",
                ),
                d("Laser", "GRBL laser", "Off", "idle", "the module is off"),
            ],
            hub: ("Connected".into(), "wait", "1 of 2 connected".into()),
        }
    }

    /// The overview: the four devices as the agent's page has them, the hub, where you are; the
    /// rest empty (the bottom row navigation, nothing to go back to).
    #[test]
    fn the_overview_shows_the_agents_page() {
        let k = keys(View::Overview, &snap());
        assert_eq!(
            k[1],
            Key::Tile {
                label: "LW450".into(),
                value: "Connected".into(),
                class: "wait",
                sub: "ready".into()
            }
        );
        assert_eq!(
            k[4],
            Key::Tile {
                label: "Hub".into(),
                value: "Connected".into(),
                class: "wait",
                sub: "1 of 2 connected".into()
            }
        );
        assert_eq!(
            k[12],
            Key::Logo {
                title: "Agent".into(),
                sub: "office-mac".into()
            }
        );
        assert_eq!(k[BACK as usize], Key::Empty);
        assert!(k[5..10].iter().all(|k| *k == Key::Empty));
    }

    /// A device's page, and the keys that move between views (read-only: nothing else does).
    #[test]
    fn a_device_and_back() {
        let s = snap();
        let v = press(View::Overview, 2, &s);
        assert_eq!(v, View::Device(2));
        let k = keys(v, &s);
        assert_eq!(
            k[2],
            Key::Tile {
                label: "Hub".into(),
                value: "connected for 3 min".into(),
                class: "",
                sub: "last contact now".into()
            }
        );
        assert_eq!(
            k[BACK as usize],
            Key::Nav {
                arrow: "←",
                label: "Back"
            }
        );
        assert_eq!(press(v, BACK, &s), View::Overview);
        assert_eq!(press(v, HOME, &s), View::Overview);
        assert_eq!(press(v, 1, &s), v);
        assert_eq!(press(View::Overview, 7, &s), View::Overview);
    }

    /// `[module] on` kept in its own folder.
    #[test]
    fn on_is_kept() {
        let dir = std::env::temp_dir().join(format!("mc-deck-{}", std::process::id()));
        let deck = Deck::load(&dir);
        assert!(!deck.on());
        deck.set_on(true);
        assert!(Deck::load(&dir).on());
        deck.set_on(false);
        assert!(!Deck::load(&dir).on());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A key is a card with the one cut corner top right (house style), when a font is there.
    #[test]
    fn a_key_has_the_cut_corner() {
        let Ok(fonts) = Fonts::system() else {
            return;
        };
        let img = draw(&Key::Empty, false, 72, 72, &fonts);
        assert_eq!(*img.get_pixel(0, 0), BLACK);
        let img = draw(
            &Key::Nav {
                arrow: "←",
                label: "Back",
            },
            false,
            72,
            72,
            &fonts,
        );
        assert_eq!(*img.get_pixel(71, 0), BLACK, "cut");
        assert_eq!(*img.get_pixel(0, 0), SURFACE, "square");
        assert_eq!(*img.get_pixel(71, 71), SURFACE, "square");
    }
}
