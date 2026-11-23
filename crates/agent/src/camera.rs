//! A USB webcam on the agent as a camera driver of the Connect SDK port (#94): nokhwa 0.10.11 capture
//! from 89a8c4e, as a `DriverClass` + `DriverImpl` after PrusaLink's V4L2 driver (`v4l2_driver.py` at
//! `171148c`).
//!
//! Capture: nokhwa 0.10.11 (owner's exception, 2026-11-16: "nokhwa overal"). The JPEG a UVC camera
//! gives has no Huffman tables; [`mjpeg2jpeg`] adds them, ported from FFmpeg's bitstream filter
//! `libavcodec/bsf/mjpeg2jpeg.c` (lines 36-142) with the tables of `libavcodec/jpegtabs.h` ([`tables`]),
//! FFmpeg master `f1d4dd2bd0ace9263b590b1f3a500190b8687bef` (2026-11-17), Copyright (c) 2010 Adrian Daerr
//! and Nicolas George, LGPL-2.1-or-later. Ours, and only ours (the brief names them): the requested
//! format, [`jpeg_end`], [`has_dht`], reading the camera all the time, waiting in `connect` for the first
//! frame ([`Shared::first_frame`]).
//!
//! Ours (#93): the plaintext id of `_scan`, [`plaintext_id`]: name plus Linux `/dev/videoN`, macOS
//! `uniqueID`, Windows symbolic link (nokhwa reads no USB serial). No resolution capability: v1 took
//! 640x480 MJPEG at 30 fps, the closest the camera has, and had no setting.

mod tables;

