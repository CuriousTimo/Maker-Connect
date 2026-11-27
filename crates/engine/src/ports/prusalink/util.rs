//! PrusaLink's `util.py` (`reference/Prusa-Link`, `171148ce`), the parts of section A (#242,
//! `docs/archive/plans/mk3-port-plan.md`): `:105-250` and `:300-351`, function by function, and
//! `loop_until` (`:51-73`), which `updatable.py` needs.
//! `sanitize_api_key` (`:37-41`) is in [`super::config`]; `get_usb_printers` (`:252-283`, pyudev)
//! comes with the serial adapter (section D).
//!
//! The Python underneath: `str(Path(...))` is CPython 3.9's `pathlib` on POSIX, `os.makedirs` and
//! `os.path.exists` the SDK port's [`pyos`], `str.strip` [`strip`], the hash the SDK's
//! [`make_fingerprint`](maker_connect_sdk::util::make_fingerprint) (the same function), `cp437`
//! CPython's codec table.
//!
//! **Not covered:**
//! - Logging (`log.info` in `power_panic_delay`): kept as a comment, as in the other engine ports.
//! - `PPData(**json.load(...))` in `power_panic_delay`: pydantic converts a value of another type;
//!   here the file must hold the types PrusaLink writes there (`file_printer.py:385`,
//!   `json.dumps(data.dict())`).
//! - `slots_with_param` (no caller in PrusaLink) with a value of another type than the field's:
//!   pydantic stores it, here it is a `TypeError`.
//! - `pwd` and `chown` on Windows: there are none; an error names the call (the MK3 port is Linux
//!   first, owner 2026-11-27).

use std::time::Duration;

use maker_connect_gcode_metadata::py::{PyErr, strip};
use maker_connect_sdk::pyos;
use serde_json::{Map, Value};

use super::config::Config;
use super::consts::{MMU_SLOTS, PP_MOVES_DELAY, SD_STORAGE_NAME};
use super::model_classes::{IndividualSlot, PPData, Telemetry};
use super::unidecode::unidecode;
use crate::ports::python::py_threading::Event;

/// `time.time()`
pub(crate) fn time() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// `loop_until(loop_evt, run_every_sec, to_run, *arg_getters, **kwarg_getters)` (`:51-73`): Call a
/// function every X seconds, quit instantly. The getters are `to_run`'s own: it calls them before
/// it runs, as the loop does. `prctl_name()` (`:44-48`, the system thread name) is not covered:
/// nothing the printer sees.
pub fn loop_until(loop_evt: &Event, run_every_sec: impl Fn() -> f64, mut to_run: impl FnMut()) {
    // prctl_name()
    while !loop_evt.is_set() {
        // if it's time to run the func

        let last_called = time();

        to_run();

        let run_again_in = f64::max(0.0, (last_called + run_every_sec()) - time());
        loop_evt.wait(Some(run_again_in));
    }
}

/// `get_clean_path` (`:105-110`): Uses pathlib to load a path string, then gets a string for it,
/// ensuring consistent formatting. `str(PosixPath(path))`: `_PosixFlavour.splitroot` (one root
/// `/`, or `//` when exactly two slashes lead), the parts without empty ones and `.`, `.` for none.
pub fn get_clean_path(path: &str) -> String {
    let (root, rest) = if let Some(rest) = path.strip_prefix("//") {
        if rest.starts_with('/') {
            ("/", path.trim_start_matches('/'))
        } else {
            ("//", rest)
        }
    } else if let Some(rest) = path.strip_prefix('/') {
        ("/", rest.trim_start_matches('/'))
    } else {
        ("", path)
    };
    let parts: Vec<&str> = rest
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    if root.is_empty() && parts.is_empty() {
        return ".".to_owned();
    }
    format!("{root}{}", parts.join("/"))
}

/// `ensure_directory` (`:113-120`): If missing, makes directories, along the supplied path
pub fn ensure_directory(directory: &str, chown_username: Option<&str>) -> Result<(), PyErr> {
    if !pyos::exists(directory.as_bytes()) {
        pyos::makedirs(directory.as_bytes(), false).map_err(os_error)?;
        let Some(chown_username) = chown_username else {
            return Ok(());
        };
        let (uid, gid) = getpwnam(chown_username)?;
        chown(directory, uid, gid)?;
    }
    Ok(())
}

