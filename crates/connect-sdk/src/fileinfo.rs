//! Listing of folders and files for the Lazy File Protocol (`fileinfo.py`).
//!
//! Everything here reads the real disk, nothing is cached.
//!
//! Ported: `get_type_by_name`, `is_inside`, `folder_info`, `file_info`. `node_info` and
//! `resolve` take `files.File` and `files.Storage`: they follow with the port of `files.py`.
//!
//! Paths on the disk are bytes (`os.fsencode` of the SDK's `str`s, see [`crate::pyos`]). Where
//! a name or a metadata value goes into the JSON, ours differs from the reference in what a
//! `serde_json::Value` cannot hold: a name that is not UTF-8 (Python's `str` keeps it with
//! lone surrogates, `json.dumps` writes them as `\udcXX`; ours replaces each bad byte by
//! U+FFFD), a metadata `int` beyond 64 bits (ours: a float) and a NaN or infinite `float`
//! (Python writes `NaN`/`Infinity`; ours `null`). A file path that is not UTF-8 is not read for
//! metadata (gcode-metadata takes a `&str`). None of these is in the tests.

use maker_connect_gcode_metadata::metadata::{MetaError, get_metadata, py_lower};
use maker_connect_gcode_metadata::py::{PyValue, repr_str};
use serde_json::{Map, Value, json};

use crate::command::HandlerError;
use crate::consts::{FIRMWARE_EXTENSION, FileType, GCODE_EXTENSIONS};
use crate::pyos::{self, OsError, StatResult, W_OK};

/// Maximum number of items of a folder, which can be sent in one FILE_INFO.
/// Larger folders are rejected with REASON_OVERSIZED.
pub const MAX_FOLDER_ITEMS: usize = 20000;

// Reasons of rejecting SEND_FILE_INFO. They are part of the contract with
// Connect, so they must not be changed or translated.
pub const REASON_NOT_FOUND: &str = "File not found";
pub const REASON_OVERSIZED: &str = "Oversized command";
pub const REASON_FORBIDDEN: &str = "Forbidden path";

/// What these functions raise: `CommandRejected(reason)`, an exception of the `os` calls, or one
/// gcode-metadata lets through.
#[derive(Debug)]
pub enum Error {
    Rejected(&'static str),
    Os(pyos::Error),
    Meta(MetaError),
}

impl From<pyos::Error> for Error {
    fn from(err: pyos::Error) -> Error {
        Error::Os(err)
    }
}

impl Error {
    /// `repr(err)`, as `command.py` sends an exception other than `CommandRejected`.
    pub fn repr(&self) -> String {
        match self {
            Error::Rejected(reason) => format!("CommandRejected({})", repr_str(reason)),
            Error::Os(err) => err.repr(),
            Error::Meta(MetaError::Os(err)) => OsError::from_io(err).repr(),
            Error::Meta(MetaError::Py(err)) => {
                format!("{}({})", err.class_name(), repr_str(err.message()))
            }
            Error::Meta(MetaError::UnknownGcodeFileType(path)) => {
                format!("UnknownGcodeFileType({})", repr_str(path))
            }
        }
    }

