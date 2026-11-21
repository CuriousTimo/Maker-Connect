//! Fragmented MP4 for MSE, one H.264 video track, ported from go2rtc (AlexxIT/go2rtc v1.9.15,
//! `7c05066`, Copyright (c) 2022 Alexey Khit, MIT), function by function:
//! `pkg/mp4/muxer.go` (`Muxer`), `pkg/mp4/mime.go` (`ContentType`), `pkg/h264/mpeg4.go`
//! (`EncodeConfig`, `DecodeConfig`), `pkg/h264/h264.go` (`NALUType`, `IsKeyframe`,
//! `GetProfileLevelID`, `JoinNALU`) and the start-of-access-unit part of `pkg/h264/rtp.go`
//! `RTPDepay` (`start_of_access_unit`).
//!
//! Where the camera side is retina's (docs/archive/plans/cameras.md, "Where the two stacks meet"): the frame
//! (AVC, length-prefixed NAL units), its 32-bit RTP timestamp, SPS/PPS from retina's `avcC`
//! through `decode_config`, width and height from retina (go2rtc decodes them from the SPS).
//! go2rtc's `GetProfileLevelID` reads the SPS from the SDP's `sprop-parameter-sets`; here the
//! SPS is retina's. `cts` is 0, as go2rtc's `ExtensionProfile` is for RTP.

use super::iso::{self, Movie};

pub const NALU_TYPE_P_FRAME: u8 = 1;
pub const NALU_TYPE_I_FRAME: u8 = 5;
pub const NALU_TYPE_SEI: u8 = 6;
pub const NALU_TYPE_SPS: u8 = 7;
pub const NALU_TYPE_PPS: u8 = 8;
pub const NALU_TYPE_AUD: u8 = 9;

const PROFILE_BASELINE: u8 = 0x42;
const PROFILE_MAIN: u8 = 0x4D;
const PROFILE_HIGH: u8 = 0x64;
const CAPABILITY_BASELINE: u8 = 0xE0;
const CAPABILITY_MAIN: u8 = 0x40;

/// `h264.NALUType`: the type of the first NAL unit (after its 4-byte length).
pub fn nalu_type(b: &[u8]) -> u8 {
    b[4] & 0x1F
}

/// `h264.IsKeyframe`.
pub fn is_keyframe(mut b: &[u8]) -> bool {
    loop {
        match nalu_type(b) {
            NALU_TYPE_P_FRAME => return false,
            NALU_TYPE_I_FRAME => return true,
            _ => {}
        }
        let size = u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize + 4;
        if size < b.len() {
            b = &b[size..];
        } else {
            return false;
        }
    }
}

/// `h264.JoinNALU`: each with its 4-byte length.
pub fn join_nalu(nalus: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for nalu in nalus {
        if !nalu.is_empty() {
            out.extend_from_slice(&(nalu.len() as u32).to_be_bytes());
            out.extend_from_slice(nalu);
        }
    }
    out
}

/// `h264.EncodeConfig`: the `avcC` record of one SPS and one PPS.
pub fn encode_config(sps: &[u8], pps: &[u8]) -> Vec<u8> {
    let sps_size = sps.len() as u16;
    let pps_size = pps.len() as u16;
    let mut buf = vec![0u8; 5 + 3 + sps.len() + 3 + pps.len()];
    buf[0] = 1;
    buf[1..4].copy_from_slice(&sps[1..4]); // profile
    buf[4] = 3 | 0xFC; // ? LengthSizeMinusOne

    let b = &mut buf[5..];
    b[0] = 1 | 0xE0; // ? sps count
    b[1..3].copy_from_slice(&sps_size.to_be_bytes());
    b[3..3 + sps.len()].copy_from_slice(sps);

    let b = &mut buf[5 + 3 + sps.len()..];
    b[0] = 1; // pps count
    b[1..3].copy_from_slice(&pps_size.to_be_bytes());
    b[3..3 + pps.len()].copy_from_slice(pps);

    buf
}

/// `h264.DecodeConfig`: the first SPS and PPS of an `avcC` record.
/// `DecodeConfig`'s three results: profile, SPS, PPS.
pub type Config<'a> = (Option<&'a [u8]>, Option<&'a [u8]>, Option<&'a [u8]>);

