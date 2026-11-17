//! A USB webcam on the agent (docs/plans/cameras.md, "USB webcams"; docs/briefs/usb-cam-step1.md, step 1):
//! one camera by its nokhwa index, a JPEG every [`EVERY`] to the hub ([`protocol::CAMERA_FRAME`]).
//!
//! Capture: nokhwa 0.10.11 (owner's exception, 2026-11-16: "nokhwa overal"). The JPEG a UVC camera
//! gives has no Huffman tables; [`mjpeg2jpeg`] adds them, ported from FFmpeg's bitstream filter
//! `libavcodec/bsf/mjpeg2jpeg.c` (lines 36-142) with the tables of `libavcodec/jpegtabs.h` ([`tables`]),
//! FFmpeg master `f1d4dd2bd0ace9263b590b1f3a500190b8687bef` (2026-11-17), Copyright (c) 2010 Adrian Daerr
//! and Nicolas George, LGPL-2.1-or-later. Ours, and only ours (the brief names them): [`EVERY`], the
//! requested format, [`jpeg_end`], [`has_dht`], reading the camera all the time.

mod tables;

use maker_connect_protocol as protocol;
use nokhwa::{
    Camera,
    pixel_format::RgbFormat,
    utils::{CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType, Resolution},
};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tables::*;

/// How often a frame goes to the hub (ours: "every few seconds").
pub const EVERY: Duration = Duration::from_secs(5);

const APP0: u8 = 0xe0;

/// FFmpeg's `jpeg_header`: SOI and a JFIF APP0.
const JPEG_HEADER: [u8; 20] = [
    0xff, 0xd8, // SOI
    0xff, 0xe0, // APP0
    0x00, 0x10, // APP0 header size
    0x4a, 0x46, 0x49, 0x46, 0x00, // 'JFIF\0'
    0x01, 0x01, // version
    0x00, // bits per type
    0x00, 0x00, // X density
    0x00, 0x00, // Y density
    0x00, // X thumbnail size
    0x00, // Y thumbnail size
];

const DHT_SEGMENT_SIZE: usize = 420;
const DHT_SEGMENT_HEAD: [u8; 5] = [0xff, 0xc4, 0x01, 0xa2, 0x00];
const DHT_SEGMENT_FRAG: [u8; 29] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x01, 0x00, 0x03, 0x01, 0x01,
    0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00,
];

/// FFmpeg's `append_dht_segment`.
fn append_dht_segment(out: &mut Vec<u8>) {
    out.extend_from_slice(&DHT_SEGMENT_HEAD);
    out.extend_from_slice(&BITS_DC_LUMINANCE[1..17]);
    out.extend_from_slice(&DHT_SEGMENT_FRAG);
    out.extend_from_slice(&VAL_DC);
    out.push(0x10);
    out.extend_from_slice(&BITS_AC_LUMINANCE[1..17]);
    out.extend_from_slice(&VAL_AC_LUMINANCE);
    out.push(0x11);
    out.extend_from_slice(&BITS_AC_CHROMINANCE[1..17]);
    out.extend_from_slice(&VAL_AC_CHROMINANCE);
}

/// FFmpeg's `mjpeg2jpeg_filter`: the packet as JPEG/JFIF with the standard Huffman tables.
pub fn mjpeg2jpeg(input: &[u8]) -> Result<Vec<u8>, &'static str> {
    if input.len() < 12 {
        return Err("input is truncated");
    }
    if input[0] != 0xff || input[1] != 0xd8 {
        return Err("input is not MJPEG");
    }
    let input_skip = if input[2] == 0xff && input[3] == APP0 {
        ((input[4] as usize) << 8) + input[5] as usize + 4
    } else {
        2
    };
    if input.len() < input_skip {
        return Err("input is truncated");
    }
    let mut out = Vec::with_capacity(input.len() - input_skip + JPEG_HEADER.len() + DHT_SEGMENT_SIZE);
    out.extend_from_slice(&JPEG_HEADER);
    append_dht_segment(&mut out);
    out.extend_from_slice(&input[input_skip..]);
    Ok(out)
}

