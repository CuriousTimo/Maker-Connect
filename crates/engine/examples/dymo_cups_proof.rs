//! The proof's side of the DYMO LabelWriter 450 port (`docs/audits/dymo-labelwriter-450.md`,
//! `scripts/dymo-labelwriter-450-check.py proof`): runs the port's filter on a PNG with a recording
//! backend and writes what it writes, so the script can run DYMO's own `raster2dymolw` on the same page
//! and compare. Not part of the agent.
//!
//! `dymo_cups_proof PNG LENGTH_MM|-|ps:PAGESIZE OPTIONS DEVICE_URI STATUS_HEX|- OUT_DIR`

use maker_connect_engine::ports::dymo_cups::cups::ppd::PpdFile;
use maker_connect_engine::ports::dymo_cups::cups::usb_backend::BackChannel;
use maker_connect_engine::ports::dymo_cups::cups_print_environment::{FilterIo, SharedIo};
use maker_connect_engine::ports::dymo_cups::{page, raster2dymolw, stderr};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

struct RecordingIo {
    stdout: Vec<u8>,
    back: Arc<BackChannel>,
    device_uri: Option<String>,
}

impl FilterIo for RecordingIo {
    fn write_stdout(&mut self, data: &[u8]) -> Result<(), i32> {
        self.stdout.extend_from_slice(data);
        Ok(())
    }
    fn back_channel_read(&mut self, buffer: &mut [u8], timeout: f64) -> isize {
        self.back.read(buffer, timeout)
    }
    fn getenv(&self, name: &str) -> Option<String> {
        if name == "DEVICE_URI" { self.device_uri.clone() } else { None }
    }
    fn ppd_open_file(&self) -> Option<PpdFile> {
        Some(PpdFile::lw450())
    }
    fn cancelled(&mut self) -> bool {
        false
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (png, length, options, uri, status, out) = (&a[1], &a[2], &a[3], &a[4], &a[5], &a[6]);
    // LENGTH_MM, `-` for none, or `ps:<PageSize choice>` for a job that names its page (W2b)
    let (page_size, length_mm) = match length.strip_prefix("ps:") {
        Some(name) => (Some(name), None),
        None if length == "-" => (None, None),
        None => (None, Some(length.parse::<u32>().unwrap())),
    };
    let device_uri = if uri == "-" { None } else { Some(uri.clone()) };

    let picture = page::decode_png(std::path::Path::new(png)).unwrap();
    let open = || Some(PpdFile::lw450());
    let page = match page::make_page(&picture, page_size, length_mm, options, &open) {
        Ok(p) => p,
        Err(why) => {
            std::fs::write(format!("{out}/ours.page-error"), why).unwrap();
            return;
        }
    };
    let h = &page.header;
    std::fs::write(
        format!("{out}/ours.header"),
        format!(
            "name={} width={} height={} bpl={} bpp={} bpc={} space={} order={} numcolors={} mediatype={} pagesize={},{} hwres={},{} int0={} margins={},{} bbox={},{},{},{}\n",
            h.cups_page_size_name, h.cups_width, h.cups_height, h.cups_bytes_per_line, h.cups_bits_per_pixel, h.cups_bits_per_color, h.cups_color_space, h.cups_color_order,
            h.cups_num_colors, h.cups_media_type, h.page_size[0], h.page_size[1], h.hw_resolution[0], h.hw_resolution[1], h.cups_integer[0], h.margins[0], h.margins[1],
            h.imaging_bounding_box[0], h.imaging_bounding_box[1], h.imaging_bounding_box[2], h.imaging_bounding_box[3]
        ),
    )
    .unwrap();
    std::fs::write(format!("{out}/page.pixels"), &page.pixels).unwrap();
    std::fs::write(format!("{out}/page.size"), format!("{}\n", h.cups_page_size_name)).unwrap();

    let back = BackChannel::new();
    if status != "-" {
        let bytes: Vec<u8> = (0..status.len() / 2).map(|i| u8::from_str_radix(&status[2 * i..2 * i + 2], 16).unwrap()).collect();
        back.write(&bytes);
    }
    let io = Rc::new(RefCell::new(RecordingIo { stdout: Vec::new(), back, device_uri }));
    let shared: SharedIo = io.clone();
    stderr::start(None);
    let argv: Vec<String> = ["raster2dymolw", "1", "maker-connect", "proof", "1", options].iter().map(|s| s.to_string()).collect();
    let mut filter = None;
    let raster = maker_connect_engine::ports::dymo_cups::cups::raster::CupsRaster::open(vec![page]);
    let status = raster2dymolw::main(shared, &argv, raster, &mut filter);
    std::fs::write(format!("{out}/ours.bin"), &io.borrow().stdout).unwrap();
    std::fs::write(format!("{out}/ours.err"), stderr::take().concat()).unwrap();
    std::fs::write(format!("{out}/ours.status"), format!("{status}\n")).unwrap();
}