pub fn decode_config(conf: &[u8]) -> Config<'_> {
    if conf.len() < 6 || conf[0] != 1 {
        return (None, None, None);
    }
    let profile = Some(&conf[1..4]);
    let (mut sps, mut pps) = (None, None);
    let count = conf[5] & 0x1F;
    let mut conf = &conf[6..];
    for _ in 0..count {
        if conf.len() < 2 {
            return (profile, sps, pps);
        }
        let size = 2 + u16::from_be_bytes([conf[0], conf[1]]) as usize;
        if conf.len() < size {
            return (profile, sps, pps);
        }
        if sps.is_none() {
            sps = Some(&conf[2..size]);
        }
        conf = &conf[size..];
    }
    let Some((&count, rest)) = conf.split_first() else {
        return (profile, sps, pps);
    };
    conf = rest;
    for _ in 0..count {
        if conf.len() < 2 {
            return (profile, sps, pps);
        }
        let size = 2 + u16::from_be_bytes([conf[0], conf[1]]) as usize;
        if conf.len() < size {
            return (profile, sps, pps);
        }
        if pps.is_none() {
            pps = Some(&conf[2..size]);
        }
        conf = &conf[size..];
    }
    (profile, sps, pps)
}

/// `h264.GetProfileLevelID`, from the SPS (go2rtc's first case, `sprop-parameter-sets`):
/// avc1.640029 (High 4.1) unless the SPS says a profile, capability and level it lets through.
pub fn profile_level_id(sps: Option<&[u8]>) -> String {
    let (mut profile, mut capab, mut level) = (PROFILE_HIGH, 0u8, 41u8);
    if let Some(sps) = sps
        && sps.len() >= 4
    {
        let conf = &sps[1..4];
        if matches!(conf[0], PROFILE_BASELINE | PROFILE_MAIN) {
            profile = conf[0];
        }
        if matches!(conf[1], CAPABILITY_BASELINE | CAPABILITY_MAIN) {
            capab = conf[1];
        }
        if matches!(conf[2], 30 | 31 | 40) {
            level = conf[2];
        }
    }
    format!("{profile:02X}{capab:02X}{level:02X}")
}

/// `mp4.ContentType` for one H.264 track.
pub fn content_type(sps: Option<&[u8]>) -> String {
    format!(r#"video/mp4; codecs="avc1.{}""#, profile_level_id(sps))
}

/// The start-of-access-unit part of go2rtc's `h264.RTPDepay` (`rtp.go:52-75`), on a whole
/// frame: an I-frame without SPS/PPS gets `ps` before it; leading SEI and AUD units are
/// skipped (a frame of only one is dropped); a frame starting with an unknown unit is dropped.
pub fn start_of_access_unit(mut payload: &[u8], ps: &[u8]) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    loop {
        if payload.len() < 5 {
            return None;
        }
        match nalu_type(payload) {
            NALU_TYPE_I_FRAME => buf.extend_from_slice(ps),
            NALU_TYPE_SEI | NALU_TYPE_AUD => {
                let i = 4 + u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]])
                    as usize;
                if i >= payload.len() {
                    return None;
                }
                payload = &payload[i..];
                continue;
            }
            NALU_TYPE_P_FRAME | NALU_TYPE_SPS | NALU_TYPE_PPS => {}
            _ => return None,
        }
        break;
    }
    buf.extend_from_slice(payload);
    Some(buf)
}

/// go2rtc's `Muxer`, one video track (track id 1, clock rate 90 kHz).
#[derive(Default)]
pub struct Muxer {
    index: u32,
    dts: u64,
    pts: u32,
}

/// The H.264 clock rate (go2rtc's `codec.ClockRate` for H.264).
const CLOCK_RATE: u32 = 90000;

impl Muxer {
    pub fn new() -> Muxer {
        Muxer {
            index: 0,
            dts: 0,
            pts: 0,
        }
    }

    /// `GetInit` for one H.264 track: go2rtc's dummy SPS/PPS when missing, and 1920x1080
    /// when the size is unknown.
    pub fn get_init(sps: Option<&[u8]>, pps: Option<&[u8]>, size: Option<(u16, u16)>) -> Vec<u8> {
        let mut mv = Movie::new(1024);
        mv.write_file_type();

        mv.start_atom(iso::MOOV);
        mv.write_movie_header();

        // some dummy SPS and PPS not a problem for MP4, but problem for HLS :(
        let sps = sps
            .filter(|s| !s.is_empty())
            .unwrap_or(&[0x67, 0x42, 0x00, 0x0a, 0xf8, 0x41, 0xa2]);
        let pps = pps
            .filter(|p| !p.is_empty())
            .unwrap_or(&[0x68, 0xce, 0x38, 0x80]);
        let (width, height) = size.unwrap_or((1920, 1080));
        mv.write_video_track(1, CLOCK_RATE, width, height, &encode_config(sps, pps));

        mv.start_atom(iso::MOOV_MVEX);
        mv.write_track_extend(1);
        mv.end_atom(); // MVEX

        mv.end_atom(); // MOOV

        mv.into_bytes()
    }

