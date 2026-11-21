//! ONVIF: an ONVIF camera's RTSP address found by `GetCapabilities`, `GetProfiles` and
//! `GetStreamUri`, with a WS-UsernameToken. Ported function by function from go2rtc's ONVIF
//! client: `pkg/onvif/client.go`, `envelope.go` and `helpers.go` (`FindTagValue`, `atoi`,
//! `GetPath`) of go2rtc v1.9.15, commit `7c05066`, Copyright (c) 2020 Alexey Khit, MIT
//! licence. Names and file:line (at `7c05066`) are in the comments. Not ported: `server.go`,
//! `Appendf` (no caller in `client.go`), `DiscoveryStreamingDevices`, `UUID`, `findScope`,
//! `GetPosixTZ`.
//!
//! Also from the Go standard library, as go2rtc uses it: `html.UnescapeString`
//! (`html/escape.go`, `unescapeEntity`; the entity table only partly, see [`html_unescape`]),
//! `time.RFC3339Nano` ([`rfc3339_nano`]) and `core.RandString` (`pkg/core/helpers.go:27`).
//!
//! The address is go2rtc's own scheme: `onvif://user:pass@host:port[?subtype=N][&snapshot]`.
//! [`resolve`] is `NewClient` followed by `GetURI`.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use sha1::{Digest as _, Sha1};

/// `client.go:15`
pub const PATH_DEVICE: &str = "/onvif/device_service";

// `server.go:12`, `:13`, `:22`, `:33` (the operation names `client.go` uses).
const DEVICE_GET_CAPABILITIES: &str = "GetCapabilities";
const DEVICE_GET_SERVICES: &str = "GetServices";
const DEVICE_GET_DEVICE_INFORMATION: &str = "GetDeviceInformation";
const MEDIA_GET_PROFILES: &str = "GetProfiles";

/// go2rtc's errors are plain texts.
pub type Error = String;

/// Go's `*url.Userinfo`: the user and the password (`None`: not given), both decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Userinfo {
    pub username: String,
    pub password: Option<String>,
}

/// `client.go:17-23`
pub struct Client {
    url: url::Url,
    user: Option<Userinfo>,

    device_url: String,
    media_url: String,
    imagin_url: String,

    /// Tests only: a fixed nonce and `created`, as the Go harness patches `envelope.go`.
    #[cfg(test)]
    fixed: Option<(String, String)>,
}

/// `url.Userinfo` of a parsed URL (`u.User`).
fn userinfo(u: &url::Url) -> Option<Userinfo> {
    let dec = |s: &str| {
        percent_encoding::percent_decode_str(s)
            .decode_utf8_lossy()
            .into_owned()
    };
    if u.username().is_empty() && u.password().is_none() {
        // Go: "onvif://:@host" would have a user, this is the one case that is lost.
        return None;
    }
    Some(Userinfo {
        username: dec(u.username()),
        password: u.password().map(dec),
    })
}

impl Client {
    /// `NewClient`, `client.go:25-48`.
    pub fn new(raw_url: &str) -> Result<Client, Error> {
        Self::new_with(raw_url, None)
    }

    #[cfg_attr(not(test), allow(unused_variables))]
    fn new_with(raw_url: &str, fixed: Option<(String, String)>) -> Result<Client, Error> {
        let u = url::Url::parse(raw_url).map_err(|e| e.to_string())?;

        // Go's `u.Host`: host and port as written.
        let host = match (u.host_str(), u.port()) {
            (Some(h), Some(p)) => format!("{h}:{p}"),
            (Some(h), None) => h.to_owned(),
            (None, _) => String::new(),
        };
        let base_url = format!("http://{host}");

        let mut client = Client {
            user: userinfo(&u),
            device_url: format!("{base_url}{}", get_path(u.path(), PATH_DEVICE)),
            url: u,
            media_url: String::new(),
            imagin_url: String::new(),
            #[cfg(test)]
            fixed,
        };

        let b = client.device_request(DEVICE_GET_CAPABILITIES)?;

        let s = find_tag_value(&b, "Media.+?XAddr");
        client.media_url = format!("{base_url}{}", get_path(&s, "/onvif/media_service"));

        let s = find_tag_value(&b, "Imaging.+?XAddr");
        client.imagin_url = format!("{base_url}{}", get_path(&s, "/onvif/imaging_service"));

        Ok(client)
    }