/// Ours: whether the frame already has a Huffman table (a DHT before its scan).
pub fn has_dht(b: &[u8]) -> bool {
    let mut i = 2;
    while i + 3 < b.len() && b[i] == 0xff {
        match b[i + 1] {
            0xc4 => return true,
            0xda => return false,
            _ => i += 2 + (((b[i + 2] as usize) << 8) | b[i + 3] as usize),
        }
    }
    false
}

/// Ours: where the JPEG ends, after its first EOI that is not inside a segment or entropy-coded data
/// (`FF00` stuffing and restart markers are skipped). nokhwa's V4L2 backend gives the whole buffer and
/// not `bytesused`, so what follows the JPEG in it is not the frame's.
pub fn jpeg_end(b: &[u8]) -> Option<usize> {
    let mut i = 2;
    while i + 1 < b.len() {
        if b[i] != 0xff {
            i += 1;
            continue;
        }
        match b[i + 1] {
            0xff => i += 1,
            0x00 | 0x01 | 0xd0..=0xd7 => i += 2,
            0xd9 => return Some(i + 2),
            _ => {
                if i + 3 >= b.len() {
                    return None;
                }
                i += 2 + (((b[i + 2] as usize) << 8) | b[i + 3] as usize);
            }
        }
    }
    None
}

/// What a camera's frame becomes: cut at its end, and with the Huffman tables if it has none.
pub fn frame_to_jpeg(buffer: &[u8]) -> Result<Vec<u8>, &'static str> {
    if buffer.len() < 12 || buffer[0] != 0xff || buffer[1] != 0xd8 {
        return Err("not a JPEG");
    }
    let end = jpeg_end(buffer).ok_or("no end of image in the frame")?;
    let jpeg = &buffer[..end];
    if has_dht(jpeg) {
        Ok(jpeg.to_vec())
    } else {
        mjpeg2jpeg(jpeg)
    }
}

/// The camera on its own thread, until the agent ends: read it all the time, send the latest frame every
/// [`EVERY`]. A camera that cannot be opened is said, and the thread ends (no retries of ours).
pub fn spawn(hub: String, token: Arc<Mutex<String>>, index: u32) {
    std::thread::spawn(move || {
        if let Err(error) = capture(&hub, &token, index) {
            println!("camera {index}: {error}");
        }
    });
}