use maker_connect_gcode_metadata::py::PyErr;
use maker_connect_sdk::camera_driver::{CameraDriver, DriverClass, DriverImpl, DriverState};
use maker_connect_sdk::consts::{CAMERA_WAIT_TIMEOUT, CameraConfig, CameraConfigs, CapabilityType};
use nokhwa::{
    Camera,
    pixel_format::RgbFormat,
    utils::{
        ApiBackend, CameraFormat, CameraIndex, CameraInfo, FrameFormat, RequestedFormat,
        RequestedFormatType, Resolution,
    },
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tables::*;

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
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x01, 0x00, 0x03, 0x01,
    0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00,
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
    let mut out =
        Vec::with_capacity(input.len() - input_skip + JPEG_HEADER.len() + DHT_SEGMENT_SIZE);
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

// --- The driver (`V4L2Driver`)

/// `V4L2Driver`; name, class name and the key are ours.
pub static NOKHWA: DriverClass = DriverClass {
    name: "nokhwa",
    class_name: "NokhwaDriver",
    requires_settings: &[("index", "The nokhwa index of the camera, like '0'")],
    scan,
    is_config_valid: CameraDriver::base_is_config_valid,
    init,
};

/// Ours (#93): `" ".join((name, X))` as `v4l2_driver.py:508`, X from nokhwa's `CameraInfo`: on Linux
/// `/dev/video{index}` (its `misc` is empty), on macOS and Windows its `misc` (`uniqueID`, symbolic link).
pub fn plaintext_id(info: &CameraInfo) -> Result<String, PyErr> {
    if cfg!(target_os = "linux") {
        let index = info.index().as_index().map_err(nokhwa_error)?;
        Ok(format!("{} /dev/video{index}", info.human_name()))
    } else {
        Ok(format!("{} {}", info.human_name(), info.misc()))
    }
}

/// `_scan`'s result for what nokhwa listed: `{plaintext id: {"index", "name"}}` (`path`, `name` in V4L2).
fn configs_from(infos: &[CameraInfo]) -> Result<CameraConfigs, PyErr> {
    let mut available = CameraConfigs::new();
    for info in infos {
        let index = info.index().as_index().map_err(nokhwa_error)?;
        let config: CameraConfig = vec![
            ("index".to_owned(), index.to_string()),
            ("name".to_owned(), info.human_name()),
        ];
        let id = plaintext_id(info)?;
        match available.iter_mut().find(|(key, _)| *key == id) {
            Some(entry) => entry.1 = config,
            None => available.push((id, config)),
        }
    }
    Ok(available)
}

/// `_scan`: returns the available USB cameras.
fn scan() -> Result<CameraConfigs, PyErr> {
    let mut infos = nokhwa::query(ApiBackend::Auto).map_err(nokhwa_error)?;
    if cfg!(target_os = "linux") {
        let mut kept = Vec::with_capacity(infos.len());
        for info in infos {
            let index = info.index().as_index().map_err(nokhwa_error)?;
            if v4l2::is_a_capture_device(&format!("/dev/video{index}"))? {
                kept.push(info);
            }
        }
        infos = kept;
    }
    configs_from(&infos)
}

/// What `_scan` (`v4l2_driver.py:481-514`) skips before it names a camera: a device without
/// `V4L2_CAP_VIDEO_CAPTURE` in its `capabilities` (`iter_video_capture_devices`, 213-219: a decoder such
/// as `cedrus`), and one with no `VIDEO_CAPTURE` formats (`if not device.info.formats`, `read_info`
/// 123-147: a UVC camera's metadata node). Without it, on .67 (Logitech C270, 2026-11-23), the metadata
/// node and `cedrus` were opened beside the video node, failed, and the video node gave no frame.
#[cfg(target_os = "linux")]
mod v4l2 {
    use maker_connect_gcode_metadata::py::PyErr;
    use std::os::fd::AsRawFd;

    /// `struct v4l2_capability` (`linux/videodev2.h`).
    #[repr(C)]
    struct Capability {
        driver: [u8; 16],
        card: [u8; 32],
        bus_info: [u8; 32],
        version: u32,
        capabilities: u32,
        device_caps: u32,
        reserved: [u32; 3],
    }

    /// `struct v4l2_fmtdesc` (`linux/videodev2.h`).
    #[repr(C)]
    struct FmtDesc {
        index: u32,
        kind: u32,
        flags: u32,
        description: [u8; 32],
        pixelformat: u32,
        mbus_code: u32,
        reserved: [u32; 3],
    }

    const V4L2_CAP_VIDEO_CAPTURE: u32 = 0x0000_0001;
    const V4L2_BUF_TYPE_VIDEO_CAPTURE: u32 = 1;
    /// `_IOR('V', 0, struct v4l2_capability)`.
    const VIDIOC_QUERYCAP: libc::c_ulong = 0x8068_5600;
    /// `_IOWR('V', 2, struct v4l2_fmtdesc)`.
    const VIDIOC_ENUM_FMT: libc::c_ulong = 0xc040_5602;

    fn os_error(path: &str, error: std::io::Error) -> PyErr {
        PyErr::Other("OSError", format!("{path}: {error}"))
    }

    pub(super) fn is_a_capture_device(path: &str) -> Result<bool, PyErr> {
        let file = std::fs::File::open(path).map_err(|e| os_error(path, e))?;
        let fd = file.as_raw_fd();
        // SAFETY: an all-zero `v4l2_capability` is valid; the kernel fills it.
        let mut caps: Capability = unsafe { std::mem::zeroed() };
        // SAFETY: `fd` is open, `caps` has the size `VIDIOC_QUERYCAP` encodes.
        if unsafe { libc::ioctl(fd, VIDIOC_QUERYCAP as _, &mut caps) } < 0 {
            return Err(os_error(path, std::io::Error::last_os_error()));
        }
        if caps.capabilities & V4L2_CAP_VIDEO_CAPTURE == 0 {
            return Ok(false);
        }
        // SAFETY: as above, for `v4l2_fmtdesc`.
        let mut fmt: FmtDesc = unsafe { std::mem::zeroed() };
        fmt.kind = V4L2_BUF_TYPE_VIDEO_CAPTURE;
        // SAFETY: `fd` is open, `fmt` has the size `VIDIOC_ENUM_FMT` encodes.
        if unsafe { libc::ioctl(fd, VIDIOC_ENUM_FMT as _, &mut fmt) } == 0 {
            return Ok(true);
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EINVAL) {
            return Ok(false);
        }
        Err(os_error(path, error))
    }
}

#[cfg(not(target_os = "linux"))]
mod v4l2 {
    use maker_connect_gcode_metadata::py::PyErr;

    pub(super) fn is_a_capture_device(_path: &str) -> Result<bool, PyErr> {
        Ok(true)
    }
}

fn init(_state: &mut DriverState) -> Box<dyn DriverImpl> {
    Box::new(NokhwaDriver {
        capture: Mutex::new(None),
    })
}

fn nokhwa_error(error: nokhwa::NokhwaError) -> PyErr {
    PyErr::Other("NokhwaError", error.to_string())
}

/// The newest frame of a camera, and how many there were (`seq`).
#[derive(Default)]
struct Held {
    seq: u64,
    raw: Vec<u8>,
    error: Option<String>,
}

struct Shared {
    held: Mutex<Held>,
    arrived: Condvar,
    stop: AtomicBool,
}

impl Shared {
    /// A frame that arrived after this call, or the camera's error, or `TimeoutError` after
    /// `timeout` (V4L2's `next_frame`).
    fn next_frame(&self, timeout: Duration) -> Result<Vec<u8>, PyErr> {
        let deadline = Instant::now() + timeout;
        let mut held = self.held.lock().unwrap();
        let seen = held.seq;
        loop {
            if let Some(error) = &held.error {
                return Err(PyErr::Other("OSError", error.clone()));
            }
            if held.seq > seen {
                return Ok(held.raw.clone());
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(PyErr::Other(
                    "TimeoutError",
                    "Getting the next frame timed out".into(),
                ));
            }
            held = self.arrived.wait_timeout(held, left).unwrap().0;
        }
    }
}

impl Shared {
    /// Ours (owner, 2026-11-23, "1, wachten op het eerste frame"): the first frame of a stream, or the
    /// camera's error, or `TimeoutError` after `timeout`. A Logitech C270 on .67 gave its first frame
    /// 1.8 to 3.5 s after the stream started, and the SDK's first photo right after `connect`
    /// (`camera_configurator.py:350-352`) waits `CAMERA_WAIT_TIMEOUT` (3 s).
    fn first_frame(&self, timeout: Duration) -> Result<(), PyErr> {
        let deadline = Instant::now() + timeout;
        let mut held = self.held.lock().unwrap();
        loop {
            if let Some(error) = &held.error {
                return Err(PyErr::Other("OSError", error.clone()));
            }
            if held.seq > 0 {
                return Ok(());
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(PyErr::Other(
                    "TimeoutError",
                    "The camera gave no first frame".into(),
                ));
            }
            held = self.arrived.wait_timeout(held, left).unwrap().0;
        }
    }
}

/// Ours: how long `connect` waits for a camera's first frame (see [`Shared::first_frame`]).
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(10);

/// One nokhwa capture of one camera: a thread reading it all the time, holding the newest frame.
struct Capture {
    shared: Arc<Shared>,
    thread: JoinHandle<Camera>,
}

impl Capture {
    fn start(mut camera: Camera) -> Result<Capture, PyErr> {
        camera.open_stream().map_err(nokhwa_error)?;
        let shared = Arc::new(Shared {
            held: Mutex::new(Held::default()),
            arrived: Condvar::new(),
            stop: AtomicBool::new(false),
        });
        let reader = shared.clone();
        let thread = std::thread::spawn(move || {
            while !reader.stop.load(Ordering::Relaxed) {
                let result = camera.frame_raw().map(|raw| raw.into_owned());
                let mut held = reader.held.lock().unwrap();
                match result {
                    Ok(raw) => {
                        held.seq += 1;
                        held.raw = raw;
                    }
                    Err(error) => held.error = Some(error.to_string()),
                }
                let failed = held.error.is_some();
                drop(held);
                reader.arrived.notify_all();
                if failed {
                    break;
                }
            }
            camera
        });
        Ok(Capture { shared, thread })
    }

    fn stop(self) -> Result<(), PyErr> {
        self.shared.stop.store(true, Ordering::Relaxed);
        let mut camera = self.thread.join().map_err(|_| {
            PyErr::Other(
                "RuntimeError",
                "the camera's reading thread panicked".into(),
            )
        })?;
        camera.stop_stream().map_err(nokhwa_error)
    }
}

struct NokhwaDriver {
    capture: Mutex<Option<Capture>>,
}

impl DriverImpl for NokhwaDriver {
    /// `_connect`: opens the camera.
    fn connect(&self, driver: &CameraDriver) -> Result<(), PyErr> {
        let index: u32 = driver
            .config_value("index")
            .ok_or_else(|| PyErr::KeyError("'index'".into()))?
            .parse()
            .map_err(|_| PyErr::ValueError("the camera's index is not a number".into()))?;
        driver.state().capabilities = vec![CapabilityType::TriggerScheme, CapabilityType::Imaging];
        // Ours (from v1): 640x480 MJPEG at 30 fps, the closest the camera has.
        let wanted = RequestedFormat::new::<RgbFormat>(RequestedFormatType::Closest(
            CameraFormat::new(Resolution::new(640, 480), FrameFormat::MJPEG, 30),
        ));
        let camera = Camera::new(CameraIndex::Index(index), wanted).map_err(nokhwa_error)?;
        if camera.camera_format().format() != FrameFormat::MJPEG {
            return Err(PyErr::Other(
                "NotSupported",
                format!(
                    "the camera gives {:?}, only MJPEG is taken",
                    camera.camera_format().format()
                ),
            ));
        }
        let capture = Capture::start(camera)?;
        // ours: connected once the camera gives frames (Shared::first_frame)
        if let Err(error) = capture.shared.first_frame(FIRST_FRAME_TIMEOUT) {
            if let Err(stop) = capture.stop() {
                log::error!("Camera {} could not be closed\n{stop}", driver.camera_id());
            }
            return Err(error);
        }
        *self.capture.lock().unwrap() = Some(capture);
        Ok(())
    }

    /// `_disconnect`: closes the camera; errors are logged, not raised.
    fn disconnect(&self, driver: &CameraDriver) -> Result<(), PyErr> {
        let Some(capture) = self.capture.lock().unwrap().take() else {
            return Ok(());
        };
        if let Err(error) = capture.stop() {
            log::error!("Camera {} could not be closed\n{error}", driver.camera_id());
        }
        Ok(())
    }

    /// `take_a_photo`: blocks for the next frame, returns its JPEG.
    fn take_a_photo(&self, _driver: &CameraDriver) -> Result<Vec<u8>, PyErr> {
        let shared = match self.capture.lock().unwrap().as_ref() {
            Some(capture) => capture.shared.clone(),
            None => {
                return Err(PyErr::Other(
                    "RuntimeError",
                    "The camera is not connected".into(),
                ));
            }
        };
        let raw = shared.next_frame(Duration::from_secs(CAMERA_WAIT_TIMEOUT))?;
        frame_to_jpeg(&raw).map_err(|why| PyErr::ValueError(why.to_owned()))
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
        assert_eq!(
            fixed.len(),
            sample.len() - 2 + JPEG_HEADER.len() + DHT_SEGMENT_SIZE
        );
        assert_eq!(jpeg_end(&fixed), Some(fixed.len()));
    }

    /// The port against FFmpeg's own filter, byte for byte (skipped where ffmpeg is not installed).
    #[test]
    fn identical_to_ffmpegs_mjpeg2jpeg() {
        let dir = std::env::temp_dir().join(format!("mc-mjpeg2jpeg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, with_app0) in [("plain", false), ("app0", true)] {
            let sample = uvc_like(with_app0);
            let (input, output) = (
                dir.join(format!("{name}.jpg")),
                dir.join(format!("{name}.out.jpg")),
            );
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
            assert_eq!(
                mjpeg2jpeg(&sample).unwrap(),
                std::fs::read(&output).unwrap(),
                "{name}"
            );
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_buffer_is_cut_at_the_end_of_the_jpeg_and_a_second_dht_is_not_added() {
        let fixed = mjpeg2jpeg(&uvc_like(true)).unwrap();
        let mut buffer = fixed.clone();
        buffer.extend_from_slice(&[0xff, 0xd9, 0, 0, 0xff, 0xd8, 7, 7]);
        assert_eq!(
            frame_to_jpeg(&buffer).unwrap(),
            fixed,
            "a frame that has tables stays as it is"
        );
        let mut raw = uvc_like(false);
        let plain = raw.clone();
        raw.extend_from_slice(&[0xaa; 1000]);
        assert_eq!(frame_to_jpeg(&raw).unwrap(), mjpeg2jpeg(&plain).unwrap());
        assert!(frame_to_jpeg(&[0u8; 100]).is_err());
        assert!(frame_to_jpeg(&plain[..60]).is_err(), "no end: dropped");
    }
    fn info(name: &str, misc: &str, index: u32) -> CameraInfo {
        CameraInfo::new(name, "description", misc, CameraIndex::Index(index))
    }

    #[test]
    fn scan_gives_index_and_name_under_the_plaintext_id() {
        let infos = [info("Cam A", "x", 0), info("Cam B", "y", 3)];
        let found = configs_from(&infos).unwrap();
        assert_eq!(found.len(), 2);
        let id = plaintext_id(&infos[1]).unwrap();
        let config = &found.iter().find(|(key, _)| *key == id).unwrap().1;
        assert_eq!(
            config,
            &vec![
                ("index".to_owned(), "3".to_owned()),
                ("name".to_owned(), "Cam B".to_owned())
            ]
        );
        // the SDK's own check of a scanned config and its hash
        assert!(CameraDriver::base_is_config_valid(&NOKHWA, &{
            let mut c = config.clone();
            c.push(("driver".to_owned(), "nokhwa".to_owned()));
            c
        }));
        assert!(CameraDriver::make_hash(&id).is_ok());
    }

    #[test]
    fn first_frame_waits_for_the_first_and_times_out() {
        let shared = Arc::new(Shared {
            held: Mutex::new(Held::default()),
            arrived: Condvar::new(),
            stop: AtomicBool::new(false),
        });
        assert!(matches!(
            shared.first_frame(Duration::from_millis(50)),
            Err(PyErr::Other("TimeoutError", _))
        ));
        let writer = shared.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            writer.held.lock().unwrap().seq = 1;
            writer.arrived.notify_all();
        });
        assert!(shared.first_frame(Duration::from_secs(5)).is_ok());
        t.join().unwrap();
    }

    #[test]
    fn next_frame_waits_for_a_frame_after_the_call_and_times_out() {
        let shared = Arc::new(Shared {
            held: Mutex::new(Held::default()),
            arrived: Condvar::new(),
            stop: AtomicBool::new(false),
        });
        shared.held.lock().unwrap().raw = vec![1];
        shared.held.lock().unwrap().seq = 1;
        // the held frame is older than the call: timeout
        let error = shared.next_frame(Duration::from_millis(50)).unwrap_err();
        assert_eq!(
            error.to_string(),
            "TimeoutError: Getting the next frame timed out"
        );
        let writer = shared.clone();
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            let mut held = writer.held.lock().unwrap();
            held.seq += 1;
            held.raw = vec![2];
            writer.arrived.notify_all();
        });
        assert_eq!(shared.next_frame(Duration::from_secs(5)).unwrap(), vec![2]);
        handle.join().unwrap();
        shared.held.lock().unwrap().error = Some("gone".into());
        assert!(shared.next_frame(Duration::from_millis(10)).is_err());
    }
}