    /// `GetURI`, `client.go:50-90`.
    pub fn get_uri(&self) -> Result<String, Error> {
        // Go: `query.Get("subtype")` is the first value, "" when there is none.
        let mut token = self
            .url
            .query_pairs()
            .find(|(k, _)| k == "subtype")
            .map(|(_, v)| v.into_owned())
            .unwrap_or_default();

        // support empty
        let i = atoi(&token);
        if i >= 0 {
            let tokens = self.get_profiles_tokens()?;
            if i as usize >= tokens.len() {
                return Err("onvif: wrong subtype".to_owned());
            }
            token = tokens[i as usize].clone();
        }

        let snapshot = self.url.query_pairs().any(|(k, _)| k == "snapshot");
        let b = if snapshot {
            self.get_snapshot_uri(&token)?
        } else {
            self.get_stream_uri(&token)?
        };

        let raw_url = find_tag_value(&b, "Uri");
        let raw_url = html_unescape(&raw_url);
        // Go's `strings.TrimSpace`: Unicode white space.
        let raw_url = raw_url.trim();

        let mut u = url::Url::parse(raw_url).map_err(|e| e.to_string())?;

        if userinfo(&u).is_none()
            && let Some(user) = &self.user
        {
            let _ = u.set_username(&user.username);
            let _ = u.set_password(user.password.as_deref());
        }

        Ok(u.to_string())
    }

    /// `GetName`, `client.go:92-99`.
    pub fn get_name(&self) -> Result<String, Error> {
        let b = self.device_request(DEVICE_GET_DEVICE_INFORMATION)?;

        Ok(find_tag_value(&b, "Manufacturer") + " " + &find_tag_value(&b, "Model"))
    }