/// `persist_file` (`:123-130`): Tells the system to write and sync the file. Unused
pub fn persist_file(file: &mut std::fs::File) -> std::io::Result<()> {
    use std::io::Write;
    file.flush()?;
    file.sync_all()
}

/// `get_gcode` (`:133-143`): Removes comments after the supplied gcode command. Makes gcode
/// encodeable in ascii. Put any other sanitization here
pub fn get_gcode(line: &str) -> String {
    let unicode_gcode = strip(line.split(';').next().unwrap_or(""));
    unidecode(unicode_gcode)
}

/// `file_is_on_sd` (`:146-150`): Checks if the file path starts wit the sd cards' storage name
pub fn file_is_on_sd(path_parts: &[&str]) -> bool {
    if path_parts.len() < 2 {
        return false;
    }
    path_parts[1] == SD_STORAGE_NAME
}

/// `make_fingerprint` (`:153-159`): Uses sha256 to hask the serial number for use as a
/// fingerprint. Ideally, we would have the printer's UUID too, but MK3 printers don't have it
pub fn make_fingerprint(sn: &str) -> String {
    maker_connect_sdk::util::make_fingerprint(sn)
}

/// `fat_datetime_to_tuple` (`:162-184`): Converts datetime from FAT file header to touple of
/// (years, months, days, hours, minutes, seconds). The date is checked as `datetime.datetime(...)`
/// checks it (CPython `_datetimemodule.c`, `check_date_args`, `check_time_args`).
pub fn fat_datetime_to_tuple(fat_datetime: i64) -> Result<(i64, i64, i64, i64, i64, i64), PyErr> {
    let seconds = (0b11111 & fat_datetime) * 2;
    let minutes = (0b111111 << 5 & fat_datetime) >> 5;
    let hours = (0b11111 << 11 & fat_datetime) >> 11;
    let days = (0b11111 << 16 & fat_datetime) >> 16;
    let months = (0b1111 << 21 & fat_datetime) >> 21;
    let years = 1980 + ((0b11111111 << 25 & fat_datetime) >> 25);
    // Date validation using the python standart library
    let value_error = |message: &str| Err(PyErr::ValueError(message.to_owned()));
    if !(1..=9999).contains(&years) {
        return value_error(&format!("year {years} is out of range"));
    }
    if !(1..=12).contains(&months) {
        return value_error("month must be in 1..12");
    }
    if days < 1 || days > days_in_month(years, months) {
        return value_error("day is out of range for month");
    }
    if !(0..=23).contains(&hours) {
        return value_error("hour must be in 0..23");
    }
    if !(0..=59).contains(&minutes) {
        return value_error("minute must be in 0..59");
    }
    if !(0..=59).contains(&seconds) {
        return value_error("second must be in 0..59");
    }
    Ok((years, months, days, hours, minutes, seconds))
}

