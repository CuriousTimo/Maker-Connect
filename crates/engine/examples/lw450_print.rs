//! Hardware test of the DYMO LabelWriter 450 port (`docs/archive/plans/agent-engine.md` D11): one job through the real
//! port ([`LabelWriter450`] over nusb), as the agent runs it, without the hub. Ours, test tooling only:
//! what goes to the printer is the port's (page, `raster2dymolw`, the `usb` backend).
//!
//! `lw450_print LENGTH_MM WIDTH_DOTS HEIGHT_DOTS [PNG PAGESIZE]`
//!
//! With PNG and PAGESIZE: that picture is printed instead, as a job naming that `PageSize` (as the hub's
//! job does), e.g. a ruler to measure where the page falls on a roll.
//!
//! Draws a test picture of WIDTH × HEIGHT dots (a frame, a diagonal, a black block), prints it as a label
//! LENGTH_MM long, and prints every event of the port. With the wrong size the port says which size the
//! label needs ("the label (...) needs W x H dots"): run it again with those.

use maker_connect_engine::ports::dymo_cups::LabelWriter450;
use maker_connect_engine::{Order, Port};
use maker_connect_protocol::JobOrder;
use std::sync::mpsc;

fn test_picture(w: usize, h: usize) -> Vec<u8> {
    let mut rgb = vec![255u8; w * h * 3];
    for y in 0..h {
        for x in 0..w {
            let frame = x < 4 || y < 4 || x + 4 >= w || y + 4 >= h;
            let diagonal = h > 1 && w > 1 && (x * (h - 1)).abs_diff(y * (w - 1)) < 3 * w.max(h);
            let block = x >= w / 4 && x < w / 4 + w / 8 && y >= h / 4 && y < h / 4 + h / 8;
            if frame || diagonal || block {
                let i = (y * w + x) * 3;
                rgb[i..i + 3].copy_from_slice(&[0, 0, 0]);
            }
        }
    }
    rgb
}

fn write_png(path: &std::path::Path, w: usize, h: usize, rgb: &[u8]) {
    let file = std::fs::File::create(path).expect("create the PNG");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .expect("PNG header")
        .write_image_data(rgb)
        .expect("PNG data");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // `--size PAGESIZE`: the page in dots as the port's geometry gives it; nothing is sent
    if args.len() == 2 && args[0] == "--size" {
        match maker_connect_engine::ports::geometry::lw450::page_sizes()
            .into_iter()
            .find(|g| g.name == args[1])
        {
            Some(g) => println!(
                "{}: {} x {} dots, length {} mm",
                g.name, g.dots[0], g.dots[1], g.length_mm
            ),
            None => eprintln!("no PageSize {} in the PPD", args[1]),
        }
        return;
    }
    if args.len() != 3 && args.len() != 5 {
        eprintln!("usage: lw450_print LENGTH_MM WIDTH_DOTS HEIGHT_DOTS [PNG PAGESIZE]");
        std::process::exit(2);
    }
    let length_mm: u32 = args[0].parse().expect("LENGTH_MM");
    let (w, h): (usize, usize) = (
        args[1].parse().expect("WIDTH_DOTS"),
        args[2].parse().expect("HEIGHT_DOTS"),
    );

    let (file, page_size) = if args.len() == 5 {
        (std::path::PathBuf::from(&args[3]), Some(args[4].clone()))
    } else {
        let file = std::env::temp_dir().join("lw450_print.png");
        write_png(&file, w, h, &test_picture(w, h));
        (file, None)
    };
    let size = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);

    let (order_tx, order_rx) = mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel();
    let port = std::thread::spawn(move || Box::new(LabelWriter450::new()).run(order_rx, event_tx));

    let order = JobOrder {
        id: 1,
        name: "lw450_print".to_owned(),
        download: String::new(),
        size,
        from_line: None,
        tape_mm: None,
        label_length_mm: Some(length_mm),
        page_size,
    };
    order_tx
        .send(Order::Job { order, file })
        .expect("send the job");
    for event in event_rx.iter() {
        println!("{event:?}");
        if matches!(event, maker_connect_engine::Event::JobEnd(_)) {
            break;
        }
    }
    let _ = order_tx.send(Order::Close);
    let _ = port.join();
}
