//! A Buddy printer's `network_info` (INFO), kept as Prusa Connect's server keeps it (owner,
//! 2026-11-24, #201: "netwerk zoals de server"): `reference/prusa-connect` at `2e8c62b`,
//! `internal/connector/xbuddy/network.go` (`networkValue`, `macText`, `ipv4Text`,
//! `hostnameText`; read only, OCL, nothing copied) and what those call in Go 1.26.11 (the
//! server's `go.mod`), ported here: `net.ParseMAC` and `HardwareAddr.String` (`src/net/mac.go`),
//! `xtoi`/`xtoi2` (`src/net/parse.go`), `net.ParseIP` and `IP.To4` (`src/net/ip.go`),
//! `netip.ParseAddr` and `parseIPv4Fields` (`src/net/netip/netip.go`), `unicode.Cf` (Unicode
//! 15.0.0, `src/unicode/tables.go`) and `machinetext.Clip` (`internal/kernel/machinetext`).
//! Go's sources are BSD-3-Clause.
//!
//! Differences: an address with a `:` is parsed by Rust's `std::net::Ipv6Addr`, not a port of
//! `netip.parseIPv6`; only an IPv4-mapped one gives a value (`To4`), and Buddy writes only
//! dotted IPv4 (`JSON_IP`, `render.cpp:418`, `:426`).

/// `maxHostnameBytes` (`network.go:33`): a DNS label is at most 63 bytes.
const MAX_HOSTNAME_BYTES: usize = 64;

/// `machinetext.Marker`.
const MARKER: &str = "…";

/// `networkValue` (`network.go:80-90`): the value kept for one `network_info` key, None when the
/// printer sent something that is not one (the server then removes the node).
pub(crate) fn network_value(key: &str, value: &str) -> Option<String> {
    let kept = if key.ends_with("_mac") {
        mac_text(value)
    } else if key.ends_with("_ipv4") {
        ipv4_text(value)
    } else {
        hostname_text(value)
    };
    Some(kept).filter(|v| !v.is_empty())
}

/// `macText` (`network.go:92-100`): a 48-bit MAC in lower case with colons, or "".
fn mac_text(value: &str) -> String {
    match parse_mac(value) {
        Some(hw) if hw.len() == 6 => hardware_addr_string(&hw),
        _ => String::new(),
    }
}