/// `days_in_month` of CPython's `_datetimemodule.c`
fn days_in_month(year: i64, month: i64) -> i64 {
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// `get_print_stats_gcode` (`:187-196`): Returns the gcode for setting print stats. The defaults
/// are `-1` each.
pub fn get_print_stats_gcode(
    quiet_percent: i64,
    quiet_left: i64,
    quiet_change_in: i64,
    normal_percent: i64,
    normal_left: i64,
    normal_change_in: i64,
) -> String {
    format!(
        "M73 Q{quiet_percent} S{quiet_left} C{quiet_change_in} \
         P{normal_percent} R{normal_left} D{normal_change_in}"
    )
}

/// `get_d3_code` (`:199-212`): Gets the D-Code for reading the eeprom
/// - `address`: address in hex
/// - `byte_count`: the number of bytes to read
///
/// Address reference: <https://github.com/prusa3d/Prusa-Firmware/blob/MK3/Firmware/eeprom.cpp>
pub fn get_d3_code(address: i64, byte_count: i64) -> Result<String, PyErr> {
    if !(0 < byte_count && byte_count < 1000) {
        return Err(PyErr::AttributeError(
            "Cannot read that many bytes".to_owned(),
        ));
    }
    if address >= 1 << 16 {
        return Err(PyErr::AttributeError(
            "The address needs to be two bytes long".to_owned(),
        ));
    }
    // format(address, 'x').upper()
    let hex = if address < 0 {
        format!("-{:x}", address.unsigned_abs())
    } else {
        format!("{address:x}")
    };
    Ok(format!("D3 Ax{} C{byte_count}", hex.to_uppercase()))
}

/// `round_to_five` (`:215-227`): Rounds a number to the nearest five. Python's `round` of a float
/// is half to even.
pub fn round_to_five(number: f64) -> i64 {
    ((number / 5.0).round_ties_even() as i64) * 5
}

/// The upper half of CPython's `cp437` codec (`encodings/cp437.py`, `decoding_table` 0x80-0xFF);
/// the lower half is ASCII.
const CP437_HIGH: &str = "\u{00C7}\u{00FC}\u{00E9}\u{00E2}\u{00E4}\u{00E0}\u{00E5}\u{00E7}\u{00EA}\u{00EB}\u{00E8}\u{00EF}\
\u{00EE}\u{00EC}\u{00C4}\u{00C5}\u{00C9}\u{00E6}\u{00C6}\u{00F4}\u{00F6}\u{00F2}\u{00FB}\u{00F9}\
\u{00FF}\u{00D6}\u{00DC}\u{00A2}\u{00A3}\u{00A5}\u{20A7}\u{0192}\u{00E1}\u{00ED}\u{00F3}\u{00FA}\
\u{00F1}\u{00D1}\u{00AA}\u{00BA}\u{00BF}\u{2310}\u{00AC}\u{00BD}\u{00BC}\u{00A1}\u{00AB}\u{00BB}\
\u{2591}\u{2592}\u{2593}\u{2502}\u{2524}\u{2561}\u{2562}\u{2556}\u{2555}\u{2563}\u{2551}\u{2557}\
\u{255D}\u{255C}\u{255B}\u{2510}\u{2514}\u{2534}\u{252C}\u{251C}\u{2500}\u{253C}\u{255E}\u{255F}\
\u{255A}\u{2554}\u{2569}\u{2566}\u{2560}\u{2550}\u{256C}\u{2567}\u{2568}\u{2564}\u{2565}\u{2559}\
\u{2558}\u{2552}\u{2553}\u{256B}\u{256A}\u{2518}\u{250C}\u{2588}\u{2584}\u{258C}\u{2590}\u{2580}\
\u{03B1}\u{00DF}\u{0393}\u{03C0}\u{03A3}\u{03C3}\u{00B5}\u{03C4}\u{03A6}\u{0398}\u{03A9}\u{03B4}\
\u{221E}\u{03C6}\u{03B5}\u{2229}\u{2261}\u{00B1}\u{2265}\u{2264}\u{2320}\u{2321}\u{00F7}\u{2248}\
\u{00B0}\u{2219}\u{00B7}\u{221A}\u{207F}\u{00B2}\u{25A0}\u{00A0}";

/// `bytes.decode("cp437")`: every byte is a character.
fn decode_cp437(line: &[u8]) -> String {
    let high: Vec<char> = CP437_HIGH.chars().collect();
    line.iter()
        .map(|&byte| {
            if byte < 0x80 {
                byte as char
            } else {
                high[usize::from(byte - 0x80)]
            }
        })
        .collect()
}

/// `decode_line` (`:229-231`): Decode a line read from the printer
pub fn decode_line(line: &[u8]) -> String {
    strip(&decode_cp437(line)).replace('\x00', "")
}

/// `is_potato_cpu` (`:234-236`): Returns True if your CPU is a potato.
/// `multiprocessing.cpu_count()` is `os.cpu_count()`, CPython's `sysconf(_SC_NPROCESSORS_ONLN)`;
/// it raises `NotImplementedError` when that gives nothing.
pub fn is_potato_cpu() -> Result<bool, PyErr> {
    Ok(cpu_count()? == 1)
}

#[cfg(unix)]
fn cpu_count() -> Result<i64, PyErr> {
    // SAFETY: sysconf has no preconditions.
    let ncpu = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) };
    if ncpu >= 1 {
        Ok(ncpu as i64)
    } else {
        Err(PyErr::Other(
            "NotImplementedError",
            "cannot determine number of cpus".to_owned(),
        ))
    }
}