    /// `GetProfilesTokens`, `client.go:101-115`.
    pub fn get_profiles_tokens(&self) -> Result<Vec<String>, Error> {
        let b = self.media_request(MEDIA_GET_PROFILES)?;

        let mut tokens = Vec::new();

        let re = regex::Regex::new(r#"Profiles.+?token="([^"]+)"#).unwrap();
        // Go: `FindAllStringSubmatch(s, 10)`, at most 10 matches.
        for m in re.captures_iter(&String::from_utf8_lossy(&b)).take(10) {
            tokens.push(m[1].to_owned());
        }

        Ok(tokens)
    }

    /// `HasSnapshots`, `client.go:117-123`.
    pub fn has_snapshots(&self) -> bool {
        match self.get_service_capabilities() {
            Ok(b) => String::from_utf8_lossy(&b).contains(r#"SnapshotUri="true""#),
            Err(_) => false,
        }
    }

    /// `GetProfile`, `client.go:125-129`.
    pub fn get_profile(&self, token: &str) -> Result<Vec<u8>, Error> {
        self.request(
            &self.media_url,
            &format!(
                "<trt:GetProfile><trt:ProfileToken>{token}</trt:ProfileToken></trt:GetProfile>"
            ),
        )
    }

    /// `GetVideoSourceConfiguration`, `client.go:131-135`.
    pub fn get_video_source_configuration(&self, token: &str) -> Result<Vec<u8>, Error> {
        self.request(
            &self.media_url,
            &format!(
                "<trt:GetVideoSourceConfiguration>\n\t<trt:ConfigurationToken>{token}</trt:ConfigurationToken>\n</trt:GetVideoSourceConfiguration>"
            ),
        )
    }

    /// `GetStreamUri`, `client.go:137-145`.
    pub fn get_stream_uri(&self, token: &str) -> Result<Vec<u8>, Error> {
        self.request(
            &self.media_url,
            &format!(
                "<trt:GetStreamUri>\n\t<trt:StreamSetup>\n\t\t<tt:Stream>RTP-Unicast</tt:Stream>\n\t\t<tt:Transport><tt:Protocol>RTSP</tt:Protocol></tt:Transport>\n\t</trt:StreamSetup>\n\t<trt:ProfileToken>{token}</trt:ProfileToken>\n</trt:GetStreamUri>"
            ),
        )
    }

    /// `GetSnapshotUri`, `client.go:147-151`.
    pub fn get_snapshot_uri(&self, token: &str) -> Result<Vec<u8>, Error> {
        self.request(
            &self.imagin_url,
            &format!(
                "<trt:GetSnapshotUri><trt:ProfileToken>{token}</trt:ProfileToken></trt:GetSnapshotUri>"
            ),
        )
    }

    /// `GetServiceCapabilities`, `client.go:153-158`.
    pub fn get_service_capabilities(&self) -> Result<Vec<u8>, Error> {
        // some cameras answer GetServiceCapabilities for media only for path = "/onvif/media"
        self.request(&self.media_url, "<trt:GetServiceCapabilities />")
    }

    /// `DeviceRequest`, `client.go:160-170`.
    pub fn device_request(&self, operation: &str) -> Result<Vec<u8>, Error> {
        let operation = match operation {
            DEVICE_GET_SERVICES => {
                "<tds:GetServices><tds:IncludeCapability>true</tds:IncludeCapability></tds:GetServices>"
                    .to_owned()
            }
            DEVICE_GET_CAPABILITIES => {
                "<tds:GetCapabilities><tds:Category>All</tds:Category></tds:GetCapabilities>"
                    .to_owned()
            }
            _ => format!("<tds:{operation}/>"),
        };
        self.request(&self.device_url, &operation)
    }

    /// `MediaRequest`, `client.go:172-175`.
    pub fn media_request(&self, operation: &str) -> Result<Vec<u8>, Error> {
        let operation = format!("<trt:{operation}/>");
        self.request(&self.media_url, &operation)
    }

    /// `Request`, `client.go:177-197`. A blocking call (go2rtc's is too): run it in
    /// `spawn_blocking` from async code.
    pub fn request(&self, url: &str, body: &str) -> Result<Vec<u8>, Error> {
        if url.is_empty() {
            return Err("onvif: unsupported service".to_owned());
        }

        let mut e = self.envelope();
        e.append(&[body]);

        // `time.Second * 5000`, `client.go:185`: 5000 seconds, as written there.
        let client = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(5000))
            .build();
        // The `Content-Type` of `client.Post` (`client.go:186`) and Go's default `User-Agent`.
        let res = client
            .post(url)
            .set("Content-Type", "application/soap+xml;charset=utf-8")
            .set("User-Agent", "Go-http-client/1.1")
            .send_bytes(&e.bytes());
        let res = match res {
            Ok(res) => res,
            // Go: `res.Status` is the status line without the protocol, "401 Unauthorized".
            Err(ureq::Error::Status(code, res)) => {
                return Err(format!(
                    "onvif: wrong response {code} {}",
                    res.status_text()
                ));
            }
            Err(e) => return Err(e.to_string()),
        };

        if res.status() != 200 {
            return Err(format!(
                "onvif: wrong response {} {}",
                res.status(),
                res.status_text()
            ));
        }

        let mut b = Vec::new();
        std::io::Read::read_to_end(&mut res.into_reader(), &mut b).map_err(|e| e.to_string())?;
        Ok(b)
    }

    fn envelope(&self) -> Envelope {
        #[cfg(test)]
        if let Some((nonce, created)) = &self.fixed {
            return Envelope::with_user_at(self.user.as_ref(), nonce, created);
        }
        Envelope::with_user(self.user.as_ref())
    }
}

/// `NewClient` and `GetURI` for an `onvif://` address: its RTSP address, with the user and
/// password of the address in it when the camera's answer has none (`client.go:85-87`).
pub fn resolve(raw_url: &str) -> Result<String, Error> {
    Client::new(raw_url)?.get_uri()
}

/// `Envelope`, `envelope.go:13-15`.
pub struct Envelope {
    buf: Vec<u8>,
}

// `envelope.go:17-21`
const PREFIX1: &str = r#"<?xml version="1.0" encoding="utf-8"?><s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope" xmlns:tt="http://www.onvif.org/ver10/schema" xmlns:tds="http://www.onvif.org/ver10/device/wsdl" xmlns:trt="http://www.onvif.org/ver10/media/wsdl">"#;
const PREFIX2: &str = "<s:Body>";
const SUFFIX: &str = "</s:Body></s:Envelope>";

impl Envelope {
    /// `NewEnvelope`, `envelope.go:23-27`.
    pub fn new() -> Envelope {
        let mut e = Envelope {
            buf: Vec::with_capacity(1024),
        };
        e.append(&[PREFIX1, PREFIX2]);
        e
    }