    /// `GetPayload`: one `moof` + `mdat` for one frame at its RTP `timestamp`.
    pub fn get_payload(&mut self, timestamp: u32, payload: &[u8]) -> Vec<u8> {
        self.index += 1;

        let mut duration = timestamp.wrapping_sub(self.pts);
        self.pts = timestamp;

        // flags important for Apple Finder video preview
        let flags = if is_keyframe(payload) {
            iso::SAMPLE_VIDEO_I_FRAME
        } else {
            iso::SAMPLE_VIDEO_NON_I_FRAME
        };

        // minumum duration important for MSE in Apple Safari
        if duration == 0 || duration > CLOCK_RATE {
            duration = CLOCK_RATE / 1000 + 1;
            self.pts = self.pts.wrapping_add(duration);
        }

        let size = payload.len();

        let mut mv = Movie::new(1024 + size);
        mv.write_movie_fragment(self.index, 1, duration, size as u32, flags, self.dts, 0);
        mv.write_data(payload);

        self.dts += u64::from(duration);

        mv.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nal(kind: u8, len: usize) -> Vec<u8> {
        let mut n = vec![kind];
        n.resize(len, 0xAB);
        n
    }

    /// The start-of-access-unit filter, case by case as `rtp.go:52-75`.
    #[test]
    fn the_start_of_an_access_unit_is_as_go2rtc_does() {
        let ps = join_nalu(&[&nal(0x67, 8), &nal(0x68, 4)]);
        // An I-frame without SPS/PPS gets them before it.
        let idr = join_nalu(&[&nal(0x65, 20)]);
        assert_eq!(
            start_of_access_unit(&idr, &ps).unwrap(),
            [ps.clone(), idr.clone()].concat()
        );
        // One with them stays as it is.
        let full = join_nalu(&[&nal(0x67, 8), &nal(0x68, 4), &nal(0x65, 20)]);
        assert_eq!(start_of_access_unit(&full, &ps).unwrap(), full);
        // Leading SEI and AUD are skipped; a frame of only one is dropped.
        let p = join_nalu(&[&nal(0x41, 10)]);
        let with_sei = join_nalu(&[&nal(0x09, 2), &nal(0x06, 5), &nal(0x41, 10)]);
        assert_eq!(start_of_access_unit(&with_sei, &ps).unwrap(), p);
        assert!(start_of_access_unit(&join_nalu(&[&nal(0x06, 5)]), &ps).is_none());
        // A frame starting with an unknown unit is dropped.
        assert!(start_of_access_unit(&join_nalu(&[&nal(0x0C, 5)]), &ps).is_none());
    }

    /// `GetProfileLevelID`'s sanitising, and the default.
    #[test]
    fn the_codec_string_is_go2rtc_s() {
        assert_eq!(profile_level_id(None), "640029");
        // The LSC: Main 4.0.
        assert_eq!(profile_level_id(Some(&[0x67, 0x4D, 0x00, 0x28])), "4D0028");
        // High 5.1 with an odd capability: High 4.1, capability 0.
        assert_eq!(profile_level_id(Some(&[0x67, 0x64, 0x01, 0x33])), "640029");
        assert_eq!(
            content_type(Some(&[0x67, 0x42, 0xE0, 0x1F])),
            r#"video/mp4; codecs="avc1.42E01F""#
        );
    }

    #[test]
    fn a_config_goes_there_and_back() {
        let (sps, pps) = (nal(0x67, 9), nal(0x68, 4));
        let conf = encode_config(&sps, &pps);
        assert_eq!(&conf[..5], &[1, 0xAB, 0xAB, 0xAB, 0xFF]);
        let (profile, s, p) = decode_config(&conf);
        assert_eq!(profile.unwrap(), &sps[1..4]);
        assert_eq!(s.unwrap(), &sps[..]);
        assert_eq!(p.unwrap(), &pps[..]);
    }

    /// go2rtc's timeline: the first frame's step is its whole timestamp, so it becomes 91 and
    /// `pts` moves by 91; the next frame's duration is measured from there.
    #[test]
    fn the_first_durations_are_go2rtc_s() {
        let mut m = Muxer::new();
        let idr = join_nalu(&[&nal(0x65, 20)]);
        let p = join_nalu(&[&nal(0x41, 10)]);
        let a = m.get_payload(900_000, &idr);
        let b = m.get_payload(904_500, &p);
        let duration = |f: &[u8]| u32::from_be_bytes(f[48..52].try_into().unwrap());
        // moof(8) mfhd(16) traf(8), then tfhd at 32: size, name, version+flags, track; duration at 48.
        assert_eq!(duration(&a), 91);
        assert_eq!(duration(&b), 4500 - 91);
        assert_eq!(
            u32::from_be_bytes(a[20..24].try_into().unwrap()),
            1,
            "sequence 1"
        );
        assert_eq!(
            u32::from_be_bytes(b[20..24].try_into().unwrap()),
            2,
            "sequence 2"
        );
    }
}
