//! The board's own page: read only, in the Maker Connect house style (wiki Design: Door Controller,
//! mockups `images/door-controller-status-{desktop,phone}.png`). It shows what the board itself knows:
//! the link to the hub, the firmware, how long it is up, and each lock's switch and rest. Names of
//! lockers and the space are the hub's, not shown here. No buttons, no settings, no secrets.

use door_controller::traits::DoorState;

#[derive(Clone, Default)]
pub struct Lock {
    /// `None`: the input cannot be read.
    pub door: Option<DoorState>,
    pub resting_s: u32,
}

#[derive(Clone, Default)]
pub struct Snapshot {
    pub serial: String,
    pub firmware: String,
    pub hub: String,
    pub tls: bool,
    pub connected: bool,
    /// The registration code to type into the hub, while registering.
    pub code: Option<String>,
    pub uptime_s: u64,
    pub rssi: Option<i8>,
    pub secure_boot: bool,
    pub flash_encryption: bool,
    pub locks: Vec<Lock>,
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn uptime(s: u64) -> String {
    let (d, h, m) = (s / 86_400, s % 86_400 / 3600, s % 3600 / 60);
    if d > 0 {
        format!("{d} d {h} h")
    } else if h > 0 {
        format!("{h} h {m} min")
    } else {
        format!("{m} min")
    }
}

const STYLE: &str = r#"
:root{--bg:#F6F7F5;--surface:#FFF;--ink:#1D1D1B;--muted:#585C54;--line:#DCDFD8;--accent:#93A682;
--accent-text:#5E6B52;--ok:#1E7B3B;--warn:#8F5B00;--bad:#B3261E;--info:#2F62B8;--cut:12px;
--font:system-ui,-apple-system,"Segoe UI",Roboto,sans-serif}
@media (prefers-color-scheme:dark){:root{--bg:#111214;--surface:#1B1C1F;--ink:#F2F1EE;--muted:#A6A8A1;
--line:#2E3033;--accent-text:#93A682;--ok:#62C77E;--warn:#E3A948;--bad:#F2857A;--info:#86B3FF}}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--ink);font:15px/1.45 var(--font)}
.cut{clip-path:polygon(0 0,calc(100% - var(--cut)) 0,100% var(--cut),100% 100%,0 100%)}
header{display:flex;align-items:center;gap:12px;padding:12px 16px;background:var(--surface);border-bottom:1px solid var(--line)}
.logo{width:28px;height:28px;background:var(--accent);color:#1D1D1B;font-weight:700;font-size:12px;display:grid;place-items:center;--cut:8px}
header b{display:block}header small{color:var(--muted)}.ro{margin-left:auto;background:rgb(147 166 130/.24);padding:3px 10px;font-weight:600;font-size:13px;--cut:6px}
main{max-width:1000px;margin:0 auto;padding:16px}h1{font-size:24px;margin:8px 0 2px}h2{font-size:19px;margin:24px 0 10px}
.sub{color:var(--muted);margin:0 0 16px}
.box{background:var(--surface);border:1px solid var(--line);padding:12px 16px}
.warn{border-left:3px solid var(--warn);margin-bottom:12px}.note{border-left:3px solid var(--info);margin-top:16px}
.grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(210px,1fr));gap:12px}
.k{color:var(--muted);font-size:13px}.v{font-size:20px;font-weight:700}.m{color:var(--muted);font-size:13px}
.st{display:inline-flex;align-items:center;gap:6px;background:rgb(147 166 130/.24);padding:1px 8px;font-size:13px;font-weight:600;--cut:6px}
.st::before{content:"";width:8px;height:8px;border-radius:50%;background:currentColor}
.ok{color:var(--ok)}.wa{color:var(--warn)}.ba{color:var(--bad)}.in{color:var(--info)}
.st span{color:var(--ink)}.lock b{font-size:17px;margin-right:8px}.code{font:700 22px ui-monospace,Menlo,monospace;letter-spacing:2px}
"#;

pub fn render(s: &Snapshot) -> String {
    let mut h = String::with_capacity(6000);
    h.push_str("<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><meta http-equiv=\"refresh\" content=\"5\"><title>Door controller</title><style>");
    h.push_str(STYLE);
    h.push_str("</style></head><body><header><div class=\"logo cut\">MC</div><div><b>Maker Connect</b><small>Door controller · ");
    h.push_str(&esc(&s.serial));
    h.push_str("</small></div><span class=\"ro cut\">Read only</span></header><main>");
    h.push_str(&format!(
        "<h1>Door controller</h1><p class=\"sub\">KC868-A6 · {} locks · serial {}</p>",
        s.locks.len(),
        esc(&s.serial)
    ));
    if !(s.secure_boot && s.flash_encryption) {
        h.push_str("<div class=\"box warn cut\"><b>Not protected: development board</b><br>");
        let off = match (s.secure_boot, s.flash_encryption) {
            (false, false) => "Secure Boot and flash encryption are off.",
            (false, true) => "Secure Boot is off.",
            _ => "Flash encryption is off.",
        };
        h.push_str(off);
        h.push_str(" This board runs any firmware put on it, and its Wi-Fi details and hub key can be read from the chip. Switch both on before real use.</div>");
    }
    if let Some(code) = &s.code {
        h.push_str(&format!(
            "<div class=\"box warn cut\"><b>Not registered yet</b><br>Type this code into the hub: <span class=\"code\">{}</span></div>",
            esc(code)
        ));
    }
    let hub_state = if s.connected {
        "<span class=\"st ok\"><span>Connected</span></span>"
    } else {
        "<span class=\"st ba\"><span>Not connected</span></span>"
    };
    h.push_str("<div class=\"grid\">");
    h.push_str(&format!(
        "<div class=\"box cut\"><div class=\"k\">Hub</div>{hub_state}<div class=\"m\">{} · {}</div></div>",
        esc(&s.hub),
        if s.tls { "TLS" } else { "no TLS" }
    ));
    h.push_str(&format!(
        "<div class=\"box cut\"><div class=\"k\">Firmware</div><div class=\"v\">{}</div><div class=\"m\">{}</div></div>",
        esc(&s.firmware),
        if s.secure_boot { "signed" } else { "not checked: Secure Boot off" }
    ));
    let wifi = s
        .rssi
        .map_or("Wi-Fi".to_string(), |r| format!("Wi-Fi {r} dBm"));
    h.push_str(&format!(
        "<div class=\"box cut\"><div class=\"k\">Up</div><div class=\"v\">{}</div><div class=\"m\">{}</div></div>",
        uptime(s.uptime_s),
        wifi
    ));
    h.push_str("</div><h2>Locks</h2><div class=\"grid\">");
    for (i, l) in s.locks.iter().enumerate() {
        let n = i + 1;
        let (class, label, line) = match (l.door, l.resting_s) {
            (None, _) => ("ba", "Unknown", "The input does not answer".to_string()),
            (Some(DoorState::Closed), 0) => ("ok", "Locked", "Switch closed".to_string()),
            (Some(DoorState::Closed), r) => (
                "in",
                "Cooling down",
                format!("Locked · can open again in {r} s"),
            ),
            (Some(DoorState::Open), 0) => {
                ("wa", "Open", "Switch open, or no switch wired".to_string())
            }
            (Some(DoorState::Open), r) => (
                "wa",
                "Open",
                format!("Switch open · can open again in {r} s"),
            ),
        };
        h.push_str(&format!(
            "<div class=\"box cut lock\"><b>Lock {n}</b><span class=\"st {class}\"><span>{label}</span></span><div class=\"m\">relay {n} · input {n}</div><div>{line}</div></div>"
        ));
    }
    h.push_str("</div><div class=\"box note cut\">This page only shows. A lock is opened in Maker Connect, where your rights are checked.</div></main></body></html>");
    h
}

fn open(h: &mut String, serial: &str, badge: &str) {
    h.push_str("<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Door controller setup</title><style>");
    h.push_str(STYLE);
    h.push_str(FORM_STYLE);
    h.push_str("</style></head><body><header><div class=\"logo cut\">MC</div><div><b>Maker Connect</b><small>Door controller · ");
    h.push_str(&esc(serial));
    h.push_str("</small></div><span class=\"ro cut\">");
    h.push_str(badge);
    h.push_str("</span></header><main>");
}

const FORM_STYLE: &str = r#"
form{display:grid;gap:12px;max-width:480px}label{display:grid;gap:4px;font-weight:600}
input[type=text],input[type=password],input[type=number]{font:inherit;padding:10px;border:1px solid var(--line);background:var(--surface);color:var(--ink);min-height:44px}
.check{display:flex;align-items:center;gap:8px;font-weight:400}.check input{width:20px;height:20px}
button{font:inherit;font-weight:700;min-height:44px;border:0;background:var(--accent);color:#1D1D1B;padding:0 20px;justify-self:start;--cut:8px;cursor:pointer}
.err{border-left:3px solid var(--bad);margin-bottom:12px}
"#;

fn protection_warning(h: &mut String, secure_boot: bool, flash_encryption: bool) {
    if !(secure_boot && flash_encryption) {
        h.push_str("<div class=\"box warn cut\"><b>Not protected: development board</b><br>Secure Boot or flash encryption is off. This board runs any firmware put on it, and the Wi-Fi details you enter here can be read from the chip. Switch both on before real use.</div>");
    }
}

/// The setup page: the only page with fields, shown only on an empty board's own hotspot.
pub fn setup_form(
    serial: &str,
    error: Option<&str>,
    secure_boot: bool,
    flash_encryption: bool,
) -> String {
    let mut h = String::with_capacity(5000);
    open(&mut h, serial, "Setup");
    h.push_str("<h1>Set up this door controller</h1><p class=\"sub\">Step 1 of 2: the network and the hub. Step 2: type the code this page then shows into the hub.</p>");
    protection_warning(&mut h, secure_boot, flash_encryption);
    if let Some(e) = error {
        h.push_str(&format!("<div class=\"box err cut\">{}</div>", esc(e)));
    }
    h.push_str("<form class=\"box cut\" method=\"post\" action=\"/setup\">\
<label>Wi-Fi network<input type=\"text\" name=\"ssid\" maxlength=\"32\" required autocomplete=\"off\"></label>\
<label>Wi-Fi password<input type=\"password\" name=\"pass\" maxlength=\"64\" autocomplete=\"off\"></label>\
<label>Hub address<input type=\"text\" name=\"host\" required placeholder=\"hub.local or 192.168.1.10\" autocomplete=\"off\"></label>\
<label>Hub port<input type=\"number\" name=\"port\" min=\"1\" max=\"65535\" placeholder=\"80, or 443 with TLS\"></label>\
<label class=\"check\"><input type=\"checkbox\" name=\"tls\" value=\"True\">The hub uses TLS (https)</label>\
<button class=\"cut\" type=\"submit\">Save and join</button></form>");
    h.push_str("<div class=\"box note cut\">The board is put back to empty only by hand: hold button S2 on the board for 10 seconds.</div></main></body></html>");
    h
}

/// After the form: the board is joining the network; the page then shows the code.
pub fn joining(serial: &str, ssid: &str) -> String {
    let mut h = String::with_capacity(3000);
    open(&mut h, serial, "Setup");
    h = h.replacen(
        "<title>",
        "<meta http-equiv=\"refresh\" content=\"5;url=/\"><title>",
        1,
    );
    h.push_str(&format!(
        "<h1>Joining {}</h1><p class=\"sub\">Saved. The board joins the network and asks the hub for a code. This page shows it in a moment.</p></main></body></html>",
        esc(ssid)
    ));
    h
}