    /// `NewEnvelopeWithUser`, `envelope.go:29-59`: the nonce is `core.RandString(16, 36)` and
    /// `created` is `time.Now().UTC().Format(time.RFC3339Nano)`.
    pub fn with_user(user: Option<&Userinfo>) -> Envelope {
        if user.is_none() {
            return Envelope::new();
        }
        let nonce = rand_string(16, 36);
        let created = rfc3339_nano(SystemTime::now());
        Self::with_user_at(user, &nonce, &created)
    }

    /// The rest of `NewEnvelopeWithUser`, for a given nonce and time.
    pub fn with_user_at(user: Option<&Userinfo>, nonce: &str, created: &str) -> Envelope {
        let Some(user) = user else {
            return Envelope::new();
        };

        let pass = user.password.as_deref().unwrap_or("");

        let mut h = Sha1::new();
        h.update(format!("{nonce}{created}{pass}").as_bytes());

        let b64 = base64::engine::general_purpose::STANDARD;

        let mut e = Envelope {
            buf: Vec::with_capacity(1024),
        };
        e.append(&[PREFIX1]);
        // `%s`: nothing is XML-escaped, as in go2rtc.
        e.append(&[&format!(
            r#"<s:Header>
	<wsse:Security xmlns:wsse="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-secext-1.0.xsd">
		<wsse:UsernameToken>
			<wsse:Username>{}</wsse:Username>
			<wsse:Password Type="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-username-token-profile-1.0#PasswordDigest">{}</wsse:Password>
			<wsse:Nonce EncodingType="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-soap-message-security-1.0#Base64Binary">{}</wsse:Nonce>
			<wsu:Created xmlns:wsu="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd">{}</wsu:Created>
		</wsse:UsernameToken>
	</wsse:Security>
</s:Header>"#,
            user.username,
            b64.encode(h.finalize()),
            b64.encode(nonce.as_bytes()),
            created
        )]);
        e.append(&[PREFIX2]);
        e
    }

    /// `Append`, `envelope.go:61-65`.
    pub fn append(&mut self, args: &[&str]) {
        for s in args {
            self.buf.extend_from_slice(s.as_bytes());
        }
    }

    /// `Bytes`, `envelope.go:71-73`.
    pub fn bytes(&self) -> Vec<u8> {
        let mut b = self.buf.clone();
        b.extend_from_slice(SUFFIX.as_bytes());
        b
    }
}

impl Default for Envelope {
    fn default() -> Self {
        Self::new()
    }
}

/// `core.RandString`, `pkg/core/helpers.go:27-39`: `size` random symbols out of the first
/// `base` of `symbols`.
fn rand_string(size: usize, base: u8) -> String {
    const SYMBOLS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ-_";
    let mut b = vec![0u8; size];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut b);
    b.iter()
        .map(|b| SYMBOLS[(b % base) as usize] as char)
        .collect()
}

/// Go's `t.UTC().Format(time.RFC3339Nano)`: `2006-01-02T15:04:05.999999999Z`, the fraction
/// without trailing zeros and left out when it is zero.
fn rfc3339_nano(t: SystemTime) -> String {
    let d = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs() as i64;
    let nanos = d.subsec_nanos();
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let mut s = format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    );
    if nanos != 0 {
        let frac = format!("{nanos:09}");
        s.push('.');
        s.push_str(frac.trim_end_matches('0'));
    }
    s.push('Z');
    s
}

/// `FindTagValue`, `helpers.go:21-28`.
pub fn find_tag_value(b: &[u8], tag: &str) -> String {
    // Go's `\w` and `\b` are ASCII: Unicode off.
    let re = regex::bytes::Regex::new(&format!(r"(?s-u)<(?:\w+:)?{tag}\b[^>]*>([^<]+)")).unwrap();
    match re.captures(b) {
        Some(m) => String::from_utf8_lossy(&m[1]).into_owned(),
        None => String::new(),
    }
}