/// `HardwareAddr.String` (`mac.go`): lower-case hex pairs joined by colons.
fn hardware_addr_string(a: &[u8]) -> String {
    a.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// `net.ParseMAC` (`mac.go`): MAC-48, EUI-48, EUI-64 or a 20-octet address, with `:` or `-`
/// between pairs, `.` between quads, or bare hex.
fn parse_mac(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if b.len() < 12 {
        return None;
    }
    if b[2] == b':' || b[2] == b'-' {
        if !(b.len() + 1).is_multiple_of(3) {
            return None;
        }
        let n = (b.len() + 1) / 3;
        if n != 6 && n != 8 && n != 20 {
            return None;
        }
        let mut hw = vec![0; n];
        let mut x = 0;
        for byte in hw.iter_mut() {
            *byte = xtoi2(&b[x..], b[2])?;
            x += 3;
        }
        Some(hw)
    } else if b[4] == b'.' {
        if !(b.len() + 1).is_multiple_of(5) {
            return None;
        }
        let n = 2 * (b.len() + 1) / 5;
        if n != 6 && n != 8 && n != 20 {
            return None;
        }
        let mut hw = vec![0; n];
        let mut x = 0;
        for i in (0..n).step_by(2) {
            hw[i] = xtoi2(&b[x..x + 2], 0)?;
            hw[i + 1] = xtoi2(&b[x + 2..], b[4])?;
            x += 5;
        }
        Some(hw)
    } else {
        if !b.len().is_multiple_of(2) {
            return None;
        }
        let n = b.len() / 2;
        if n != 6 && n != 8 && n != 20 {
            return None;
        }
        let mut hw = vec![0; n];
        let mut x = 0;
        for byte in hw.iter_mut() {
            *byte = xtoi2(&b[x..x + 2], 0)?;
            x += 2;
        }
        Some(hw)
    }
}

/// `xtoi2` (`parse.go`): two hex digits, then `e` (if anything follows).
fn xtoi2(s: &[u8], e: u8) -> Option<u8> {
    if s.len() > 2 && s[2] != e {
        return None;
    }
    let (n, ei) = xtoi(&s[..2.min(s.len())])?;
    (ei == 2).then_some(n as u8)
}

/// `xtoi` (`parse.go`): hex digits from the start; the value, how many were read. (`big`, the
/// overflow bound, is never reached by two digits.)
fn xtoi(s: &[u8]) -> Option<(u32, usize)> {
    let mut n = 0;
    let mut i = 0;
    while i < s.len() {
        let d = match s[i] {
            c @ b'0'..=b'9' => c - b'0',
            c @ b'a'..=b'f' => c - b'a' + 10,
            c @ b'A'..=b'F' => c - b'A' + 10,
            _ => break,
        };
        n = n * 16 + u32::from(d);
        i += 1;
    }
    (i != 0).then_some((n, i))
}

/// `ipv4Text` (`network.go:102-110`): `net.ParseIP(value).To4()` in dotted form, or "".
fn ipv4_text(value: &str) -> String {
    parse_ip_to4(value).map_or_else(String::new, |[a, b, c, d]| format!("{a}.{b}.{c}.{d}"))
}

/// `net.ParseIP` (no zone) then `To4`: `netip.ParseAddr` picks the form by the first `.`, `:`
/// or `%`.
fn parse_ip_to4(s: &str) -> Option<[u8; 4]> {
    match s.bytes().find(|c| matches!(c, b'.' | b':' | b'%'))? {
        b'.' => parse_ipv4_fields(s),
        b':' => {
            if s.contains('%') {
                return None; // a zone: `parseIP` refuses it
            }
            s.parse::<std::net::Ipv6Addr>()
                .ok()?
                .to_ipv4_mapped()
                .map(|v4| v4.octets())
        }
        _ => None,
    }
}

/// `parseIPv4Fields` (`netip.go`): four decimal fields up to 255, no leading zero.
fn parse_ipv4_fields(s: &str) -> Option<[u8; 4]> {
    let b = s.as_bytes();
    let mut fields = [0u8; 4];
    let (mut val, mut pos, mut dig_len) = (0u32, 0, 0);
    for i in 0..b.len() {
        match b[i] {
            c @ b'0'..=b'9' => {
                if dig_len == 1 && val == 0 {
                    return None;
                }
                val = val * 10 + u32::from(c - b'0');
                dig_len += 1;
                if val > 255 {
                    return None;
                }
            }
            b'.' => {
                if i == 0 || i == b.len() - 1 || b[i - 1] == b'.' || pos == 3 {
                    return None;
                }
                fields[pos] = val as u8;
                pos += 1;
                val = 0;
                dig_len = 0;
            }
            _ => return None,
        }
    }
    if pos < 3 {
        return None;
    }
    fields[3] = val as u8;
    Some(fields)
}

/// `hostnameText` (`network.go:112-124`): control and format characters dropped, spaces
/// trimmed, cut to `maxHostnameBytes`.
fn hostname_text(value: &str) -> String {
    let kept: String = value
        .chars()
        .filter(|&r| !(r.is_control() || is_format(r)))
        .collect();
    clip(kept.trim(), MAX_HOSTNAME_BYTES)
}

/// `unicode.Is(unicode.Cf, r)`: Unicode 15.0.0's format characters (`tables.go` `_Cf`).
fn is_format(r: char) -> bool {
    const CF: [(u32, u32, u32); 18] = [
        (0x00ad, 0x0600, 1363),
        (0x0601, 0x0605, 1),
        (0x061c, 0x06dd, 193),
        (0x070f, 0x0890, 385),
        (0x0891, 0x08e2, 81),
        (0x180e, 0x200b, 2045),
        (0x200c, 0x200f, 1),
        (0x202a, 0x202e, 1),
        (0x2060, 0x2064, 1),
        (0x2066, 0x206f, 1),
        (0xfeff, 0xfff9, 250),
        (0xfffa, 0xfffb, 1),
        (0x110bd, 0x110cd, 16),
        (0x13430, 0x1343f, 1),
        (0x1bca0, 0x1bca3, 1),
        (0x1d173, 0x1d17a, 1),
        (0xe0001, 0xe0020, 31),
        (0xe0021, 0xe007f, 1),
    ];
    let c = u32::from(r);
    CF.iter()
        .any(|&(lo, hi, stride)| lo <= c && c <= hi && (c - lo).is_multiple_of(stride))
}

/// `machinetext.Clip` (`machinetext.go:34-45`): at most `limit` bytes, cut on a character
/// boundary, with the marker added.
fn clip(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut cut = limit;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}{MARKER}", &text[..cut])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Go 1.26.11's `parseMACTests` (`src/net/mac_test.go:13-93`), every row: the bytes, or none;
    /// and, as `TestParseMAC` checks, `String` parses back to the same bytes.
    #[test]
    fn parse_mac_is_gos() {
        let ok: [(&str, &[u8]); 15] = [
            ("00:00:5e:00:53:01", &[0x00, 0x00, 0x5e, 0x00, 0x53, 0x01]),
            ("00-00-5e-00-53-01", &[0x00, 0x00, 0x5e, 0x00, 0x53, 0x01]),
            ("0000.5e00.5301", &[0x00, 0x00, 0x5e, 0x00, 0x53, 0x01]),
            ("00005e005301", &[0x00, 0x00, 0x5e, 0x00, 0x53, 0x01]),
            (
                "02:00:5e:10:00:00:00:01",
                &[0x02, 0x00, 0x5e, 0x10, 0x00, 0x00, 0x00, 0x01],
            ),
            (
                "02-00-5e-10-00-00-00-01",
                &[0x02, 0x00, 0x5e, 0x10, 0x00, 0x00, 0x00, 0x01],
            ),
            (
                "0200.5e10.0000.0001",
                &[0x02, 0x00, 0x5e, 0x10, 0x00, 0x00, 0x00, 0x01],
            ),
            (
                "02005e1000000001",
                &[0x02, 0x00, 0x5e, 0x10, 0x00, 0x00, 0x00, 0x01],
            ),
            (
                "00:00:00:00:fe:80:00:00:00:00:00:00:02:00:5e:10:00:00:00:01",
                &[
                    0, 0, 0, 0, 0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0x02, 0, 0x5e, 0x10, 0, 0, 0, 0x01,
                ],
            ),
            (
                "00-00-00-00-fe-80-00-00-00-00-00-00-02-00-5e-10-00-00-00-01",
                &[
                    0, 0, 0, 0, 0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0x02, 0, 0x5e, 0x10, 0, 0, 0, 0x01,
                ],
            ),
            (
                "0000.0000.fe80.0000.0000.0000.0200.5e10.0000.0001",
                &[
                    0, 0, 0, 0, 0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0x02, 0, 0x5e, 0x10, 0, 0, 0, 0x01,
                ],
            ),
            (
                "00000000fe8000000000000002005e1000000001",
                &[
                    0, 0, 0, 0, 0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0x02, 0, 0x5e, 0x10, 0, 0, 0, 0x01,
                ],
            ),
            ("ab:cd:ef:AB:CD:EF", &[0xab, 0xcd, 0xef, 0xab, 0xcd, 0xef]),
            (
                "ab:cd:ef:AB:CD:EF:ab:cd",
                &[0xab, 0xcd, 0xef, 0xab, 0xcd, 0xef, 0xab, 0xcd],
            ),
            (
                "ab:cd:ef:AB:CD:EF:ab:cd:ef:AB:CD:EF:ab:cd:ef:AB:CD:EF:ab:cd",
                &[
                    0xab, 0xcd, 0xef, 0xab, 0xcd, 0xef, 0xab, 0xcd, 0xef, 0xab, 0xcd, 0xef, 0xab,
                    0xcd, 0xef, 0xab, 0xcd, 0xef, 0xab, 0xcd,
                ],
            ),
        ];
        for (input, bytes) in ok {
            let hw = parse_mac(input).unwrap_or_else(|| panic!("{input}"));
            assert_eq!(hw, bytes, "{input}");
            assert_eq!(parse_mac(&hardware_addr_string(&hw)), Some(hw), "{input}");
        }
        for input in [
            "01.02.03.04.05.06",
            "01:02:03:04:05:06:",
            "x1:02:03:04:05:06",
            "01002:03:04:05:06",
            "01:02003:04:05:06",
            "01:02:03004:05:06",
            "01:02:03:04005:06",
            "01:02:03:04:05006",
            "01-02:03:04:05:06",
            "01:02-03-04-05-06",
            "0123:4567:89AF",
            "0123-4567-89AF",
            "0123456789AF0",
        ] {
            assert_eq!(parse_mac(input), None, "{input}");
        }
    }

    /// What the server keeps of each key (`network.go`): Buddy's upper-case MAC in lower case,
    /// only a 48-bit one; a dotted IPv4 without leading zeros, or an IPv4-mapped IPv6; a
    /// hostname without control or format characters, trimmed and cut at 64 bytes.
    #[test]
    fn network_values_are_the_servers() {
        let v = |k: &str, s: &str| network_value(k, s);
        assert_eq!(
            v("lan_mac", "10:9C:70:1A:2B:3C").as_deref(),
            Some("10:9c:70:1a:2b:3c")
        );
        assert_eq!(
            v("wifi_mac", "02:00:5e:10:00:00:00:01"),
            None,
            "EUI-64: not 48-bit"
        );
        assert_eq!(v("lan_mac", "nonsense"), None);
        assert_eq!(
            v("lan_ipv4", "192.168.60.40").as_deref(),
            Some("192.168.60.40")
        );
        assert_eq!(v("lan_ipv4", "192.168.060.40"), None, "a leading zero");
        assert_eq!(v("lan_ipv4", "192.168.60.256"), None);
        assert_eq!(v("lan_ipv4", "1.2.3"), None);
        assert_eq!(
            v("wifi_ipv4", "::ffff:10.0.0.7").as_deref(),
            Some("10.0.0.7")
        );
        assert_eq!(v("wifi_ipv4", "fe80::1"), None);
        assert_eq!(v("wifi_ipv4", ""), None);
        assert_eq!(
            v("hostname", " prusa-mini\u{7}\u{200b} ").as_deref(),
            Some("prusa-mini")
        );
        assert_eq!(v("hostname", "\u{0}"), None);
        let long = "ü".repeat(40); // 80 bytes
        let cut = v("hostname", &long).unwrap();
        assert_eq!(cut, format!("{}…", "ü".repeat(32)));
    }
}
