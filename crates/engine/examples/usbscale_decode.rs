//! The scale port's decoder, for `scripts/usbscale-audit-check.py`: each argument is one report as hex
//! bytes joined by commas (`03,04,02,00,34,12`); one line each, tab separated: `ok`, the status text, the
//! unit name, the weight's bits, the exponent, the raw weight; or `reject` and why.

use maker_connect_engine::ports::usbscale::usbscale::{Rejected, print_scale_data};

fn main() {
    for arg in std::env::args().skip(1) {
        let dat: Vec<u8> = arg
            .split(',')
            .map(|b| u8::from_str_radix(b, 16).expect("hex bytes"))
            .collect();
        match print_scale_data(&dat) {
            Ok(r) => println!(
                "ok\t{}\t{}\t{:016x}\t{}\t{}",
                r.status_text,
                r.unit_name,
                r.weight.to_bits(),
                r.exponent,
                r.raw
            ),
            Err(Rejected::NotAReport(id)) => println!("reject\treport ID {id}"),
            Err(Rejected::Short(n)) => println!("reject\tshort: {n} bytes"),
        }
    }
}