/// `atoi`, `helpers.go:120-129`: 0 for "", -1 when it is not a number.
fn atoi(s: &str) -> i64 {
    if s.is_empty() {
        return 0;
    }
    s.parse::<i64>().unwrap_or(-1)
}

/// `GetPath`, `helpers.go:153-162` (a path that starts with "/" gives the default, as there).
pub fn get_path(url_or_path: &str, def_path: &str) -> String {
    if url_or_path.is_empty() || url_or_path.starts_with('/') {
        return def_path.to_owned();
    }
    match url::Url::parse(url_or_path) {
        Ok(u) => get_path(u.path(), def_path),
        Err(_) => def_path.to_owned(),
    }
}

/// Go's `html.UnescapeString` (`html/escape.go:164`, `unescapeEntity` `:36`). The numeric
/// references and the `replacementTable` are complete; of the ~2200 named references only
/// `amp`, `lt`, `gt`, `quot` (also `AMP`, `LT`, `GT`, `QUOT`, and those four without the
/// semicolon) and `apos;` are in the table here. Others stay as written.
pub fn html_unescape(s: &str) -> String {
    const REPLACEMENT_TABLE: [u32; 32] = [
        0x20AC, 0x0081, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160,
        0x2039, 0x0152, 0x008D, 0x017D, 0x008F, 0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022,
        0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x009D, 0x017E, 0x0178,
    ];
    // `entity.go`: the names in the table (with ";" where Go lists them so).
    fn entity(name: &str) -> Option<char> {
        Some(match name {
            "amp;" | "AMP;" | "amp" | "AMP" => '&',
            "lt;" | "LT;" | "lt" | "LT" => '<',
            "gt;" | "GT;" | "gt" | "GT" => '>',
            "quot;" | "QUOT;" | "quot" | "QUOT" => '"',
            "apos;" => '\'',
            _ => return None,
        })
    }
    // `entity.go:10`
    const LONGEST_ENTITY_WITHOUT_SEMICOLON: usize = 6;

    let s = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(s.len());
    let mut src = 0;
    while src < s.len() {
        if s[src] != b'&' {
            out.push(s[src]);
            src += 1;
            continue;
        }
        // `unescapeEntity`, `escape.go:36-141`.
        let t = &s[src..];
        let mut i = 1;
        if t.len() <= 1 {
            out.push(s[src]);
            src += 1;
            continue;
        }

        if t[i] == b'#' {
            if t.len() <= 3 {
                // We need to have at least "&#.".
                out.push(s[src]);
                src += 1;
                continue;
            }
            i += 1;
            let mut c = t[i];
            let mut hex = false;
            if c == b'x' || c == b'X' {
                hex = true;
                i += 1;
            }

            let mut x: i32 = 0;
            while i < t.len() {
                c = t[i];
                i += 1;
                if hex {
                    if c.is_ascii_digit() {
                        x = x
                            .wrapping_mul(16)
                            .wrapping_add(i32::from(c) - i32::from(b'0'));
                        continue;
                    } else if (b'a'..=b'f').contains(&c) {
                        x = x
                            .wrapping_mul(16)
                            .wrapping_add(i32::from(c) - i32::from(b'a') + 10);
                        continue;
                    } else if (b'A'..=b'F').contains(&c) {
                        x = x
                            .wrapping_mul(16)
                            .wrapping_add(i32::from(c) - i32::from(b'A') + 10);
                        continue;
                    }
                } else if c.is_ascii_digit() {
                    x = x
                        .wrapping_mul(10)
                        .wrapping_add(i32::from(c) - i32::from(b'0'));
                    continue;
                }
                if c != b';' {
                    i -= 1;
                }
                break;
            }

            if i <= 3 {
                // No characters matched.
                out.push(s[src]);
                src += 1;
                continue;
            }

            let r: u32 = if (0x80..=0x9F).contains(&x) {
                // Replace characters from Windows-1252 with UTF-8 equivalents.
                REPLACEMENT_TABLE[(x - 0x80) as usize]
            } else if x == 0 || (0xD800..=0xDFFF).contains(&x) || !(0..=0x10FFFF).contains(&x) {
                // Replace invalid characters with the replacement character.
                0xFFFD
            } else {
                x as u32
            };
            let ch = char::from_u32(r).unwrap_or('\u{FFFD}');
            let mut buf = [0u8; 4];
            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            src += i;
            continue;
        }

        // Consume the maximum number of characters possible, with the consumed characters
        // matching one of the named references.
        while i < t.len() {
            let c = t[i];
            i += 1;
            if c.is_ascii_alphanumeric() {
                continue;
            }
            if c != b';' {
                i -= 1;
            }
            break;
        }

        let entity_name = &t[1..i];
        // The name is ASCII (letters, digits, ";").
        let name = std::str::from_utf8(entity_name).unwrap_or("");
        if entity_name.is_empty() {
            // No-op.
        } else if let Some(x) = entity(name) {
            let mut buf = [0u8; 4];
            out.extend_from_slice(x.encode_utf8(&mut buf).as_bytes());
            src += i;
            continue;
        } else {
            let mut max_len = entity_name.len() - 1;
            if max_len > LONGEST_ENTITY_WITHOUT_SEMICOLON {
                max_len = LONGEST_ENTITY_WITHOUT_SEMICOLON;
            }
            let mut j = max_len;
            let mut found = false;
            while j > 1 {
                if let Some(x) = entity(&name[..j]) {
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(x.encode_utf8(&mut buf).as_bytes());
                    src += j + 1;
                    found = true;
                    break;
                }
                j -= 1;
            }
            if found {
                continue;
            }
        }

        out.extend_from_slice(&s[src..src + i]);
        src += i;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::{BufRead as _, Read as _, Write as _};

    // go2rtc `pkg/onvif/onvif_test.go`: `TestGetStreamUri` (7 cases).
    const URIS: [(&str, &str, &str); 7] = [
        (
            "Dahua stream default",
            include_str!("onvif_fixtures/uri1.xml"),
            include_str!("onvif_fixtures/uri1.url"),
        ),
        (
            "Dahua snapshot default",
            include_str!("onvif_fixtures/uri2.xml"),
            include_str!("onvif_fixtures/uri2.url"),
        ),
        (
            "Dahua stream formatted",
            include_str!("onvif_fixtures/uri3.xml"),
            include_str!("onvif_fixtures/uri3.url"),
        ),
        (
            "Dahua snapshot formatted",
            include_str!("onvif_fixtures/uri4.xml"),
            include_str!("onvif_fixtures/uri4.url"),
        ),
        (
            "Unknown",
            include_str!("onvif_fixtures/uri5.xml"),
            include_str!("onvif_fixtures/uri5.url"),
        ),
        (
            "go2rtc 1.9.4",
            include_str!("onvif_fixtures/uri6.xml"),
            include_str!("onvif_fixtures/uri6.url"),
        ),
        (
            "go2rtc 1.9.8",
            include_str!("onvif_fixtures/uri7.xml"),
            include_str!("onvif_fixtures/uri7.url"),
        ),
    ];

    #[test]
    fn get_stream_uri() {
        for (name, xml, want) in URIS {
            let uri = find_tag_value(xml.as_bytes(), "Uri");
            let uri = html_unescape(&uri);
            let uri = uri.trim();
            let u = url::Url::parse(uri).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(u.to_string(), want, "{name}");
        }
    }

    // `TestGetCapabilities` (2 cases).
    #[test]
    fn get_capabilities() {
        for (name, xml) in [
            ("Dahua default", include_str!("onvif_fixtures/caps1.xml")),
            ("Dahua formatted", include_str!("onvif_fixtures/caps2.xml")),
        ] {
            let raw = find_tag_value(xml.as_bytes(), "Media.+?XAddr");
            assert_eq!(raw, "http://192.168.1.123/onvif/media_service", "{name}");
            let raw = find_tag_value(xml.as_bytes(), "Imaging.+?XAddr");
            assert_eq!(raw, "http://192.168.1.123/onvif/imaging_service", "{name}");
        }
    }

    /// A stand-in camera as the Go harness's `httptest` server: it records every request as
    /// `METHOD path\ncontent-type\nuser-agent\nbody` and answers the same texts.
    pub(crate) fn camera() -> (String, std::sync::mpsc::Receiver<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let host = listener.local_addr().unwrap().to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let mut it = line.split_whitespace();
                let (method, path) = (it.next().unwrap().to_owned(), it.next().unwrap().to_owned());
                let (mut len, mut ctype, mut agent) = (0usize, String::new(), String::new());
                loop {
                    let mut h = String::new();
                    reader.read_line(&mut h).unwrap();
                    let h = h.trim_end();
                    if h.is_empty() {
                        break;
                    }
                    let (k, v) = h.split_once(':').unwrap();
                    let v = v.trim().to_owned();
                    match k.to_ascii_lowercase().as_str() {
                        "content-length" => len = v.parse().unwrap(),
                        "content-type" => ctype = v,
                        "user-agent" => agent = v,
                        _ => {}
                    }
                }
                let mut body = vec![0u8; len];
                reader.read_exact(&mut body).unwrap();
                let s = String::from_utf8(body).unwrap();
                let answer = if s.contains("GetCapabilities") {
                    r#"<tds:GetCapabilitiesResponse><tt:Media><tt:XAddr>http://x/onvif/media_service</tt:XAddr></tt:Media><tt:Imaging><tt:XAddr>http://x/onvif/imaging_service</tt:XAddr></tt:Imaging></tds:GetCapabilitiesResponse>"#
                } else if s.contains("GetProfiles") {
                    r#"<trt:GetProfilesResponse><trt:Profiles token="prof_a" fixed="true"><tt:Name>a</tt:Name></trt:Profiles><trt:Profiles token="prof_b"></trt:Profiles></trt:GetProfilesResponse>"#
                } else if s.contains("GetStreamUri") {
                    r#"<trt:GetStreamUriResponse><trt:MediaUri><tt:Uri>rtsp://192.168.1.123:554/cam?channel=1&amp;subtype=1</tt:Uri></trt:MediaUri></trt:GetStreamUriResponse>"#
                } else {
                    ""
                };
                tx.send(format!("{method} {path}\n{ctype}\n{agent}\n{s}"))
                    .unwrap();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                    answer.len()
                )
                .unwrap();
            }
        });
        (host, rx)
    }

    fn fixture(n: usize) -> String {
        let f = match n {
            1 => include_str!("onvif_fixtures/req1.txt"),
            2 => include_str!("onvif_fixtures/req2.txt"),
            3 => include_str!("onvif_fixtures/req3.txt"),
            4 => include_str!("onvif_fixtures/req4.txt"),
            5 => include_str!("onvif_fixtures/req5.txt"),
            6 => include_str!("onvif_fixtures/req6.txt"),
            7 => include_str!("onvif_fixtures/req7.txt"),
            8 => include_str!("onvif_fixtures/req8.txt"),
            9 => include_str!("onvif_fixtures/req9.txt"),
            10 => include_str!("onvif_fixtures/req10.txt"),
            11 => include_str!("onvif_fixtures/req11.txt"),
            12 => include_str!("onvif_fixtures/req12.txt"),
            13 => include_str!("onvif_fixtures/req13.txt"),
            14 => include_str!("onvif_fixtures/req14.txt"),
            15 => include_str!("onvif_fixtures/req15.txt"),
            16 => include_str!("onvif_fixtures/req16.txt"),
            17 => include_str!("onvif_fixtures/req17.txt"),
            _ => include_str!("onvif_fixtures/req18.txt"),
        };
        f.to_owned()
    }

    /// The Go harness's 18 requests (`onvif_fixtures/make-fixtures.sh`, go2rtc's own client run
    /// against its server with a fixed nonce and time): the same calls here send the same
    /// method, path, content type, user agent and body, byte for byte, and read the same
    /// addresses.
    #[test]
    fn requests_identical_to_go2rtc() {
        let (host, rx) = camera();
        let fixed = || {
            Some((
                "abcdefghij012345".to_owned(),
                "2026-11-21T10:20:30.5Z".to_owned(),
            ))
        };
        let mut n = 0;
        let mut check = |rx: &std::sync::mpsc::Receiver<String>| {
            n += 1;
            let got = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(got, fixture(n), "request {n}");
        };
        for (raw, want) in [
            (
                format!("onvif://admin:s3cret@{host}?subtype=1"),
                "rtsp://admin:s3cret@192.168.1.123:554/cam?channel=1&subtype=1",
            ),
            (
                format!("onvif://{host}"),
                "rtsp://192.168.1.123:554/cam?channel=1&subtype=1",
            ),
        ] {
            let c = Client::new_with(&raw, fixed()).unwrap();
            check(&rx); // GetCapabilities
            assert_eq!(c.get_uri().unwrap(), want);
            check(&rx); // GetProfiles
            check(&rx); // GetStreamUri
            let _ = c.get_snapshot_uri("t1");
            check(&rx);
            let _ = c.get_video_source_configuration("t2");
            check(&rx);
            let _ = c.get_profile("t3");
            check(&rx);
            let _ = c.get_service_capabilities();
            check(&rx);
            let _ = c.get_name();
            check(&rx);
            let _ = c.has_snapshots();
            check(&rx);
        }
        assert_eq!(n, 18);
    }

    #[test]
    fn subtype_picks_the_profile() {
        let (host, rx) = camera();
        // No subtype: the first profile (`atoi("") == 0`); wrong one: an error.
        let c = Client::new(&format!("onvif://{host}")).unwrap();
        c.get_uri().unwrap();
        let mut sent = Vec::new();
        while let Ok(r) = rx.try_recv() {
            sent.push(r);
        }
        assert!(sent[2].contains("<trt:ProfileToken>prof_a</trt:ProfileToken>"));
        let c = Client::new(&format!("onvif://{host}?subtype=2")).unwrap();
        assert_eq!(c.get_uri().unwrap_err(), "onvif: wrong subtype");
        // Not a number: used as the profile token itself, as go2rtc does.
        let c = Client::new(&format!("onvif://{host}?subtype=my_token")).unwrap();
        c.get_uri().unwrap();
        let sent: Vec<String> = rx.try_iter().collect();
        assert!(
            sent.last()
                .unwrap()
                .contains("<trt:ProfileToken>my_token</trt:ProfileToken>")
        );
    }

    #[test]
    fn atoi_as_go() {
        assert_eq!(atoi(""), 0);
        assert_eq!(atoi("3"), 3);
        assert_eq!(atoi("+3"), 3);
        assert_eq!(atoi("-3"), -3);
        assert_eq!(atoi("x"), -1);
        assert_eq!(atoi("99999999999999999999"), -1);
    }

    #[test]
    fn get_path_as_go() {
        // A path starting with "/" gives the default (helpers.go:154).
        assert_eq!(get_path("", "/d"), "/d");
        assert_eq!(get_path("/onvif/media", "/d"), "/d");
        assert_eq!(get_path("http://h/onvif/media", "/d"), "/d");
    }

    #[test]
    fn html_unescape_as_go() {
        assert_eq!(html_unescape("a&amp;b&lt;&gt;&quot;&apos;"), "a&b<>\"'");
        assert_eq!(html_unescape("&amp"), "&"); // without the semicolon
        assert_eq!(html_unescape("&ampx"), "&x"); // longest prefix
        assert_eq!(html_unescape("&#65;&#x42;&#X43"), "ABC");
        assert_eq!(html_unescape("&#128;"), "\u{20AC}");
        assert_eq!(
            html_unescape("&#0;&#xD800;&#x110000;"),
            "\u{FFFD}\u{FFFD}\u{FFFD}"
        );
        assert_eq!(html_unescape("&#;&"), "&#;&");
        assert_eq!(html_unescape("&unknown;"), "&unknown;");
        assert_eq!(html_unescape("no entity"), "no entity");
    }

    #[test]
    fn created_as_rfc3339_nano() {
        let t = |s, n| UNIX_EPOCH + Duration::new(s, n);
        assert_eq!(rfc3339_nano(t(0, 0)), "1970-01-01T00:00:00Z");
        assert_eq!(
            rfc3339_nano(t(1_795_256_430, 500_000_000)),
            "2026-11-21T10:20:30.5Z"
        );
        assert_eq!(
            rfc3339_nano(t(951_782_400, 123_456_789)),
            "2000-02-29T00:00:00.123456789Z"
        );
    }

    #[test]
    fn nonce_symbols() {
        let n = rand_string(16, 36);
        assert_eq!(n.len(), 16);
        assert!(n.bytes().all(|c| c.is_ascii_alphanumeric()));
    }
}