#[cfg(not(unix))]
fn cpu_count() -> Result<i64, PyErr> {
    std::thread::available_parallelism()
        .map(|n| n.get() as i64)
        .map_err(|_| {
            PyErr::Other(
                "NotImplementedError",
                "cannot determine number of cpus".to_owned(),
            )
        })
}

/// `PrinterDevice` (`:239-250`): The data model for the usb detected printer
#[derive(Debug, Clone, PartialEq)]
pub struct PrinterDevice {
    pub vendor_id: String,
    pub model_id: String,
    pub serial_number: String,
    pub path: String,
}

/// `walk_dict` (`:300-308`): Walks a dict, yielding the path to each bottom-most value
pub fn walk_dict(
    data: &Map<String, Value>,
    key_path: Option<Vec<String>>,
) -> Vec<(Vec<String>, Value)> {
    let key_path = key_path.unwrap_or_default();
    let mut out = Vec::new();
    for (key, value) in data {
        let mut path = key_path.clone();
        path.push(key.clone());
        if let Value::Object(value) = value {
            out.extend(walk_dict(value, Some(path)));
        } else {
            out.push((path, value.clone()));
        }
    }
    out
}

/// `slots_with_param` (`:311-326`): Fills out the slot information with defaults, only the active
/// one gets the real value. `key` is a field of `IndividualSlot`; pydantic's `ValueError` for any
/// other.
pub fn slots_with_param(
    latest_telemetry: &Telemetry,
    key: &str,
    default: &Value,
    value: &Value,
) -> Result<Option<Vec<(String, IndividualSlot)>>, PyErr> {
    let Some(slot) = &latest_telemetry.slot else {
        return Ok(None);
    };
    let active_slot = slot.active;

    let mut slots = Vec::new();
    for slot in 1..=MMU_SLOTS {
        let slot_name = slot.to_string();
        let mut individual = IndividualSlot::default();
        if Some(slot) == active_slot {
            setattr(&mut individual, key, value)?;
        } else {
            setattr(&mut individual, key, default)?;
        }
        slots.push((slot_name, individual));
    }
    Ok(Some(slots))
}

/// `setattr(IndividualSlot(), key, value)` (pydantic 1.10.13 `BaseModel.__setattr__`, no
/// validation on assignment: the value is stored as given). Here a value of the field's type or
/// `None`; another is a `TypeError` (the module's "not covered").
fn setattr(slot: &mut IndividualSlot, key: &str, value: &Value) -> Result<(), PyErr> {
    let not_typed = || PyErr::TypeError(format!("{key}: {value} is not of the field's type"));
    match key {
        "material" => {
            slot.material = match value {
                Value::Null => None,
                Value::String(s) => Some(s.clone()),
                _ => return Err(not_typed()),
            }
        }
        "temp" => {
            slot.temp = match value {
                Value::Null => None,
                v => Some(v.as_f64().ok_or_else(not_typed)?),
            }
        }
        "fan_hotend" | "fan_print" => {
            let v = match value {
                Value::Null => None,
                v => Some(v.as_i64().ok_or_else(not_typed)?),
            };
            if key == "fan_hotend" {
                slot.fan_hotend = v;
            } else {
                slot.fan_print = v;
            }
        }
        _ => {
            return Err(PyErr::ValueError(format!(
                "\"IndividualSlot\" object has no field \"{key}\""
            )));
        }
    }
    Ok(())
}

/// `_parse_little_endian_uint32` (`:329-333`): Decodes the D-Code specified little-endian
/// uint32_t eeprom variable. `data` is the match's group `"data"`.
pub fn parse_little_endian_uint32(data: &str) -> Result<u32, PyErr> {
    let str_data = data.replace(' ', "");
    let data = fromhex(&str_data)?;
    let bytes: [u8; 4] = data
        .try_into()
        // `struct.error`
        .map_err(|_| PyErr::Other("error", "unpack requires a buffer of 4 bytes".to_owned()))?;
    Ok(u32::from_le_bytes(bytes))
}