fn capture(hub: &str, token: &Mutex<String>, index: u32) -> Result<(), Box<dyn std::error::Error>> {
    // Ours: 640x480 MJPEG at 30 fps, the closest the camera has.
    let wanted = RequestedFormat::new::<RgbFormat>(RequestedFormatType::Closest(CameraFormat::new(
        Resolution::new(640, 480),
        FrameFormat::MJPEG,
        30,
    )));
    let mut camera = Camera::new(CameraIndex::Index(index), wanted)?;
    camera.open_stream()?;
    let format = camera.camera_format();
    if format.format() != FrameFormat::MJPEG {
        return Err(format!("the camera gives {:?}, step 1 takes MJPEG only", format.format()).into());
    }
    println!(
        "camera {index}: {} at {format}, a frame every {} s",
        camera.info().human_name(),
        EVERY.as_secs()
    );
    let url = format!("{hub}{}", protocol::CAMERA_FRAME.replace("{index}", &index.to_string()));
    let mut last: Option<Instant> = None;
    loop {
        let raw = camera.frame_raw()?;
        if last.is_some_and(|at| at.elapsed() < EVERY) {
            continue;
        }
        let jpeg = match frame_to_jpeg(&raw) {
            Ok(jpeg) => jpeg,
            Err(why) => {
                println!("camera {index}: a frame dropped: {why}");
                continue;
            }
        };
        last = Some(Instant::now());
        let bearer = format!("Bearer {}", token.lock().unwrap());
        if let Err(error) = ureq::post(&url)
            .set("authorization", &bearer)
            .set("content-type", "image/jpeg")
            .send_bytes(&jpeg)
        {
            println!("camera {index}: the frame did not reach the hub: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A JPEG as a UVC camera gives it: what a normal encoder wrote, its Huffman tables taken out.
    /// `with_app0` keeps the JFIF header.
    fn uvc_like(with_app0: bool) -> Vec<u8> {
        let mut image = Vec::new();
        let pixels: Vec<u8> = (0..64 * 48 * 3).map(|i| (i * 7 % 251) as u8).collect();
        jpeg_encoder::Encoder::new(&mut image, 90)
            .encode(&pixels, 64, 48, jpeg_encoder::ColorType::Rgb)
            .unwrap();
        let mut out = vec![0xff, 0xd8];
        let mut i = 2;
        while image[i] == 0xff {
            let marker = image[i + 1];
            let len = (((image[i + 2] as usize) << 8) | image[i + 3] as usize) + 2;
            if marker != 0xc4 && (marker != APP0 || with_app0) {
                out.extend_from_slice(&image[i..i + len]);
            }
            if marker == 0xda {
                out.extend_from_slice(&image[i + len..]);
                break;
            }
            i += len;
        }
        out
    }

    #[test]
    fn the_sample_has_no_tables_and_the_port_adds_them() {
        let sample = uvc_like(false);
        assert!(!has_dht(&sample));
        let fixed = mjpeg2jpeg(&sample).unwrap();
        assert!(has_dht(&fixed));
        assert_eq!(fixed.len(), sample.len() - 2 + JPEG_HEADER.len() + DHT_SEGMENT_SIZE);
        assert_eq!(jpeg_end(&fixed), Some(fixed.len()));
    }

    /// The port against FFmpeg's own filter, byte for byte (skipped where ffmpeg is not installed).
    #[test]
    fn identical_to_ffmpegs_mjpeg2jpeg() {
        let dir = std::env::temp_dir().join(format!("mc-mjpeg2jpeg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, with_app0) in [("plain", false), ("app0", true)] {
            let sample = uvc_like(with_app0);
            let (input, output) = (dir.join(format!("{name}.jpg")), dir.join(format!("{name}.out.jpg")));
            std::fs::write(&input, &sample).unwrap();
            let run = std::process::Command::new("ffmpeg")
                .args(["-v", "error", "-y", "-f", "image2", "-c:v", "mjpeg", "-i"])
                .arg(&input)
                .args(["-c", "copy", "-bsf:v", "mjpeg2jpeg", "-f", "image2"])
                .arg(&output)
                .status();
            let Ok(status) = run else {
                eprintln!("ffmpeg not installed: the byte compare is skipped");
                return;
            };
            assert!(status.success());
            assert_eq!(mjpeg2jpeg(&sample).unwrap(), std::fs::read(&output).unwrap(), "{name}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_buffer_is_cut_at_the_end_of_the_jpeg_and_a_second_dht_is_not_added() {
        let fixed = mjpeg2jpeg(&uvc_like(true)).unwrap();
        let mut buffer = fixed.clone();
        buffer.extend_from_slice(&[0xff, 0xd9, 0, 0, 0xff, 0xd8, 7, 7]);
        assert_eq!(frame_to_jpeg(&buffer).unwrap(), fixed, "a frame that has tables stays as it is");
        let mut raw = uvc_like(false);
        let plain = raw.clone();
        raw.extend_from_slice(&[0xaa; 1000]);
        assert_eq!(frame_to_jpeg(&raw).unwrap(), mjpeg2jpeg(&plain).unwrap());
        assert!(frame_to_jpeg(&[0u8; 100]).is_err());
        assert!(frame_to_jpeg(&plain[..60]).is_err(), "no end: dropped");
    }
}