    /// As `Command.__call__` handles it: `CommandRejected` rejects, any other fails.
    pub fn into_handler_error(self) -> HandlerError {
        match self {
            Error::Rejected(reason) => HandlerError::Rejected(reason.to_string()),
            err => HandlerError::Failed(err.repr()),
        }
    }
}

/// Return file type of file/folder by its name
pub fn get_type_by_name(name: &[u8], is_dir: bool) -> FileType {
    if is_dir {
        return FileType::Folder;
    }

    let filename = py_lower(&String::from_utf8_lossy(name));
    if GCODE_EXTENSIONS.iter().any(|ext| filename.ends_with(ext)) {
        return FileType::PrintFile;
    }

    if filename.ends_with(FIRMWARE_EXTENSION) {
        return FileType::Firmware;
    }

    FileType::File
}

/// Check that `os_path` doesn't lead outside of `storage_path`, even
/// through symlinks
pub fn is_inside(os_path: &[u8], storage_path: &[u8]) -> Result<bool, pyos::Error> {
    let storage_real = pyos::realpath(storage_path)?;
    let real = pyos::realpath(os_path)?;
    let mut prefix = storage_real.clone();
    while prefix.last() == Some(&b'/') {
        prefix.pop();
    }
    prefix.push(b'/');
    Ok(real == storage_real || real.starts_with(&prefix))
}

fn name_str(name: &[u8]) -> String {
    String::from_utf8_lossy(name).into_owned()
}

/// `int(st_mtime)`
fn m_timestamp(stats: &StatResult) -> i64 {
    stats.st_mtime as i64
}

/// Return list of direct children of a folder
pub fn folder_info(
    virtual_path: &str,
    storage_path: &[u8],
    os_path: &[u8],
    stats: &StatResult,
) -> Result<Map<String, Value>, Error> {
    let mut children: Vec<Value> = Vec::new();
    for entry in pyos::scandir(os_path)? {
        let mut entry = entry?;
        if entry.name.starts_with(b".") {
            continue;
        }
        if entry.is_symlink()? && !is_inside(&entry.path, storage_path)? {
            continue;
        }
        let (is_dir, entry_stats) = match entry.is_dir(true).and_then(|is_dir| {
            let st = entry.stat(true)?;
            Ok((is_dir, st))
        }) {
            Ok(v) => v,
            // e.g. a broken link or a removed file
            Err(pyos::Error::Os(_)) => continue,
            Err(err) => return Err(err.into()),
        };
        if children.len() >= MAX_FOLDER_ITEMS {
            return Err(Error::Rejected(REASON_OVERSIZED));
        }
        let mut child = Map::new();
        child.insert("name".into(), json!(name_str(&entry.name)));
        child.insert(
            "type".into(),
            json!(get_type_by_name(&entry.name, is_dir).value()),
        );
        child.insert("read_only".into(), json!(!pyos::access(&entry.path, W_OK)?));
        child.insert("m_timestamp".into(), json!(m_timestamp(&entry_stats)));
        if !is_dir {
            child.insert("size".into(), json!(entry_stats.st_size));
        }
        children.push(Value::Object(child));
    }
    let mut info = Map::new();
    info.insert("type".into(), json!(FileType::Folder.value()));
    info.insert("path".into(), json!(virtual_path));
    info.insert("read_only".into(), json!(!pyos::access(os_path, W_OK)?));
    info.insert("m_timestamp".into(), json!(m_timestamp(stats)));
    info.insert("children".into(), Value::Array(children));
    Ok(info)
}

/// A metadata value as `json.dumps` writes it (see the module's header for what differs).
fn py_to_json(value: &PyValue) -> Value {
    match value {
        PyValue::None => Value::Null,
        PyValue::Bool(b) => json!(b),
        PyValue::Int(i) => {
            let s = i.as_str();
            if let Ok(v) = s.parse::<i64>() {
                json!(v)
            } else if let Ok(v) = s.parse::<u64>() {
                json!(v)
            } else {
                serde_json::Number::from_f64(s.parse::<f64>().unwrap_or(f64::NAN))
                    .map_or(Value::Null, Value::Number)
            }
        }
        PyValue::Float(x) => serde_json::Number::from_f64(*x).map_or(Value::Null, Value::Number),
        PyValue::Str(s) => json!(s),
        PyValue::List(items) => Value::Array(items.iter().map(py_to_json).collect()),
        PyValue::Dict(d) => {
            Value::Object(d.iter().map(|(k, v)| (k.clone(), py_to_json(v))).collect())
        }
    }
}

/// Return info and metadata of a file
pub fn file_info(
    virtual_path: &str,
    os_path: &[u8],
    stats: &StatResult,
) -> Result<Map<String, Value>, Error> {
    let mut info = Map::new();
    info.insert(
        "type".into(),
        json!(get_type_by_name(os_path, false).value()),
    );
    info.insert("path".into(), json!(virtual_path));
    info.insert("read_only".into(), json!(!pyos::access(os_path, W_OK)?));
    info.insert("m_timestamp".into(), json!(m_timestamp(stats)));
    info.insert("size".into(), json!(stats.st_size));
    let Ok(os_path) = std::str::from_utf8(os_path) else {
        return Ok(info);
    };
    let meta = match get_metadata(os_path, true, None) {
        Ok(meta) => meta,
        Err(MetaError::UnknownGcodeFileType(_)) => return Ok(info),
        Err(err) if err.is_file_not_found() => return Ok(info),
        Err(err) => return Err(Error::Meta(err)),
    };
    // dict.update: a key already there keeps its place
    for (k, v) in meta.data.iter() {
        info.insert(k.clone(), py_to_json(v));
    }

    // include the biggest thumbnail, if available
    if !meta.thumbnails.is_empty() {
        let mut biggest: &[u8] = b"";
        for data in meta.thumbnails.values() {
            if data.len() > biggest.len() {
                biggest = data;
            }
        }
        let preview =
            String::from_utf8(biggest.to_vec()).expect("a thumbnail is the encoding of a str");
        info.insert("preview".into(), json!(preview));
    }
    Ok(info)
}