/// `bytes.fromhex(s)` (CPython 3.9 `_PyBytes_FromHex`): pairs of hex digits, ASCII whitespace
/// skipped between pairs.
fn fromhex(s: &str) -> Result<Vec<u8>, PyErr> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let error = |pos: usize| {
        PyErr::ValueError(format!(
            "non-hexadecimal number found in fromhex() arg at position {pos}"
        ))
    };
    while i < chars.len() {
        if chars[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let top = chars[i]
            .to_digit(16)
            .filter(|_| chars[i].is_ascii())
            .ok_or_else(|| error(i))?;
        let Some(&next) = chars.get(i + 1) else {
            return Err(error(i + 1));
        };
        let bottom = next
            .to_digit(16)
            .filter(|_| next.is_ascii())
            .ok_or_else(|| error(i + 1))?;
        out.push((top * 16 + bottom) as u8);
        i += 2;
    }
    Ok(out)
}

/// `power_panic_delay` (`:336-351`): Adds a dynamic delay depending on power panic details. This is
/// needed so the printer reaches a stable state before we reset it.
pub fn power_panic_delay(cfg: &Config) -> Result<(), PyErr> {
    let pp_file_path = cfg
        .daemon
        .str("power_panic_file")
        .map_err(|e| PyErr::Other(e.class, e.message))?;
    if !pyos::exists(pp_file_path.as_bytes()) {
        return Ok(());
    }

    let text = std::fs::read_to_string(pp_file_path).map_err(|e| {
        os_error(pyos::Error::Os(pyos::OsError::from_io_with_filename(
            &e,
            pp_file_path.as_bytes(),
        )))
    })?;
    let data: Value =
        serde_json::from_str(&text).map_err(|e| PyErr::Other("JSONDecodeError", e.to_string()))?;
    let pp_data = PPData::from_json(&data)?;
    if pp_data.using_rip_port {
        return Ok(());
    }

    // log.info("Waiting an extra %ss for printer to heat up "
    //          "and finish its moves", PP_MOVES_DELAY)
    std::thread::sleep(Duration::from_secs(PP_MOVES_DELAY as u64));
    Ok(())
}

/// A `pyos` error as the Python exception: `OSError`'s errno class and `str()`.
pub(crate) fn os_error(error: pyos::Error) -> PyErr {
    match error {
        pyos::Error::Os(os) => PyErr::Other(os.class_name(), os.str()),
        pyos::Error::Value(message) => PyErr::ValueError(message),
    }
}

/// `pwd.getpwnam(name)`: `pw_uid`, `pw_gid`
#[cfg(unix)]
fn getpwnam(name: &str) -> Result<(u32, u32), PyErr> {
    let not_found = || PyErr::KeyError(format!("'getpwnam(): name not found: {name}'"));
    let cname = std::ffi::CString::new(name).map_err(|_| not_found())?;
    // SAFETY: cname is NUL-terminated; getpwnam returns null or a pointer to a static entry.
    let entry = unsafe { libc::getpwnam(cname.as_ptr()) };
    if entry.is_null() {
        return Err(not_found());
    }
    // SAFETY: a non-null entry points at a valid passwd struct.
    Ok(unsafe { ((*entry).pw_uid, (*entry).pw_gid) })
}

/// `os.chown(path, uid, gid)`
#[cfg(unix)]
fn chown(path: &str, uid: u32, gid: u32) -> Result<(), PyErr> {
    std::os::unix::fs::chown(path, Some(uid), Some(gid)).map_err(|e| {
        os_error(pyos::Error::Os(pyos::OsError::from_io_with_filename(
            &e,
            path.as_bytes(),
        )))
    })
}

#[cfg(not(unix))]
fn getpwnam(_name: &str) -> Result<(u32, u32), PyErr> {
    Err(PyErr::Other(
        "ModuleNotFoundError",
        "No module named 'pwd'".to_owned(),
    ))
}

#[cfg(not(unix))]
fn chown(_path: &str, _uid: u32, _gid: u32) -> Result<(), PyErr> {
    Err(PyErr::Other(
        "AttributeError",
        "module 'os' has no attribute 'chown'".to_owned(),
    ))
}

#[cfg(test)]
mod tests;
