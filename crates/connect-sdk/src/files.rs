//! File management (`files.py`).
//!
//! Reference: the SDK at `c0d7d233` (the same file at `04c5bd5`), its
//! `prusa/connect/printer/files.py`, ported function by function. Linux, macOS and Windows, as [`crate::pyos`].
//!
//! Paths and names are the SDK's `str`s; on the disk they are bytes ([`crate::pyos`]). A name
//! read from the disk that is not UTF-8 is read with U+FFFD for each bad byte, where Python
//! keeps it with lone surrogates (as [`crate::fileinfo`]). The Python exceptions are [`Error`].
//! A `File` node is shared (`Arc<Mutex<..>>`); its parent is a `Weak`, as Python's
//! `weakref.proxy`. `File.__getattr__` (`node.a.b`) is [`File::child`]; `get` with a `str`
//! (a `TypeError`) cannot be written in Rust.
//!
//! **Ours: the watching, through the crate `notify`** (owner, 2026-11-25, #236: "volledig gebruik
//! te maken van die rust notify voor alles. 1 codebase"). The reference's `InotifyHandler` reads
//! the events of `inotify_simple.INotify` (`wd`, `mask`, `name`), to which it adds one watch per
//! folder (`update_watch_dir`). Here each of those watches is a `notify` watch of that folder
//! alone (`RecursiveMode::NonRecursive`) that carries the `wd` as its `user_data`, asking for
//! the kinds `WATCH_FLAGS` names (`EventKindMask` `CREATE | REMOVE | MODIFY_NAME |
//! ACCESS_CLOSE`: the kernel mask `IN_CREATE | IN_DELETE | IN_MOVED_FROM | IN_MOVED_TO |
//! IN_CLOSE_WRITE`, and notify adds `IN_DELETE_SELF | IN_MOVE_SELF` to each such watch).
//! On Linux, where notify reads inotify itself, [`to_inotify`] turns each of notify's events back
//! into the event it stands for; the rest of the handler is the reference's:
//!
//! | notify | inotify event |
//! |---|---|
//! | `Create(Folder)` / `Create(File)` | `CREATE \| ISDIR` / `CREATE` |
//! | `Remove(_)` of the watched folder itself | `DELETE_SELF`, then `IGNORED` (the kernel's) |
//! | `Remove(Other)` with info `unmount` | `UNMOUNT`, then `IGNORED` |
//! | `Remove(Folder)` / `Remove(File)` | `DELETE \| ISDIR` / `DELETE` |
//! | `Modify(Name(From))` of the watched folder, no tracker | `MOVE_SELF` |
//! | `Modify(Name(From))` | `MOVED_FROM` |
//! | `Modify(Name(To))` | `MOVED_TO`, `\| ISDIR` when `lstat` of the path says a folder |
//! | `Modify(Name(Both))` | none: notify's own pairing of the two above |
//! | `Access(Close(Write))` | `CLOSE_WRITE` |
//! | `Other` with `Flag::Rescan` | `Q_OVERFLOW` (`wd` -1) |
//!
//! The `name` is the last part of the event's path, empty for the watched folder itself.
//! What differs from the reference, by this:
//! - `handler(timeout)` reads what notify's thread has passed on by then, where `INotify.read`
//!   reads the kernel's queue: right after a file action, its events may not have arrived yet.
//!   `timeout` (milliseconds, as `inotify_simple`'s: 0 returns at once, negative blocks) waits
//!   for the first event only, as `INotify.read` does.
//! - The kernel's `ISDIR` of a `MOVED_TO` is read again from the disk (notify drops it).
//! - notify ends the watch of a folder that is moved or deleted (`inotify.rs`, `MOVED_FROM` and
//!   `DELETE`: `queue_watch_removal`) and swallows the kernel's `IGNORED` of it; with
//!   `inotify_simple` the watch of a moved folder stays and reports under its old path. The
//!   kernel's reuse of a `wd` for a folder it already watches is ours to keep (`file_id`): on
//!   Linux and macOS by its device and inode, on Windows not.
//! - An error notify passes on is logged (`inotify_simple` has none). Outside Linux, one with a
//!   watch's path and `user_data` (a watch notify could not arm again) is then looked at as an
//!   event of that path; any other is dropped.
//! - On macOS (FSEvents) and Windows (`ReadDirectoryChangesW`) there is no `CLOSE_WRITE` and
//!   no paired rename, and FSEvents repeats a path's earlier flags with each later change. There
//!   each of notify's events only says which path to look at: what the disk holds now, against
//!   what the watched folder held, gives the inotify events the kernel would have given
//!   (`State::translate`, its table). The handler's tests run on all three; the comparison with
//!   the reference itself (`tests/files_reference.rs`) on Linux, where `inotify_simple` runs.

use std::io::Write;
use std::path::Path;
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use indexmap::IndexMap;
use maker_connect_gcode_metadata::metadata::{MetaError, get_metadata};
use maker_connect_gcode_metadata::py::repr_str;
use notify::event::{AccessKind, AccessMode, CreateKind, ModifyKind, RemoveKind, RenameMode};
use notify::{
    Config, EventKind, EventKindMask, RecommendedWatcher, RecursiveMode, WatchPathConfig, Watcher,
};
use serde_json::{Map, Value, json};

use crate::clock::time;
use crate::consts::{Event, FileType, Source, StorageType};
use crate::fileinfo::{self, get_type_by_name};
use crate::models::{EventCallback, Kwargs};
use crate::pyos::{self, Filename, OsError, W_OK};

/// More inotify events than this in one batch are reported to Connect as
/// one change with `rescan`, instead of FILE_CHANGED for each of them.
pub const RESCAN_THRESHOLD: usize = 20;

/// What `files.py` raises.
#[derive(Debug)]
pub enum Error {
    /// `InvalidStorageError(message)`, a `ValueError`.
    InvalidStorage(String),
    /// `ValueError(message)`.
    Value(String),
    /// An exception of an `os` call.
    Os(pyos::Error),
    /// `FileNotFoundError(message)`, raised by `files.py` itself.
    FileNotFound(String),
    /// `KeyError(key)`: the `repr` of the key.
    Key(String),
    /// `IndexError(message)`.
    Index(String),
    /// `AttributeError(message)`: an attribute of `None`.
    Attribute(String),
    /// `TypeError(message)`.
    Type(String),
    /// `AssertionError()`.
    Assertion,
    /// `ReferenceError`: the parent of a node is gone (`weakref.proxy`).
    Reference,
    /// An exception of gcode-metadata that `create_cache` lets through.
    Meta(MetaError),
}

impl Error {
    /// `repr(err)`.
    pub fn repr(&self) -> String {
        match self {
            Error::InvalidStorage(m) => format!("InvalidStorageError({})", repr_str(m)),
            Error::Value(m) => format!("ValueError({})", repr_str(m)),
            Error::Os(err) => err.repr(),
            Error::FileNotFound(m) => format!("FileNotFoundError({})", repr_str(m)),
            Error::Key(key) => format!("KeyError({key})"),
            Error::Index(m) => format!("IndexError({})", repr_str(m)),
            Error::Attribute(m) => format!("AttributeError({})", repr_str(m)),
            Error::Type(m) => format!("TypeError({})", repr_str(m)),
            Error::Assertion => "AssertionError()".into(),
            Error::Reference => {
                "ReferenceError('weakly-referenced object no longer exists')".into()
            }
            Error::Meta(MetaError::Os(err)) => OsError::from_io(err).repr(),
            Error::Meta(MetaError::Py(err)) => {
                format!("{}({})", err.class_name(), repr_str(err.message()))
            }
            Error::Meta(MetaError::UnknownGcodeFileType(path)) => {
                format!("UnknownGcodeFileType({})", repr_str(path))
            }
        }
    }

    /// `isinstance(err, OSError)`.
    pub fn is_os_error(&self) -> bool {
        matches!(
            self,
            Error::Os(pyos::Error::Os(_)) | Error::FileNotFound(_) | Error::Meta(MetaError::Os(_))
        )
    }
}

impl From<pyos::Error> for Error {
    fn from(err: pyos::Error) -> Error {
        Error::Os(err)
    }
}

/// `AttributeError` of an attribute read on `None`.
fn none_attribute(name: &str) -> Error {
    Error::Attribute(format!("'NoneType' object has no attribute '{name}'"))
}

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// `posixpath.join(a, *p)` of `str`s.
fn join(a: &str, p: &[&str]) -> String {
    let p: Vec<&[u8]> = p.iter().map(|s| s.as_bytes()).collect();
    lossy(&pyos::join(a.as_bytes(), &p))
}

/// `s.strip(chars)`, `s.rstrip(chars)`
fn strip<'a>(s: &'a str, chars: &str) -> &'a str {
    s.trim_matches(|c| chars.contains(c))
}

fn rstrip<'a>(s: &'a str, chars: &str) -> &'a str {
    s.trim_end_matches(|c| chars.contains(c))
}

/// `s[n:]` of a `str`: by characters.
fn slice_from(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[i..],
        None => "",
    }
}

// https://stackoverflow.com/a/18715879
/// returns the longest common substring from the beginning of sa and sb
pub fn common_start(sa: &str, sb: &str) -> String {
    sa.chars()
        .zip(sb.chars())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a)
        .collect()
}

/// Ours: whether `path` starts with a Windows drive (`C:/`, `C:\\`); never elsewhere.
fn has_drive(path: &str) -> bool {
    let b = path.as_bytes();
    cfg!(windows)
        && b.len() >= 3
        && b[0].is_ascii_alphabetic()
        && b[1] == b':'
        && matches!(b[2], b'/' | b'\\')
}

/// Delete file or folder.
///
/// `abs_path`: absolute path; `is_dir`: True if folder; `force`: Force delete non-empty folder
/// if True
pub fn delete(abs_path: &str, is_dir: bool, force: bool) -> Result<(), Error> {
    let p = abs_path.as_bytes();
    if pyos::exists(p) {
        if is_dir {
            if force {
                pyos::rmtree(p)?;
            } else {
                pyos::rmdir(p)?;
            }
        } else {
            pyos::unlink(p)?;
        }
        Ok(())
    } else {
        Err(Error::FileNotFound(format!(
            "{abs_path}. File or folder doesn't exist."
        )))
    }
}

/// Return file type of file/folder
pub fn get_file_type(file: &File) -> FileType {
    let (name, is_dir) = {
        let node = file.lock();
        (node.name.clone(), node.is_dir)
    };
    get_type_by_name(name.as_bytes(), is_dir)
}

struct Node {
    name: String,
    is_dir: bool,
    parent: Option<Weak<Mutex<Node>>>,
    attrs: Map<String, Value>,
    children: IndexMap<String, File>,
}

/// A node of a Filesystem representing either a file or a folder
#[derive(Clone)]
pub struct File(Arc<Mutex<Node>>);

impl std::fmt::Debug for File {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "File({:?})", self.name())
    }
}

impl std::fmt::Display for File {
    /// `__str__`
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name())
    }
}

impl File {
    /// Create a File object
    ///
    /// `name`: Filename; `is_dir`: Flag whether this is a folder; `parent`: Parent for this
    /// File, which itself is a File(is_dir=True); `attrs`: Any attributes for the file you want
    /// to store. File's to_dict() method add `read_only`, `m_timestamp` and `size` attributes,
    /// if it finds them.
    pub fn new(name: &str, is_dir: bool, parent: Option<&File>, attrs: Map<String, Value>) -> File {
        File(Arc::new(Mutex::new(Node {
            name: name.to_string(),
            is_dir,
            parent: parent.map(|p| Arc::downgrade(&p.0)),
            attrs,
            children: IndexMap::new(),
        })))
    }

    /// A folder or a file without attributes: `File(name, is_dir=..)`.
    pub fn node(name: &str, is_dir: bool) -> File {
        File::new(name, is_dir, None, Map::new())
    }

    fn lock(&self) -> MutexGuard<'_, Node> {
        self.0.lock().unwrap()
    }

    pub fn name(&self) -> String {
        self.lock().name.clone()
    }

    pub fn is_dir(&self) -> bool {
        self.lock().is_dir
    }

    /// `self.attrs`, a copy.
    pub fn attrs(&self) -> Map<String, Value> {
        self.lock().attrs.clone()
    }

    /// `self.attrs = attrs`
    pub fn set_attrs(&self, attrs: Map<String, Value>) {
        self.lock().attrs = attrs;
    }

    /// `self.children.values()`, in their order.
    pub fn children(&self) -> Vec<File> {
        self.lock().children.values().cloned().collect()
    }

    /// `self.children = {}`
    pub fn clear_children(&self) {
        self.lock().children = IndexMap::new();
    }

    /// The same node (`is`).
    pub fn ptr_eq(&self, other: &File) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// Return `size` from `self.attrs` for a FILE or compute it for a folder.
    pub fn size(&self) -> i64 {
        let children = {
            let node = self.lock();
            if !node.is_dir {
                // file
                return node.attrs.get("size").and_then(Value::as_i64).unwrap_or(0);
            }
            node.children.values().cloned().collect::<Vec<_>>()
        };

        // folder
        let mut size = 0;
        for child in children {
            size += child.size();
        }
        size
    }

    /// `self.size = value`
    pub fn set_size(&self, value: i64) {
        self.lock().attrs.insert("size".into(), json!(value));
    }

    /// Gets a parent node (`None` for a root; a parent that is gone is a `ReferenceError`)
    pub fn parent(&self) -> Result<Option<File>, Error> {
        match &self.lock().parent {
            None => Ok(None),
            Some(weak) => weak.upgrade().map(File).map(Some).ok_or(Error::Reference),
        }
    }

    /// Sets a parent node, uses weakref, to stop the creation of a refloop
    pub fn set_parent(&self, parent: &File) {
        self.lock().parent = Some(Arc::downgrade(&parent.0));
    }

    /// Add a file to this File's children. Note that `self` must be a folder.
    ///
    /// Raises ValueError if self is not a folder; returns the added file.
    pub fn add(&self, name: &str, is_dir: bool) -> Result<File, Error> {
        if !self.is_dir() {
            return Err(Error::Value("You can add only to directories".into()));
        }
        let node = File::new(name, is_dir, Some(self), Map::new());
        // Ignore hidden files and folders .<filename> / .<foldername>
        if !name.starts_with('.') {
            self.lock().children.insert(name.to_string(), node.clone());
        }
        Ok(node)
    }

    /// Return the node identified by `parts`, which is a collection of names that will be
    /// matched on the path to it.
    pub fn get<S: AsRef<str>>(&self, parts: &[S]) -> Option<File> {
        if parts.len() == 1 && parts[0].as_ref().is_empty() {
            return Some(self.clone());
        }

        let mut last = self.clone();
        for part in parts {
            let node = last.child(part.as_ref())?;
            last = node;
        }
        Some(last)
    }

    /// Return all the parts until root
    pub fn abs_parts(&self) -> Result<Vec<String>, Error> {
        let mut result = Vec::new();
        let mut node = self.clone();
        while let Some(parent) = node.parent()? {
            result.insert(0, node.name());
            node = parent;
        }
        Ok(result)
    }

    /// Return the absolute path of this File
    ///
    /// `prefix`: prefix as a path to be used for absolute path generation
    pub fn abs_path(&self, prefix: &str) -> Result<String, Error> {
        let mut prefix = prefix.to_string();
        // Ours, on Windows: a path with a drive (`C:/`) is absolute already, where the
        // reference's `path.sep + prefix` makes it `\\C:\\...`, which Windows refuses.
        if !prefix.starts_with('/') && !has_drive(&prefix) {
            prefix = format!("/{prefix}");
        }
        Ok(join(&prefix, &[&self.abs_parts()?.join("/")]))
    }

    /// Delete this node
    pub fn delete(&self) -> Result<(), Error> {
        if let Some(parent) = self.parent()? {
            // only if parent is set
            let name = self.name();
            if parent.lock().children.shift_remove(&name).is_none() {
                return Err(Error::Key(repr_str(&name)));
            }
        }
        Ok(())
    }

    /// Pretty print the File as a tree. `self` should be a folder for this method to make any
    /// makes sense.
    pub fn pprint(&self, file: &mut dyn Write) -> std::io::Result<()> {
        self.pprint_from(file, "", true, true)
    }

    fn pprint_from(
        &self,
        file: &mut dyn Write,
        prefix: &str,
        last: bool,
        first: bool,
    ) -> std::io::Result<()> {
        let symbol = if last { "└─" } else { "├─" };
        let name = self.name();
        let line = if first {
            format!("{symbol} {name}")
        } else {
            format!("{prefix} {symbol} {name}")
        };
        writeln!(file, "{line}")?;
        let prefix = format!("{prefix}{}", if last { "   " } else { " │  " });
        let children = self.children();
        let mut counter = children.len();
        for child in children {
            counter -= 1;
            child.pprint_from(file, &prefix, counter == 0, false)?;
        }
        Ok(())
    }

    /// Return self in the format for Connect Backend
    pub fn to_dict(&self, include_children: bool) -> Map<String, Value> {
        let mut result = Map::new();
        result.insert("type".into(), json!(get_file_type(self).value()));
        let (name, is_dir, attrs) = {
            let node = self.lock();
            (node.name.clone(), node.is_dir, node.attrs.clone())
        };
        result.insert("name".into(), json!(name));
        for attr in ["read_only", "m_timestamp"] {
            if let Some(v) = attrs.get(attr) {
                result.insert(attr.into(), v.clone());
            }
        }
        result.insert("size".into(), json!(self.size()));

        if is_dir && include_children {
            let children: Vec<Value> = self
                .children()
                .iter()
                .map(|child| Value::Object(child.to_dict(false)))
                .collect();
            result.insert("children".into(), Value::Array(children));
        }

        result
    }

    /// `item in self`: `__contains__`
    pub fn contains(&self, item: &str) -> bool {
        self.lock().children.contains_key(item)
    }

    /// `self[name]`, `self.name`: `__getitem__`, `__getattr__`
    pub fn child(&self, name: &str) -> Option<File> {
        self.lock().children.get(name).cloned()
    }

    /// Read and fill `read_only`, `size_` and `m_timestamp` attributes on this file according
    /// to `abs_path` file on storage.
    pub fn fill_attrs(&self, abs_path: &str) -> Result<(), Error> {
        let p = abs_path.as_bytes();
        let result = (|| -> Result<(), pyos::Error> {
            let stats = pyos::stat(p)?;
            let read_only = !pyos::access(p, W_OK)?;
            self.lock()
                .attrs
                .insert("read_only".into(), json!(read_only));
            if !self.is_dir() {
                self.set_size(stats.st_size);
            }
            self.lock()
                .attrs
                .insert("m_timestamp".into(), json!(stats.st_mtime as i64));
            Ok(())
        })();
        match result {
            Ok(()) => Ok(()),
            Err(pyos::Error::Os(err)) => {
                log::error!("Failed to get attributes from `{abs_path}`: {}", err.repr());
                Ok(())
            }
            Err(err) => Err(err.into()),
        }
    }
}

/// Represent a storage
pub struct Storage {
    pub tree: File,
    pub storage: String,
    pub path_storage: Option<String>,
    pub use_inotify: bool,
    pub storage_type: StorageType,
    pub system_space: Option<i64>,
    last_updated: Mutex<f64>,
}

impl std::fmt::Debug for Storage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.str())
    }
}

impl Storage {
    /// Initialize a Storage.
    ///
    /// `tree`: tree of File instances; `storage`: storage on the virtual FS;
    /// `abs_path_storage`: absolute path on the physical storage; `use_inotify`: whether to
    /// handle this storage using inotify; `storage_type`: type of the storage reported to
    /// Connect; `system_space`: space (in bytes) taken by files which do not belong to the user,
    /// e.g. the application itself
    pub fn new(
        tree: File,
        storage: &str,
        abs_path_storage: Option<&str>,
        use_inotify: bool,
        storage_type: StorageType,
        system_space: Option<i64>,
    ) -> Result<Storage, Error> {
        if use_inotify && abs_path_storage.is_none_or(str::is_empty) {
            let msg = "`use_inotify` requires `abs_path_storage` to be set";
            return Err(Error::Value(msg.into()));
        }
        // TODO check other storage if there is already abs_path_storage used
        Ok(Storage {
            tree,
            storage: storage.to_string(),
            path_storage: abs_path_storage.map(str::to_string),
            use_inotify,
            storage_type,
            system_space,
            last_updated: Mutex::new(time()),
        })
    }

    pub fn last_updated(&self) -> f64 {
        *self.last_updated.lock().unwrap()
    }

    /// `self.path_storage`, when it is set and not empty.
    fn path(&self) -> Option<&str> {
        self.path_storage.as_deref().filter(|p| !p.is_empty())
    }

    /// Returns free space of storage in bytes
    pub fn get_space_info(&self) -> Result<Map<String, Value>, Error> {
        if let Some(path_storage) = self.path()
            && pyos::exists(path_storage.as_bytes())
        {
            let path_ = pyos::statvfs(path_storage.as_bytes())?;
            let free_space = path_.f_bavail * path_.f_bsize;
            let total_space = path_.f_blocks * path_.f_bsize;

            let mut space_info = Map::new();
            space_info.insert("free_space".into(), json!(free_space));
            space_info.insert("total_space".into(), json!(total_space));
            if let Some(system_space) = self.system_space {
                space_info.insert("system_space".into(), json!(system_space));
            }
            return Ok(space_info);
        }
        Ok(Map::new())
    }

    /// Returns the description of the storage in the format for Connect.
    ///
    /// The description contains no files, Connect asks for them by SEND_FILE_INFO command.
    pub fn to_dict(&self) -> Result<Map<String, Value>, Error> {
        let mut result = Map::new();
        result.insert("mountpoint".into(), json!(format!("/{}", self.storage)));
        result.insert("name".into(), json!(self.storage));
        result.insert("type".into(), json!(self.storage_type.value()));
        result.insert("is_sfn".into(), json!(false));
        if let Some(path_storage) = self.path() {
            result.insert(
                "read_only".into(),
                json!(!pyos::access(path_storage.as_bytes(), W_OK)?),
            );
        }
        for (k, v) in self.get_space_info()? {
            result.insert(k, v);
        }
        Ok(result)
    }

    /// `__str__`, `__repr__`
    pub fn str(&self) -> String {
        let path = match &self.path_storage {
            Some(p) => p.clone(),
            None => "None".into(),
        };
        format!("Storage({} -> {path})", self.storage)
    }
}

/// Model a collection of Files (which are grouped in to trees). This is flat like the DOS
/// filesystem, therefore unlike in UNIX OSes trees cannot be nested in each other.
///
/// A filesystem translates from physical representation on the storage to virtual. This
/// virtual one is then sent to Connect.
pub struct Filesystem {
    pub sep: String,
    storage_dict: Mutex<IndexMap<String, Arc<Storage>>>,
    checked_files: Mutex<Vec<String>>,
    event_cb: Option<EventCallback>,
}

impl Filesystem {
    /// Create a Filesystem (FS).
    ///
    /// `sep`: Separator on the FS; `event_cb`: SDK's Printer.event_cb method. If set, the FS
    /// will call callback to put events to event queue on attach/detach operations and the
    /// InotifyHandler on changes to the FS.
    pub fn new(sep: &str, event_cb: Option<EventCallback>) -> Filesystem {
        Filesystem {
            sep: sep.to_string(),
            storage_dict: Mutex::new(IndexMap::new()),
            checked_files: Mutex::new(Vec::new()),
            event_cb,
        }
    }

    /// `self.storage_dict`
    pub fn storage_dict(&self) -> MutexGuard<'_, IndexMap<String, Arc<Storage>>> {
        self.storage_dict.lock().unwrap()
    }

    /// `self.storage_dict.values()`, a copy.
    fn storages_list(&self) -> Vec<Arc<Storage>> {
        self.storage_dict().values().cloned().collect()
    }

    /// `self.checked_files`, a copy.
    pub fn checked_files(&self) -> Vec<String> {
        self.checked_files.lock().unwrap().clone()
    }

    /// Attach the tree under a storage.
    ///
    /// `name`: The storage; `tree`: The tree of `File` instances to be attached;
    /// `storage_path`: Path on storage; `use_inotify`: Whether to use inotify on this storage
    /// or not; `storage_type`: Type of the storage reported to Connect; `system_space`: Space
    /// taken by files not belonging to the user.
    ///
    /// Raises InvalidStorageError: If the storage is already used, or when it contains
    /// `self.sep` or when the `name` is empty or `self.sep` only.
    pub fn attach(
        &self,
        name: &str,
        tree: File,
        storage_path: &str,
        use_inotify: bool,
        storage_type: StorageType,
        system_space: Option<i64>,
    ) -> Result<(), Error> {
        if name.is_empty() {
            return Err(Error::InvalidStorage("Storage cannot be empty".into()));
        }

        if name.contains(self.sep.as_str()) {
            let msg = format!("Storage cannot contain {}", self.sep);
            return Err(Error::InvalidStorage(msg));
        }

        let storage = {
            let mut storage_dict = self.storage_dict();
            if storage_dict.contains_key(name) {
                return Err(Error::InvalidStorage(format!("`{name}` is already used")));
            }

            let storage = Arc::new(Storage::new(
                tree,
                name,
                Some(storage_path),
                use_inotify,
                storage_type,
                system_space,
            )?);
            storage_dict.insert(name.to_string(), storage.clone());
            storage
        };

        // send MEDIUM_INSERTED event
        if self.event_cb.is_some() {
            let payload = storage.to_dict()?;
            self.connect_event(Event::MediumInserted, payload);
        }
        Ok(())
    }

    /// Detach a storage.
    ///
    /// Raises InvalidStorageError if `name` is not atached
    pub fn detach(&self, name: &str) -> Result<(), Error> {
        if self.storage_dict().shift_remove(name).is_none() {
            let msg = format!("`{name}` is not used as a storage");
            return Err(Error::InvalidStorage(msg));
        }

        // send MEDIUM_EJECTED event
        if self.event_cb.is_some() {
            let mut payload = Map::new();
            payload.insert("mountpoint".into(), json!(format!("{}{name}", self.sep)));
            self.connect_event(Event::MediumEjected, payload);
        }
        Ok(())
    }

    /// Return the File addressed by `abs_path`
    pub fn get(&self, abs_path: &str) -> Option<File> {
        let abs_path = strip(abs_path, &self.sep);
        let mut split = abs_path.split(self.sep.as_str());
        let storage = split.next().unwrap_or_default();
        let parts: Vec<&str> = split.collect();

        let tree = self.storage_dict().get(storage)?.tree.clone();
        tree.get(&parts)
    }

    /// Update storage.tree structure.
    ///
    /// Add the nearest part of real file system to tree.
    ///
    /// Example: `Filesystem.update(['/tmp/tmpvbqhald4/folder/a'], '/tmp/tmpvbqhald4/folder',
    /// File('test'))`: result is node File('a') is add to File('test')
    pub fn update(
        abs_paths: &[String],
        abs_storage: &str,
        node: Option<&File>,
    ) -> Result<(), Error> {
        if let Some(node) = node {
            let relative_paths = InotifyHandler::get_relative_paths(abs_storage, abs_paths)?;

            for item in relative_paths {
                if !item.contains('/') && item != "/" {
                    node.add(&item, true)?;
                }
            }
        }
        Ok(())
    }

    /// Gets the OS file path of the file specified by abs_path
    pub fn get_os_path(&self, abs_path: &str) -> Result<String, Error> {
        let file = self.get(abs_path);
        let abs_path = strip(abs_path, &self.sep);
        let storage_name = abs_path.split(self.sep.as_str()).next().unwrap_or_default();
        let storage = self
            .storage_dict()
            .get(storage_name)
            .cloned()
            .ok_or_else(|| Error::Key(repr_str(storage_name)))?;
        let file = file.ok_or_else(|| none_attribute("abs_path"))?;
        let path_storage = storage
            .path_storage
            .as_deref()
            .ok_or_else(|| none_attribute("startswith"))?;
        file.abs_path(path_storage)
    }

    /// Returns descriptions of all storages in the format for Connect.
    ///
    /// It doesn't touch files, so it is fast regardless of the amount of files on the storages.
    pub fn storages(&self) -> Result<Vec<Map<String, Value>>, Error> {
        self.storages_list().iter().map(|s| s.to_dict()).collect()
    }

    /// Translate a path on the virtual FS to the path on the disk.
    ///
    /// See `fileinfo.resolve`.
    pub fn resolve(
        &self,
        abs_path: &str,
    ) -> Result<(String, Arc<Storage>, Vec<u8>), fileinfo::Error> {
        let storage_dict = self.storage_dict().clone();
        fileinfo::resolve(&storage_dict, &self.sep, abs_path)
    }

    /// Return the data of FILE_INFO event for a file or a folder.
    ///
    /// The data are read from the disk. For a folder, only its direct children are listed.
    ///
    /// A storage attached without the path on the disk (e.g. an SD card of a printer, which is
    /// not accessible as a folder) has nothing to read, so it is answered from its tree. The
    /// owner of the storage keeps the tree up to date.
    ///
    /// Raises CommandRejected: see `resolve`, `Oversized command` is raised if the folder has
    /// more than `MAX_FOLDER_ITEMS` items
    pub fn file_info(&self, abs_path: &str) -> Result<Map<String, Value>, fileinfo::Error> {
        let (virtual_path, storage, os_path) = self.resolve(abs_path)?;
        let Some(path_storage) = storage.path() else {
            let parts: Vec<&str> = virtual_path.split(self.sep.as_str()).skip(2).collect();
            let node = storage.tree.get(&parts).ok_or_else(|| {
                fileinfo::Error::Attribute("'NoneType' object has no attribute 'name'".into())
            })?;
            return fileinfo::node_info(&virtual_path, &node);
        };
        let result = (|| {
            let stats = pyos::stat(&os_path)?;
            if pyos::isdir(&os_path) {
                return fileinfo::folder_info(
                    &virtual_path,
                    path_storage.as_bytes(),
                    &os_path,
                    &stats,
                );
            }
            fileinfo::file_info(&virtual_path, &os_path, &stats)
        })();
        match result {
            Err(err) if fileinfo::is_not_found_or_not_a_directory(&err) => {
                Err(fileinfo::Error::Rejected(fileinfo::REASON_NOT_FOUND))
            }
            other => other,
        }
    }

    /// Initialize a (File) tree from `dirpath` and attach it.
    ///
    /// `dirpath`: The folder on store from which to create the FS; `storage`: Storage;
    /// `storage_type`: Type of the storage reported to Connect; `system_space`: Space taken by
    /// files not belonging to the user
    pub fn from_dir(
        &self,
        dirpath: &str,
        storage: &str,
        storage_type: StorageType,
        system_space: Option<i64>,
    ) -> Result<(), Error> {
        // normalize dirpath
        let mut dirpath = lossy(&pyos::abspath(dirpath.as_bytes())?);
        if !dirpath.ends_with('/') {
            dirpath.push('/');
        }

        let root = File::node(storage, true);
        root.fill_attrs(&dirpath)?;

        let prefix_len = dirpath.chars().count();
        for item in pyos::walk(dirpath.as_bytes(), true, None, false) {
            let (abs_dir, dirs, files) = item?;
            let abs_dir = lossy(&abs_dir);
            let dirnames: Vec<&str> = slice_from(&abs_dir, prefix_len).split('/').collect();

            // skip hidden folders
            if dirnames.iter().any(|e| e.starts_with('.')) {
                continue;
            }

            let parent = root.get(&dirnames);
            let parent = || parent.as_ref().ok_or_else(|| none_attribute("add"));

            for name in dirs {
                let name = lossy(&name);
                let node = parent()?.add(&name, true)?;
                node.fill_attrs(&join(&abs_dir, &[&name]))?;
            }
            for name in files {
                let name = lossy(&name);
                let node = parent()?.add(&name, false)?;
                node.fill_attrs(&join(&abs_dir, &[&name]))?;
            }
        }

        self.attach(storage, root, &dirpath, true, storage_type, system_space)
    }

    /// Send an event to connect if `self.events` is set
    pub fn connect_event(&self, event: Event, data: Kwargs) {
        if let Some(event_cb) = &self.event_cb {
            event_cb(event, Source::Wui, None, None, data);
        }
    }

    /// Wait for the max time until a file is appended to the path by inotify.
    pub fn wait_until_path(&self, path_: &str, wait_timeout: f64) -> bool {
        let mut i = 0.0;
        while wait_timeout < 0.0 || i < wait_timeout {
            if self.get(path_).is_some() {
                return true;
            }
            std::thread::sleep(Duration::from_secs_f64(0.1));
            i += 0.1;
        }
        false
    }
}

/// `inotify_simple.flags`, the bits of an event's `mask`.
pub mod flags {
    pub const ACCESS: u32 = 0x1;
    pub const MODIFY: u32 = 0x2;
    pub const ATTRIB: u32 = 0x4;
    pub const CLOSE_WRITE: u32 = 0x8;
    pub const CLOSE_NOWRITE: u32 = 0x10;
    pub const OPEN: u32 = 0x20;
    pub const MOVED_FROM: u32 = 0x40;
    pub const MOVED_TO: u32 = 0x80;
    pub const CREATE: u32 = 0x100;
    pub const DELETE: u32 = 0x200;
    pub const DELETE_SELF: u32 = 0x400;
    pub const MOVE_SELF: u32 = 0x800;
    pub const UNMOUNT: u32 = 0x2000;
    pub const Q_OVERFLOW: u32 = 0x4000;
    pub const IGNORED: u32 = 0x8000;
    pub const ONLYDIR: u32 = 0x1000000;
    pub const DONT_FOLLOW: u32 = 0x2000000;
    pub const EXCL_UNLINK: u32 = 0x4000000;
    pub const MASK_CREATE: u32 = 0x10000000;
    pub const MASK_ADD: u32 = 0x20000000;
    pub const ISDIR: u32 = 0x40000000;
    pub const ONESHOT: u32 = 0x80000000;

    /// `flags` in the order of the enum, with their names.
    pub const ALL: [(u32, &str); 22] = [
        (ACCESS, "ACCESS"),
        (MODIFY, "MODIFY"),
        (ATTRIB, "ATTRIB"),
        (CLOSE_WRITE, "CLOSE_WRITE"),
        (CLOSE_NOWRITE, "CLOSE_NOWRITE"),
        (OPEN, "OPEN"),
        (MOVED_FROM, "MOVED_FROM"),
        (MOVED_TO, "MOVED_TO"),
        (CREATE, "CREATE"),
        (DELETE, "DELETE"),
        (DELETE_SELF, "DELETE_SELF"),
        (MOVE_SELF, "MOVE_SELF"),
        (UNMOUNT, "UNMOUNT"),
        (Q_OVERFLOW, "Q_OVERFLOW"),
        (IGNORED, "IGNORED"),
        (ONLYDIR, "ONLYDIR"),
        (DONT_FOLLOW, "DONT_FOLLOW"),
        (EXCL_UNLINK, "EXCL_UNLINK"),
        (MASK_CREATE, "MASK_CREATE"),
        (MASK_ADD, "MASK_ADD"),
        (ISDIR, "ISDIR"),
        (ONESHOT, "ONESHOT"),
    ];

    /// `flags.from_mask(mask)`: the flags in `mask`, in the order of the enum.
    pub fn from_mask(mask: u32) -> Vec<u32> {
        ALL.iter()
            .map(|(f, _)| *f)
            .filter(|f| mask & f != 0)
            .collect()
    }

    pub fn name(flag: u32) -> &'static str {
        ALL.iter().find(|(f, _)| *f == flag).map_or("?", |(_, n)| n)
    }
}

/// An `inotify_simple.Event` the handler reads: `(wd, mask, cookie, name)`; the cookie is not
/// read by the handler.
#[derive(Debug, Clone, PartialEq)]
pub struct InotifyEvent {
    pub wd: i64,
    pub mask: u32,
    pub name: String,
}

/// Ours: one of notify's events as the inotify event(s) it stands for (the table in the
/// module's doc). `wds`: the watched folder of each `wd`.
pub fn to_inotify(
    event: notify::Result<notify::Event>,
    wds: &IndexMap<i64, String>,
) -> Vec<InotifyEvent> {
    use flags::*;
    let event = match event {
        Ok(event) => event,
        Err(err) => {
            log::warn!("notify: {err}");
            return Vec::new();
        }
    };
    if event.need_rescan() {
        return vec![InotifyEvent {
            wd: -1,
            mask: Q_OVERFLOW,
            name: String::new(),
        }];
    }
    let (Some(wd), Some(path)) = (event.user_data(), event.paths.first()) else {
        log::debug!("notify event without a watch or a path: {event:?}");
        return Vec::new();
    };
    let wd = wd as i64;
    let is_self = wds.get(&wd).is_some_and(|w| Path::new(w) == path);
    let name = if is_self {
        String::new()
    } else {
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let one = |mask: u32| InotifyEvent {
        wd,
        mask,
        name: name.clone(),
    };
    let ignored = InotifyEvent {
        wd,
        mask: IGNORED,
        name: String::new(),
    };
    match event.kind {
        EventKind::Create(CreateKind::Folder) => vec![one(CREATE | ISDIR)],
        EventKind::Create(_) => vec![one(CREATE)],
        EventKind::Remove(RemoveKind::Other) if event.info() == Some("unmount") => {
            vec![one(UNMOUNT), ignored]
        }
        EventKind::Remove(_) if is_self => vec![one(DELETE_SELF), ignored],
        EventKind::Remove(RemoveKind::Folder) => vec![one(DELETE | ISDIR)],
        EventKind::Remove(_) => vec![one(DELETE)],
        EventKind::Modify(ModifyKind::Name(RenameMode::From))
            if is_self && event.tracker().is_none() =>
        {
            vec![one(MOVE_SELF)]
        }
        EventKind::Modify(ModifyKind::Name(RenameMode::From)) => vec![one(MOVED_FROM)],
        EventKind::Modify(ModifyKind::Name(RenameMode::To)) => {
            let is_dir = pyos::lstat(&pyos::path_bytes(path)).is_ok_and(|st| st.is_dir());
            vec![one(if is_dir { MOVED_TO | ISDIR } else { MOVED_TO })]
        }
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => vec![one(CLOSE_WRITE)],
        _ => Vec::new(),
    }
}

type Handler = fn(&InotifyHandler, &mut State, &str, bool) -> Result<(), Error>;

struct State {
    watcher: RecommendedWatcher,
    events: Receiver<notify::Result<notify::Event>>,
    /// paths changed during one burst of events, None if not collecting
    changed_paths: Option<Vec<String>>,
    /// watch descriptors
    wds: IndexMap<i64, String>,
    next_wd: i64,
    /// Ours: the folder (device, inode) of each `wd`, for the kernel's reuse of a `wd`
    /// ([`file_id`]).
    inodes: IndexMap<i64, (u64, u64)>,
    /// Ours, outside Linux ([`Listing`]): what each watched folder held when last looked at.
    #[cfg(not(target_os = "linux"))]
    listings: IndexMap<i64, Listing>,
}

/// Ours, outside Linux: one entry of a watched folder, as `lstat` saw it.
#[cfg(not(target_os = "linux"))]
#[derive(Clone, PartialEq)]
struct Entry {
    is_dir: bool,
    size: u64,
    mtime: Option<std::time::SystemTime>,
    id: Option<(u64, u64)>,
}

/// Ours, outside Linux: a watched folder's entries by name. FSEvents (macOS) and
/// `ReadDirectoryChangesW` (Windows) have no `CLOSE_WRITE` and no paired rename, and FSEvents
/// repeats a path's earlier flags with each later change, so the kind of notify's event cannot
/// say which inotify event it stands for. What the disk holds now, against what the folder held,
/// can ([`State::translate`]).
#[cfg(not(target_os = "linux"))]
type Listing = std::collections::HashMap<String, Entry>;

#[cfg(not(target_os = "linux"))]
fn entry(path: &Path) -> Option<Entry> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    Some(Entry {
        is_dir: meta.is_dir(),
        size: meta.len(),
        mtime: meta.modified().ok(),
        id: file_id(&path.to_string_lossy()),
    })
}

#[cfg(not(target_os = "linux"))]
fn listing(dir: &str) -> Listing {
    let mut listing = Listing::new();
    if let Ok(read) = std::fs::read_dir(dir) {
        for item in read.flatten() {
            if let Some(e) = entry(&item.path()) {
                listing.insert(item.file_name().to_string_lossy().into_owned(), e);
            }
        }
    }
    listing
}

#[cfg(not(target_os = "linux"))]
impl State {
    /// Ours, outside Linux: one of notify's events as the inotify events the kernel gives for
    /// the change it reports, found by looking at the disk:
    ///
    /// | the path | inotify event |
    /// |---|---|
    /// | the watched folder, gone or another folder | `DELETE_SELF`, `IGNORED` |
    /// | new in the folder: a folder / a file | `CREATE \| ISDIR` / `CLOSE_WRITE` |
    /// | gone from the folder: a folder / a file | `DELETE \| ISDIR` (and `IGNORED` of its watch) / `DELETE` |
    /// | another one by that name (inode or kind) | as gone, then as new |
    /// | a file of another size or mtime | `CLOSE_WRITE` |
    /// | anything else | none |
    ///
    /// An event of a path that is not the folder or in it, or of a watch already ended, gives
    /// none.
    fn translate(&mut self, event: notify::Result<notify::Event>) -> Vec<InotifyEvent> {
        use flags::*;
        let event = match event {
            Ok(event) => event,
            // A watch that notify could not arm again (on Windows `ReadDirectoryChangesW`'s
            // `ERROR_ACCESS_DENIED` once its folder is gone, `windows.rs` `handle_event`) comes as
            // an error with the watch's path and `user_data`, and no `Remove`: it says which
            // path to look at, as any event does here.
            Err(err) => {
                log::warn!("notify: {err}");
                match (err.user_data(), err.paths.first()) {
                    (Some(wd), Some(path)) => {
                        notify::Event::new(EventKind::Remove(RemoveKind::Any))
                            .add_path(path.clone())
                            .set_user_data(wd)
                    }
                    _ => return Vec::new(),
                }
            }
        };
        if event.need_rescan() {
            return vec![InotifyEvent {
                wd: -1,
                mask: Q_OVERFLOW,
                name: String::new(),
            }];
        }
        let (Some(wd), Some(path)) = (event.user_data(), event.paths.first()) else {
            return Vec::new();
        };
        let wd = wd as i64;
        // A watch ended here has no listing, though `wds` keeps it until its `IGNORED` is handled.
        let (Some(dir), true) = (self.wds.get(&wd).cloned(), self.listings.contains_key(&wd))
        else {
            return Vec::new();
        };
        let ev = |wd: i64, mask: u32, name: &str| InotifyEvent {
            wd,
            mask,
            name: name.to_string(),
        };
        let mut out = Vec::new();
        if path == Path::new(&dir) {
            let now = entry(path);
            let same = now.as_ref().is_some_and(|e| {
                e.is_dir && (e.id.is_none() || e.id == self.inodes.get(&wd).copied())
            });
            if same {
                return out;
            }
            // A folder in a watched folder: its `DELETE | ISDIR` there, once, whichever of the two
            // notify tells first; the storage's own folder: `DELETE_SELF`.
            let parent = path
                .parent()
                .and_then(|p| self.live_wd(&p.to_string_lossy()));
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
            match (parent, name) {
                (Some(parent), Some(name)) => {
                    if let Some(listing) = self.listings.get_mut(&parent) {
                        listing.remove(&name);
                    }
                    out.push(ev(parent, DELETE | ISDIR, &name));
                }
                _ => out.push(ev(wd, DELETE_SELF, "")),
            }
            out.extend(self.end_watches(wd));
            return out;
        }
        if path.parent() != Some(Path::new(&dir)) {
            return out;
        }
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            return out;
        };
        let before = self.listings.get(&wd).and_then(|l| l.get(&name)).cloned();
        let now = entry(path);
        let replaced =
            matches!((&before, &now), (Some(b), Some(n)) if b.id != n.id || b.is_dir != n.is_dir);
        if let Some(b) = &before
            && (now.is_none() || replaced)
        {
            if b.is_dir {
                out.push(ev(wd, DELETE | ISDIR, &name));
                if let Some(child_wd) = self.live_wd(&join(&dir, &[&name])) {
                    out.extend(self.end_watches(child_wd));
                }
            } else {
                out.push(ev(wd, DELETE, &name));
            }
        }
        if let Some(n) = &now
            && (before.is_none() || replaced)
        {
            out.push(ev(
                wd,
                if n.is_dir {
                    CREATE | ISDIR
                } else {
                    CLOSE_WRITE
                },
                &name,
            ));
        } else if let (Some(b), Some(n)) = (&before, &now)
            && !n.is_dir
            && (b.size != n.size || b.mtime != n.mtime)
        {
            out.push(ev(wd, CLOSE_WRITE, &name));
        }
        if let Some(listing) = self.listings.get_mut(&wd) {
            match now {
                Some(n) => {
                    listing.insert(name, n);
                }
                None => {
                    listing.remove(&name);
                }
            }
        }
        out
    }

    /// The watch of a folder, while it has not ended. Paths are compared as `Path`s: on Windows
    /// notify's have `\\` where `wds` has `/`.
    fn live_wd(&self, dir: &str) -> Option<i64> {
        self.wds
            .iter()
            .find(|(w, p)| Path::new(p) == Path::new(dir) && self.listings.contains_key(*w))
            .map(|(w, _)| *w)
    }

    /// The watches of `wd`'s folder and of the folders in it end, as the kernel ends each of a
    /// folder gone (its `IGNORED`). notify stops watching them, so a folder made again by that
    /// name is watched anew.
    fn end_watches(&mut self, wd: i64) -> Vec<InotifyEvent> {
        let Some(dir) = self.wds.get(&wd).cloned() else {
            return Vec::new();
        };
        let ended: Vec<(i64, String)> = self
            .wds
            .iter()
            .filter(|(w, p)| {
                (**w == wd || Path::new(p).starts_with(&dir)) && self.listings.contains_key(*w)
            })
            .map(|(w, p)| (*w, p.clone()))
            .collect();
        let mut out = Vec::new();
        for (w, p) in ended {
            self.listings.shift_remove(&w);
            let _ = self.watcher.unwatch(Path::new(&p));
            out.push(InotifyEvent {
                wd: w,
                mask: flags::IGNORED,
                name: String::new(),
            });
        }
        out
    }
}

/// Ours: the (device, inode) `inotify_add_watch` knows a folder by. The kernel gives an object
/// that is already watched the `wd` it has: "If the filesystem object was already being watched
/// (perhaps via a different link to the same object), then the descriptor for the existing watch
/// is returned" (`inotify_add_watch(2)`), so with `inotify_simple` a folder moved inside the
/// storage keeps its `wd`, and `update_watch_dir` gives that `wd` the new path. None on Windows,
/// where std reads no such number: there a moved folder gets a new `wd`.
fn file_id(abs_path: &str) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(abs_path).ok().map(|m| (m.dev(), m.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = abs_path;
        None
    }
}

/// This handler is initialised with a Filesystem instance and using it makes sure that all its
/// storage' `trees are updated on changes on the physical storage
pub struct InotifyHandler {
    pub fs: Arc<Filesystem>,
    state: Mutex<State>,
}

impl InotifyHandler {
    // handlers for inotify file events
    const HANDLERS: [(u32, Handler); 7] = [
        (flags::CLOSE_WRITE, InotifyHandler::process_close_write),
        (flags::CREATE, InotifyHandler::process_create_dironly),
        (flags::DELETE, InotifyHandler::process_delete),
        (flags::MOVED_TO, InotifyHandler::process_create),
        (flags::MOVED_FROM, InotifyHandler::process_delete),
        (flags::DELETE_SELF, InotifyHandler::process_delete),
        (flags::MOVE_SELF, InotifyHandler::process_delete),
    ];

    pub const WATCH_FLAGS: u32 = flags::CLOSE_WRITE
        | flags::CREATE
        | flags::DELETE
        | flags::MOVED_TO
        | flags::MOVED_FROM
        | flags::DELETE_SELF
        | flags::MOVE_SELF;

    pub fn new(fs: Arc<Filesystem>) -> Result<InotifyHandler, Error> {
        let (tx, rx) = channel();
        // Outside Linux every kind: each one only says to look at the path ([`Listing`]).
        #[cfg(target_os = "linux")]
        let kinds = EventKindMask::CREATE
            | EventKindMask::REMOVE
            | EventKindMask::MODIFY_NAME
            | EventKindMask::ACCESS_CLOSE;
        #[cfg(not(target_os = "linux"))]
        let kinds = EventKindMask::ALL;
        let config = Config::default().with_event_kinds(kinds);
        let watcher = RecommendedWatcher::new(tx, config).map_err(notify_error)?;
        let handler = InotifyHandler {
            fs,
            state: Mutex::new(State {
                watcher,
                events: rx,
                changed_paths: None,
                wds: IndexMap::new(),
                next_wd: 1,
                inodes: IndexMap::new(),
                #[cfg(not(target_os = "linux"))]
                listings: IndexMap::new(),
            }),
        };
        // init storage watches
        {
            let mut state = handler.state.lock().unwrap();
            for storage in handler.fs.storages_list() {
                if storage.use_inotify
                    && let Some(path_storage) = &storage.path_storage
                {
                    handler.init_wd(&mut state, path_storage, None, true)?;
                }
            }
        }
        Ok(handler)
    }

    /// `self.wds`, a copy.
    pub fn wds(&self) -> IndexMap<i64, String> {
        self.state.lock().unwrap().wds.clone()
    }

    /// Creates the cache file
    pub fn create_cache(&self, new_path: &str) -> Result<(), Error> {
        let path_ = self.get_abs_os_path(new_path)?;
        if pyos::exists(path_.as_bytes()) {
            match get_metadata(&path_, true, None) {
                Ok(meta) => meta.save_cache().map_err(Error::Meta)?,
                Err(MetaError::UnknownGcodeFileType(_)) => {}
                Err(err) => return Err(Error::Meta(err)),
            }
        }
        Ok(())
    }

    /// When a file is deleted, the cache file is deleted
    pub fn delete_cache(&self, old_path: &str) -> Result<(), Error> {
        let (head, tail) = pyos::split(self.get_abs_os_path(old_path)?.as_bytes());
        let cache_path = format!("{}/.{}.cache", lossy(&head), lossy(&tail));
        if pyos::exists(cache_path.as_bytes()) {
            pyos::unlink(cache_path.as_bytes())?;
        }
        Ok(())
    }

    /// Check if the path is watched and if not, it's added.
    fn update_watch_dir(&self, state: &mut State, abs_paths: &[String]) -> Result<(), Error> {
        for abs_path in abs_paths {
            if !state.wds.values().any(|p| p == abs_path) {
                let id = file_id(abs_path);
                let reused = id.and_then(|id| {
                    state
                        .inodes
                        .iter()
                        .find(|(_, known)| **known == id)
                        .map(|(wd, _)| *wd)
                });
                let watch_dir_id = reused.unwrap_or(state.next_wd);
                state
                    .watcher
                    .watch_with(
                        Path::new(abs_path),
                        WatchPathConfig::new(RecursiveMode::NonRecursive)
                            .with_user_data(watch_dir_id as u64),
                    )
                    .map_err(notify_error)?;
                if reused.is_none() {
                    state.next_wd += 1;
                }
                state.wds.insert(watch_dir_id, abs_path.clone());
                if let Some(id) = id {
                    state.inodes.insert(watch_dir_id, id);
                }
                #[cfg(not(target_os = "linux"))]
                state.listings.insert(watch_dir_id, listing(abs_path));
                log::debug!("Added watch ({watch_dir_id}) for {abs_path}");
            }
        }
        Ok(())
    }

    /// Returns paths relative to the relative_point.
    ///
    /// `InotifyHandler.get_relative_paths('/tmp/r', ['/tmp/r/folder'])` is `['folder']`
    pub fn get_relative_paths(
        relative_point: &str,
        abs_paths: &[String],
    ) -> Result<Vec<String>, Error> {
        let mut relative_paths = Vec::new();
        for abs_path in abs_paths {
            if abs_path.contains(relative_point) {
                relative_paths.push(lossy(&pyos::relpath(
                    abs_path.as_bytes(),
                    Some(relative_point.as_bytes()),
                )?));
            }
        }

        if let Some(i) = relative_paths.iter().position(|p| p == ".") {
            relative_paths.remove(i);
        }

        Ok(relative_paths)
    }

    /// Update all dirs from root to bottom.
    ///
    /// Add all dirs to inotify to watcher.
    ///
    /// `abs_storage`: absolute path to storage; `node`: instance of File; `init`: wheter the
    /// function is called on init or create handler
    fn init_wd(
        &self,
        state: &mut State,
        abs_storage: &str,
        node: Option<&File>,
        init: bool,
    ) -> Result<(), Error> {
        let walk_storage = pyos::walk(abs_storage.as_bytes(), true, None, false);

        if init {
            let mut abs_paths = Vec::new();
            for item in walk_storage {
                abs_paths.push(lossy(&item?.0));
            }
            self.update_watch_dir(state, &abs_paths)?;
            Filesystem::update(&abs_paths, abs_storage, node)?;
        } else {
            let mut dir_paths = Vec::new();
            let mut file_paths = Vec::new();

            for item in walk_storage {
                let (dir_path, _, file_names) = item?;
                let dir_path = lossy(&dir_path);
                for file_name in file_names {
                    let file_name = lossy(&file_name);
                    if !file_name.starts_with('.') {
                        let file_path = join(&dir_path, &[&file_name]);
                        file_paths.push(file_path);
                    }
                }
                dir_paths.push(dir_path);
            }

            self.update_watch_dir(state, &dir_paths)?;
            Filesystem::update(&dir_paths, abs_storage, node)?;

            for file_path in file_paths {
                if !self.fs.checked_files().contains(&file_path) {
                    self.process_create(state, &file_path, false)?;
                    self.fs.checked_files.lock().unwrap().push(file_path);
                }
            }
        }
        Ok(())
    }

    /// Because we are adding inotify watch descriptors to all subdirectories, ignore all
    /// DELETE events if they are followed by any DElETE on any of their parents.
    pub fn filter_delete_events(
        wds: &IndexMap<i64, String>,
        events: Vec<InotifyEvent>,
    ) -> Result<Vec<InotifyEvent>, Error> {
        use flags::*;
        let wd_path = |wd: i64| -> Result<&String, Error> {
            wds.get(&wd).ok_or_else(|| Error::Key(wd.to_string()))
        };
        // TODO add test
        let mut ignorelist = vec![false; events.len()];
        let rev_events: Vec<InotifyEvent> = events.into_iter().rev().collect(); // we are examining from the end
        for (i, event) in rev_events.iter().enumerate() {
            log::debug!(
                "Event: {i} {event:?} ({:?}), {}",
                from_mask(event.mask)
                    .iter()
                    .map(|f| name(*f))
                    .collect::<Vec<_>>(),
                wd_path(event.wd)?
            );
            if (event.mask & ISDIR != 0 && event.mask & DELETE != 0)
                || event.mask & DELETE_SELF != 0
            {
                log::debug!(" found DEL at {i}");
                let mut event_dir = rstrip(wd_path(event.wd)?, "/").to_string();
                if event.mask & DELETE != 0 {
                    event_dir = join(&event_dir, &[&event.name]);
                }
                for (j, nxt) in rev_events[i + 1..].iter().enumerate() {
                    if (nxt.mask & DELETE != 0 && nxt.mask & ISDIR != 0)
                        || nxt.mask & DELETE_SELF != 0
                    {
                        let mut sub_event_dir = rstrip(wd_path(nxt.wd)?, "/").to_string();
                        if nxt.mask & DELETE != 0 {
                            sub_event_dir = join(&sub_event_dir, &[&nxt.name]);
                        }
                        if sub_event_dir.starts_with(&event_dir) {
                            // i+1: 0 is the DELETE_SELF/DELETE (DIR) event,
                            //  1 is the following event
                            ignorelist[i + 1 + j] = true;
                        }
                    }
                }
            }
        }
        let result: Vec<InotifyEvent> = rev_events
            .into_iter()
            .zip(ignorelist)
            .filter(|(_, i)| !i)
            .map(|(e, _)| e)
            .collect();
        Ok(result.into_iter().rev().collect())
    }

    /// Read the events: `self.inotify.read(timeout=timeout)`, through notify (see the module's
    /// doc).
    fn read(state: &mut State, timeout: i64) -> Vec<InotifyEvent> {
        let mut raw = Vec::new();
        while let Ok(event) = state.events.try_recv() {
            raw.push(event);
        }
        if raw.is_empty() && timeout != 0 {
            let first = if timeout < 0 {
                state
                    .events
                    .recv()
                    .map_err(|_| RecvTimeoutError::Disconnected)
            } else {
                state
                    .events
                    .recv_timeout(Duration::from_millis(timeout as u64))
            };
            if let Ok(event) = first {
                raw.push(event);
                while let Ok(event) = state.events.try_recv() {
                    raw.push(event);
                }
            }
        }
        let mut events = Vec::new();
        for event in raw {
            #[cfg(target_os = "linux")]
            events.extend(to_inotify(event, &state.wds));
            #[cfg(not(target_os = "linux"))]
            events.extend(state.translate(event));
        }
        events
    }

    /// Process inotify events. This picks the proper `process_$FLAG` handler method and
    /// executes it with the absolute_path of the affected file as argument.
    ///
    /// `timeout`: milliseconds, as `INotify.read`'s (the SDK's default is 0)
    pub fn call(&self, timeout: i64) -> Result<(), Error> {
        let mut state = self.state.lock().unwrap();
        let events = Self::read(&mut state, timeout);
        self.handle(&mut state, events)
    }

    /// What `__call__` does with the events it read.
    fn handle(&self, state: &mut State, events: Vec<InotifyEvent>) -> Result<(), Error> {
        let events = Self::filter_delete_events(&state.wds, events)?;
        // A lot of changes at once (e.g. copying a folder) is reported as
        // one change which asks Connect for rescan of the affected folder.
        if events.len() > RESCAN_THRESHOLD {
            state.changed_paths = Some(Vec::new());
            let result = self.process_events(state, &events);
            let changed_paths = state.changed_paths.take();
            result?;
            if let Some(changed_paths) = changed_paths
                && !changed_paths.is_empty()
            {
                self.send_rescan(state, &changed_paths)?;
            }
        } else {
            self.process_events(state, &events)?;
        }
        Ok(())
    }

    /// Process the list of inotify events
    fn process_events(&self, state: &mut State, events: &[InotifyEvent]) -> Result<(), Error> {
        for event in events {
            // Ignore hidden files .<filename>
            if event.name.starts_with('.') {
                continue;
            }
            let parent_dir = state
                .wds
                .get(&event.wd)
                .cloned()
                .ok_or_else(|| Error::Key(event.wd.to_string()))?;
            for storage in self.fs.storages_list() {
                let path_storage = storage.path_storage.as_deref().ok_or_else(|| {
                    Error::Type(
                        "startswith first arg must be str or a tuple of str, not NoneType".into(),
                    )
                })?;
                if parent_dir.starts_with(path_storage) {
                    *storage.last_updated.lock().unwrap() = time();
                }
            }
            for flag in flags::from_mask(event.mask) {
                // remove wds that are no longer needed
                if flag == flags::IGNORED {
                    if state.wds.shift_remove(&event.wd).is_none() {
                        return Err(Error::Key(event.wd.to_string()));
                    }
                    state.inodes.shift_remove(&event.wd);
                    continue;
                }
                // ignore non watched events
                if Self::WATCH_FLAGS & flag == 0 {
                    log::debug!("Ignoring {}", flags::name(flag));
                    continue;
                }

                let abs_path = join(&parent_dir, &[&event.name]);

                log::debug!("Flag: {} {abs_path} {event:?}", flags::name(flag));
                let handler = Self::HANDLERS
                    .iter()
                    .find(|(f, _)| *f == flag)
                    .map(|(_, h)| *h)
                    .expect("every flag of WATCH_FLAGS has a handler");
                log::debug!("Calling {}: {abs_path}", flags::name(flag));
                handler(self, state, &abs_path, event.mask & flags::ISDIR != 0)?;
            }
        }
        Ok(())
    }

    /// Relative path to os path.
    ///
    /// '/test/test_dir' -> '/tmp/tmpbvyl_9mr/test_dir'
    pub fn get_abs_os_path(&self, relative_path_param: &str) -> Result<String, Error> {
        let normal_path = lossy(&pyos::normpath(relative_path_param.as_bytes()));
        let mut split_path: Vec<String> = normal_path.split('/').map(str::to_string).collect();
        let Some(i) = split_path.iter().position(String::is_empty) else {
            return Err(Error::Value("list.remove(x): x not in list".into()));
        };
        split_path.remove(i);
        let Some(first) = split_path.first() else {
            return Err(Error::Index("list index out of range".into()));
        };
        let storage = self.fs.storage_dict().get(first).cloned();
        let Some(storage) = storage else {
            return Err(Error::FileNotFound("Storage doesn't exist.".into()));
        };
        let path_storage = storage.path_storage.clone().ok_or_else(|| {
            Error::Type("expected str, bytes or os.PathLike object, not NoneType".into())
        })?;
        let rest: Vec<&str> = split_path[1..].iter().map(String::as_str).collect();
        Ok(join(&path_storage, &rest))
    }

    /// Find the proper storage for the `path` in self.fs
    pub fn attach_for(&self, abs_path: &str) -> Result<Arc<Storage>, Error> {
        let mut storage_list = Vec::new();
        // exclude non-applicable storage
        for storage in self.fs.storages_list() {
            if !storage.use_inotify {
                continue;
            }
            let path_storage = storage.path_storage.as_deref().ok_or_else(|| {
                Error::Type(
                    "startswith first arg must be str or a tuple of str, not NoneType".into(),
                )
            })?;
            if !abs_path.starts_with(path_storage) {
                continue;
            }
            storage_list.push(storage);
        }
        // return the storage with the longest match
        // NOTE this is still not really deterministic. Consider the scenario
        // storage1:       /tmp/
        // storage2:       /tmp/a
        // abs_path:     /tmp/a/d
        // which one to take in this case?
        // (Counter.most_common(): the first counted of the longest)
        let mut result: Option<(Arc<Storage>, usize)> = None;
        for storage in storage_list {
            let path_storage = storage.path_storage.as_deref().unwrap_or_default();
            let overlap = common_start(rstrip(path_storage, "/"), abs_path);
            let len = overlap.chars().count();
            if result.as_ref().is_none_or(|(_, best)| len > *best) {
                result = Some((storage, len));
            }
        }
        let Some((storage, len)) = result else {
            return Err(Error::Index("list index out of range".into()));
        };
        if len == 0 {
            return Err(Error::Assertion);
        }
        Ok(storage)
    }

    /// Return the relative part of `abs_path` minus the `storage` split by self.fs.sep
    fn rel_path_parts(&self, abs_path: &str, storage: &Storage) -> Vec<String> {
        let n = storage
            .path_storage
            .as_deref()
            .unwrap_or_default()
            .chars()
            .count();
        let rel_path = slice_from(abs_path, n);
        rstrip(rel_path, &self.fs.sep)
            .split(self.fs.sep.as_str())
            .map(str::to_string)
            .collect()
    }

    /// Handles CLOSE_WRITE inotify event, it either creates or modifies a file depending on
    /// whether it exists in our tree or not
    fn process_close_write(
        &self,
        state: &mut State,
        abs_path: &str,
        is_dir: bool,
    ) -> Result<(), Error> {
        let base_storage = self.attach_for(abs_path)?;
        let parts = self.rel_path_parts(abs_path, &base_storage);
        let node = base_storage.tree.get(&parts);
        if node.is_none() {
            self.process_create(state, abs_path, is_dir)
        } else {
            self.process_modify(state, abs_path, is_dir)
        }
    }

    /// Handles CREATE inotify event, but only for directories we expect to wait for a
    /// CLOSE_WRITE event from files
    fn process_create_dironly(
        &self,
        state: &mut State,
        abs_path: &str,
        is_dir: bool,
    ) -> Result<(), Error> {
        self.process_create_(state, abs_path, is_dir, true)
    }

    /// Handles CREATE inotify event for all types of files
    fn process_create(&self, state: &mut State, abs_path: &str, is_dir: bool) -> Result<(), Error> {
        self.process_create_(state, abs_path, is_dir, false)
    }

    /// Handle CREATE inotify signal by creating the file/folder determined by `abs_path`.
    /// `is_dir` is set, if the event was generated for a folder.
    fn process_create_(
        &self,
        state: &mut State,
        abs_path: &str,
        is_dir: bool,
        dir_only: bool,
    ) -> Result<(), Error> {
        if is_dir || !dir_only {
            let base_storage = self.attach_for(abs_path)?;
            let mut parts = self.rel_path_parts(abs_path, &base_storage);
            let name = parts.pop().unwrap_or_default();
            let parent = base_storage.tree.get(&parts);
            let Some(parent) = parent else {
                return Ok(());
            };
            let node = parent.add(&name, is_dir)?;
            node.fill_attrs(abs_path)?;

            if is_dir {
                // add inotify watch
                self.init_wd(state, abs_path, Some(&node), false)?; // add inotify watch
            } else if !dir_only {
                self.create_cache(&node.abs_path(&base_storage.storage)?)?;
            }

            self.send_file_changed(
                state,
                None,
                Some(&node.abs_path(&base_storage.storage)?),
                Some(&node),
                Some(base_storage.get_space_info()?),
                false,
            )?;
        }
        Ok(())
    }

    /// Handle DELETE inotify signal by deleting the node indicated by `abs_path`.
    fn process_delete(
        &self,
        state: &mut State,
        abs_path: &str,
        _is_dir: bool,
    ) -> Result<(), Error> {
        let base_storage = self.attach_for(abs_path)?;
        let parts = self.rel_path_parts(abs_path, &base_storage);
        // top level dir (storage.tree) was deleted or detached
        if Some(abs_path) == base_storage.path_storage.as_deref() {
            let node = base_storage.tree.clone();
            node.clear_children();
            node.set_attrs(Map::new());
            let path_ = node.abs_path(&base_storage.storage)?;
            self.delete_cache(&path_)?;
            // TODO: Why is this there?
            // self.create_cache(path_)
            self.send_file_changed(
                state,
                Some(&path_),
                Some(&path_),
                Some(&node),
                Some(base_storage.get_space_info()?),
                false,
            )?;
        } else {
            // some watched folder other than top level was deleted
            let node = base_storage.tree.get(&parts);
            let node = node.ok_or_else(|| none_attribute("delete"))?;
            node.delete()?;
            let path_ = node.abs_path(&base_storage.storage)?;
            self.delete_cache(&path_)?;
            self.send_file_changed(
                state,
                Some(&path_),
                None,
                None,
                Some(base_storage.get_space_info()?),
                false,
            )?;
        }
        Ok(())
    }

    /// Process MODIFY inotify signal by updating the attributes for a file indicated by
    /// `abs_path`.
    fn process_modify(
        &self,
        state: &mut State,
        abs_path: &str,
        _is_dir: bool,
    ) -> Result<(), Error> {
        let base_storage = self.attach_for(abs_path)?;
        let parts = self.rel_path_parts(abs_path, &base_storage);
        let node = base_storage.tree.get(&parts);
        let node = node.ok_or_else(|| none_attribute("fill_attrs"))?;
        node.fill_attrs(abs_path)?;
        let path_ = node.abs_path(&base_storage.storage)?;
        self.send_file_changed(
            state,
            Some(&path_),
            Some(&path_),
            Some(&node),
            Some(base_storage.get_space_info()?),
            false,
        )
    }

    /// If self.fs.events is set, put FIlE_CHANGED event to event queue.
    ///
    /// `space_info`: `free_space`, `total_space` and `system_space` of the storage, which are
    /// sent along with the event; `rescan`: Tell Connect to drop everything it knows about
    /// `new_path` and to synchronize it again. Raises ValueError if both old_path and new_path
    /// are not set
    fn send_file_changed(
        &self,
        state: &mut State,
        old_path: Option<&str>,
        new_path: Option<&str>,
        file: Option<&File>,
        space_info: Option<Map<String, Value>>,
        rescan: bool,
    ) -> Result<(), Error> {
        let old_set = old_path.filter(|p| !p.is_empty());
        let new_set = new_path.filter(|p| !p.is_empty());
        if old_set.is_none() && new_set.is_none() {
            let msg = "At least one of (old_path, new_path) must be set";
            return Err(Error::Value(msg.into()));
        }
        if let Some(changed_paths) = &mut state.changed_paths {
            // collecting a burst of changes
            changed_paths.push(new_set.or(old_set).unwrap_or_default().to_string());
            return Ok(());
        }
        let mut data = Map::new();
        data.insert("old_path".into(), json!(old_path));
        data.insert("new_path".into(), json!(new_path));
        if let Some(file) = file {
            // Connect asks for the content of a folder itself
            data.insert("file".into(), Value::Object(file.to_dict(false)));
        }
        if let Some(space_info) = space_info {
            for (k, v) in space_info {
                data.insert(k, v);
            }
        }
        if rescan {
            data.insert("rescan".into(), json!(true));
        }
        self.fs.connect_event(Event::FileChanged, data);
        Ok(())
    }

    /// Send FILE_CHANGED with `rescan` instead of many events.
    ///
    /// There is one event for each storage with changes, because every path in the protocol
    /// has to start with a mountpoint. The event refers to the nearest folder containing all
    /// the changes in the storage.
    fn send_rescan(&self, state: &mut State, changed_paths: &[String]) -> Result<(), Error> {
        let sep = self.fs.sep.as_str();
        let mut by_storage: IndexMap<String, Vec<String>> = IndexMap::new();
        for changed in changed_paths {
            let folder = match changed.rsplit_once(sep) {
                Some((head, _)) => head,
                None => changed.as_str(),
            };
            let folder = if folder.is_empty() { sep } else { folder };
            let storage_name = strip(folder, sep)
                .split(sep)
                .next()
                .unwrap_or_default()
                .to_string();
            by_storage
                .entry(storage_name)
                .or_default()
                .push(folder.to_string());
        }

        for (storage_name, folders) in by_storage {
            let storage = self.fs.storage_dict().get(&storage_name).cloned();
            let Some(storage) = storage else {
                // the storage is gone, nothing to sync
                continue;
            };
            let folders: Vec<&[u8]> = folders.iter().map(|f| f.as_bytes()).collect();
            let common = lossy(&pyos::commonpath(&folders)?);
            self.send_file_changed(
                state,
                None,
                Some(&common),
                None,
                Some(storage.get_space_info()?),
                true,
            )?;
        }
        Ok(())
    }
}

/// An error of notify adding a watch, as `inotify_simple`'s `OSError(errno, strerror)`.
fn notify_error(err: notify::Error) -> Error {
    match &err.kind {
        notify::ErrorKind::Io(io) => Error::Os(pyos::Error::Os(OsError::from_io(io))),
        notify::ErrorKind::PathNotFound => {
            Error::Os(pyos::Error::Os(OsError::from_errno(libc::ENOENT, None)))
        }
        _ => Error::Os(pyos::Error::Os(OsError {
            errno: None,
            strerror: err.to_string(),
            filename: None::<Filename>,
            filename2: None,
            winerror: None,
        })),
    }
}

/// `tests/test_files.py` and `tests/test_files_helpers.py` at `c0d7d233`, test by test. The
/// `fs_from_dir` fixture mocks `stat`, `abspath` and `walk`; here it is a real folder with the
/// same files, sizes, times and read-only bits. `TestINotify` needs inotify: Linux only. Its
/// `handler()` follows a short wait, for notify's thread (see the module's doc). The SDK's test
/// G-code: `FILES_SDK_DIR` (the SDK's checkout), else `reference/Prusa-Connect-SDK-Printer`;
/// without it, the two tests that copy it say so and pass.
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    struct TempDir(PathBuf);

    impl TempDir {
        /// A new, empty folder of its own in the system's temporary folder.
        fn new() -> TempDir {
            use std::sync::atomic::{AtomicU32, Ordering};
            static COUNT: AtomicU32 = AtomicU32::new(0);
            loop {
                let n = COUNT.fetch_add(1, Ordering::Relaxed);
                let dir =
                    std::env::temp_dir().join(format!("files-test-{}-{n}", std::process::id()));
                match std::fs::create_dir(&dir) {
                    Ok(()) => return TempDir(dir),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(e) => panic!("{e}"),
                }
            }
        }

        /// The folder as `pyos` writes a path (on Windows with `/`).
        fn path(&self) -> String {
            String::from_utf8(pyos::path_bytes(&self.0)).unwrap()
        }
    }

    fn set_readonly(p: &Path, readonly: bool) -> std::io::Result<()> {
        let mut perm = std::fs::metadata(p)?.permissions();
        #[cfg(unix)]
        {
            // `set_readonly(false)` gives write to all; the owner's is enough here.
            use std::os::unix::fs::PermissionsExt;
            let mode = perm.mode();
            perm.set_mode(if readonly {
                mode & !0o222
            } else {
                mode | 0o200
            });
        }
        #[cfg(not(unix))]
        perm.set_readonly(readonly);
        std::fs::set_permissions(p, perm)
    }

    fn make_writable(p: &Path) {
        if let Ok(meta) = std::fs::symlink_metadata(p) {
            if meta.file_type().is_symlink() {
                return;
            }
            let _ = set_readonly(p, false);
            if meta.is_dir()
                && let Ok(entries) = std::fs::read_dir(p)
            {
                for e in entries.flatten() {
                    make_writable(&e.path());
                }
            }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            make_writable(&self.0);
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Create file tree in memory.
    fn nodes() -> File {
        let root = File::node("storage", true);
        let a = root.add("a", true).unwrap();
        a.add("1.gcode", false).unwrap();
        a.add("b", true).unwrap();
        let h = root.add(".h", true).unwrap();
        h.add("2.gcode", false).unwrap();
        h.add(".hidden.gcode", false).unwrap();
        let c = a.add("c", true).unwrap();
        c.add("2.sl1", false).unwrap();
        c.add("3.txt", false).unwrap();
        root
    }

    /// The mocked `stat` of `fs_from_dir`: size 3044, mtime 1596120005; its paths do not exist,
    /// so `access(.., W_OK)` is false.
    const MTIME: i64 = 1596120005;

    fn write_file(p: &Path, size: usize) {
        std::fs::write(p, vec![b'x'; size]).unwrap();
    }

    /// A frozen folder's `read_only`: true, as the mock's `access`; on Windows false, where
    /// CPython's `os.access(.., W_OK)` is true for every folder (`attr & FILE_ATTRIBUTE_DIRECTORY`).
    const FOLDER_READ_ONLY: bool = cfg!(not(windows));

    /// The mocked `stat`'s time, and read only.
    fn freeze(p: &Path) {
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(MTIME as u64);
        let times = std::fs::FileTimes::new().set_accessed(t).set_modified(t);
        let mut open = std::fs::OpenOptions::new();
        open.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // FILE_WRITE_ATTRIBUTES, to set the times; FILE_FLAG_BACKUP_SEMANTICS, to open a
            // folder
            open.access_mode(0x100).custom_flags(0x0200_0000);
        }
        open.open(p).unwrap().set_times(times).unwrap();
        set_readonly(p, true).unwrap();
    }

    /// A folder as the mocked `walk` lists it (dirs, then files of each folder), frozen
    /// bottom-up.
    fn tree_on_disk(root: &Path, dirs: &[&str], files: &[&str]) {
        for d in dirs {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        for f in files {
            write_file(&root.join(f), 3044);
        }
        let mut all: Vec<PathBuf> = dirs.iter().chain(files).map(|p| root.join(p)).collect();
        all.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
        for p in all {
            freeze(&p);
        }
        freeze(root);
    }

    fn fs_from_dir() -> (TempDir, Filesystem) {
        let tmp = TempDir::new();
        tree_on_disk(&tmp.0, &["b", "c"], &["1.gcode", "c/2.sl1", "c/3.txt"]);
        let fs = Filesystem::new("/", None);
        fs.from_dir(&tmp.path(), "a", StorageType::Local, None)
            .unwrap();
        (tmp, fs)
    }

    /// The fixture `fs`: `/tmp`, which `test_get_space_info` needs to exist; on Windows the
    /// temp folder in its place.
    fn fs() -> Filesystem {
        #[cfg(unix)]
        let tmp = "/tmp".to_string();
        #[cfg(not(unix))]
        let tmp = std::env::temp_dir().to_string_lossy().into_owned();
        let fs = Filesystem::new("/", None);
        fs.attach("storage", nodes(), &tmp, false, StorageType::Local, None)
            .unwrap();
        fs
    }

    // TestFile: Test the methods of the File class

    #[test]
    fn test_add() {
        let root = File::node("root", true);
        assert!(root.children().is_empty());
        assert!(root.is_dir());
        root.add("child", false).unwrap();
        assert!(root.contains("child"));
        assert!(!root.child("child").unwrap().is_dir());
    }

    #[test]
    fn test_add_hidden_file() {
        let root = File::node("root", true);
        assert!(root.children().is_empty());
        assert!(root.is_dir());
        root.add(".child", false).unwrap();
        assert!(!root.contains(".child"));
    }

    #[test]
    fn test_add_to_file() {
        let file = File::node("file", false);
        assert!(!file.is_dir());
        assert!(matches!(
            file.add("another_file", false),
            Err(Error::Value(_))
        ));
    }

    /// Make sure that adding twice the same name adds it only once. Any further add() with the
    /// name name overwrite the previous file.
    #[test]
    fn test_add_multiple() {
        let root = File::node("root", true);
        assert_eq!(root.children().len(), 0);
        root.add("a", true).unwrap();
        assert!(root.child("a").unwrap().is_dir());
        root.add("a", false).unwrap();
        assert!(!root.child("a").unwrap().is_dir());
        assert!(root.child("a").is_some());
        assert_eq!(root.children().len(), 1);
    }

    #[test]
    fn test_get() {
        let nodes = nodes();
        // 1st level
        assert!(nodes.get(&["a"]).is_some());
        // deeper
        assert!(nodes.get(&["a", "c", "2.sl1"]).is_some());
    }

    // test_get_str: `get` with a `str` cannot be written in Rust (a `&[S]`).

    #[test]
    fn test_delete() {
        let nodes = nodes();
        nodes.get(&["a", "c"]).unwrap().delete().unwrap();
        assert!(!nodes.get(&["a"]).unwrap().contains("c"));
    }

    #[test]
    fn test_getitem() {
        let nodes = nodes();
        assert!(nodes.child("not found").is_none());
        assert!(nodes.get(&["a"]).unwrap().child("1.gcode").is_some());
        assert!(nodes.get(&["a", "c"]).unwrap().child("2.sl1").is_some());
        assert!(nodes.get(&["a", "c"]).unwrap().child("3.txt").is_some());
    }

    #[test]
    fn test_getattr() {
        let nodes = nodes();
        let a = nodes.child("a").unwrap();
        assert_eq!(a.child("b").unwrap().name(), "b");
        assert_eq!(a.child("c").unwrap().name(), "c");
    }

    #[test]
    fn test_parent() {
        let nodes = nodes();
        let a = nodes.child("a").unwrap();
        let c = a.child("c").unwrap();
        assert!(c.parent().unwrap().unwrap().ptr_eq(&a));
    }

    #[test]
    fn test_size() {
        let (_tmp, fs) = fs_from_dir();
        assert_eq!(fs.get("/a").unwrap().size(), 9132);
        assert_eq!(fs.get("/a/c").unwrap().size(), 6088);
        assert_eq!(fs.get("/a/c/2.sl1").unwrap().size(), 3044);
        assert_eq!(fs.get("/a/b").unwrap().size(), 0);
        assert_eq!(fs.get("/a/1.gcode").unwrap().size(), 3044);
    }

    #[test]
    fn test_m_time_ts() {
        let (_tmp, fs) = fs_from_dir();
        assert_eq!(fs.get("/a").unwrap().attrs()["m_timestamp"], json!(MTIME));
    }

    #[test]
    fn test_to_dict() {
        let (_tmp, fs) = fs_from_dir();
        let res = fs.get("/a").unwrap().to_dict(true);
        // walk's order is scandir's (the mock's: b, c; 1.gcode)
        let mut children = res["children"].as_array().unwrap().clone();
        children.sort_by_key(|c| c["name"].as_str().unwrap().to_string());
        let mut res = res.clone();
        res.insert("children".into(), Value::Array(children));
        assert_eq!(
            Value::Object(res),
            json!({
                "type": "FOLDER",
                "name": "a",
                "read_only": FOLDER_READ_ONLY,
                "m_timestamp": 1596120005,
                "size": 9132,
                "children": [{
                    "type": "PRINT_FILE",
                    "name": "1.gcode",
                    "read_only": true,
                    "m_timestamp": 1596120005,
                    "size": 3044,
                }, {
                    "type": "FOLDER",
                    "name": "b",
                    "read_only": FOLDER_READ_ONLY,
                    "m_timestamp": 1596120005,
                    "size": 0,
                }, {
                    "type": "FOLDER",
                    "name": "c",
                    "read_only": FOLDER_READ_ONLY,
                    "m_timestamp": 1596120005,
                    "size": 6088,
                }],
            })
        );
    }

    #[test]
    fn test_to_dict_no_children() {
        let (_tmp, fs) = fs_from_dir();
        let res = fs.get("/a").unwrap().to_dict(false);
        assert_eq!(
            Value::Object(res),
            json!({
                "type": "FOLDER",
                "name": "a",
                "read_only": FOLDER_READ_ONLY,
                "m_timestamp": 1596120005,
                "size": 9132,
            })
        );
    }

    #[test]
    fn test_contains() {
        assert!(nodes().contains("a"));
    }

    #[test]
    fn test_str() {
        let d = File::node("folder", true);
        let f = File::node("filename", false);
        assert_eq!(d.to_string(), "folder");
        assert_eq!(f.to_string(), "filename");
    }

    #[test]
    fn test_abs_parts() {
        let nodes = nodes();
        let node = nodes.get(&["a", "c", "2.sl1"]).unwrap();
        assert_eq!(node.abs_parts().unwrap(), ["a", "c", "2.sl1"]);
    }

    #[test]
    fn test_abs_path() {
        let nodes = nodes();
        let node = nodes.get(&["a", "c", "2.sl1"]).unwrap();
        assert_eq!(node.abs_path("/test").unwrap(), "/test/a/c/2.sl1");
    }

    // TestFilesystem: Test Filesystem class interface.

    #[test]
    fn test_storage() {
        let fs = fs();
        assert_eq!(fs.storage_dict().len(), 1);
        assert!(fs.storage_dict().contains_key("storage"));
    }

    #[test]
    fn test_attach_empty() {
        let fs = fs();
        let r = fs.attach("", nodes(), "", true, StorageType::Local, None);
        assert!(matches!(r, Err(Error::InvalidStorage(_))));
    }

    #[test]
    fn test_storage_contains_sep() {
        let fs = fs();
        let r = fs.attach("/b", nodes(), "", true, StorageType::Local, None);
        assert!(matches!(r, Err(Error::InvalidStorage(_))));
    }

    #[test]
    fn test_storage_already_used() {
        let fs = fs();
        let r = fs.attach("storage", nodes(), "", true, StorageType::Local, None);
        assert!(matches!(r, Err(Error::InvalidStorage(_))));
    }

    #[test]
    fn test_get_space_info() {
        let fs = fs();
        let info = fs.storage_dict()["storage"].get_space_info().unwrap();
        assert!(info["free_space"].as_u64().unwrap() > 0);
        assert!(info["total_space"].as_u64().unwrap() > 0);
    }

    #[test]
    fn test_detach() {
        let fs = fs();
        fs.detach("storage").unwrap();
        assert_eq!(fs.storage_dict().len(), 0);
    }

    #[test]
    fn test_detach_invalid_storage() {
        let fs = Filesystem::new("/", None);
        assert!(matches!(
            fs.detach("doesn-not-exist"),
            Err(Error::InvalidStorage(_))
        ));
    }

    #[test]
    fn test_from_dir() {
        let (_tmp, fs_from_dir) = fs_from_dir();
        let b = fs_from_dir.get("/a/b").unwrap();
        assert!(b.children().is_empty());
        assert!(b.is_dir());
        assert!(fs_from_dir.get("/a/1.gcode").is_some());
        assert!(fs_from_dir.get("/a/c/2.sl1").is_some());
        assert!(fs_from_dir.get("/a/c/3.txt").is_some());

        // test root node
        assert!(fs_from_dir.get("/a").unwrap().is_dir());
        assert_eq!(fs_from_dir.get("/a/").unwrap().name(), "a");
        assert!(fs_from_dir.get("/a/").unwrap().parent().unwrap().is_none());
    }

    #[test]
    fn test_from_hidden_dir() {
        let (_tmp, fs_from_dir) = fs_from_dir();
        // `h.is_dir` of None is an AttributeError: `get` gives None
        assert!(fs_from_dir.get("/.h").is_none());
        assert!(fs_from_dir.get("/.h/2.gcode").is_none());
        assert!(fs_from_dir.get("/.h/.hidden.gcode").is_none());
    }

    #[test]
    fn test_from_hidden_subdir() {
        let tmp = TempDir::new();
        tree_on_disk(
            &tmp.0,
            &["b", "b/.c"],
            &["1.gcode", "b/.c/2.sl1", "b/.c/3.txt"],
        );
        let fs = Filesystem::new("/", None);
        fs.from_dir(&tmp.path(), "a", StorageType::Local, None)
            .unwrap();
        assert!(fs.get("/a/b/.c/2.sl1").is_none());
        assert!(fs.get("/a/b/.c").is_none());
    }

    #[test]
    fn test_from_hidden_subsubdir() {
        let tmp = TempDir::new();
        tree_on_disk(&tmp.0, &["b", "b/.c", "b/.c/d"], &["b/.c/d/1.sl1"]);
        let fs = Filesystem::new("/", None);
        fs.from_dir(&tmp.path(), "a", StorageType::Local, None)
            .unwrap();
        assert!(fs.get("/a/b/.c").is_none());
        assert!(fs.get("/a/b/.c/d").is_none());
        assert!(fs.get("/a/b/.c/d/1.sl1").is_none());
    }

    #[test]
    fn test_get_root() {
        let fs = fs();
        let a = fs.get("storage").unwrap();
        assert_eq!(a.name(), "storage");
        assert!(a.is_dir());
        assert_eq!(a.children().len(), 1);
    }

    /// Test walking along the file tree using get()
    #[test]
    fn test_get_deep() {
        let fs = fs();
        assert!(fs.get("storage/a/1.gcode").is_some());
        assert!(fs.get("storage/a/c/2.sl1").is_some());
        assert!(fs.get("storage/a/c/3.txt").is_some());
    }

    // test_files_helpers.py

    /// The longest common beginning of two strings
    #[test]
    fn test_common_start() {
        assert_eq!(common_start("/a/b/c", "/a/b/d"), "/a/b/");
        assert_eq!(common_start("abc", "xyz"), "");
    }

    /// A file is deleted
    #[test]
    fn test_delete_file() {
        let tmp = TempDir::new();
        let path = tmp.0.join("file.txt");
        std::fs::write(&path, "x").unwrap();
        delete(path.to_str().unwrap(), false, false).unwrap();
        assert!(!path.exists());
    }

    /// An empty directory is deleted, a non-empty one only when forced
    #[test]
    fn test_delete_directory() {
        let tmp = TempDir::new();
        let empty = tmp.0.join("empty");
        std::fs::create_dir(&empty).unwrap();
        delete(empty.to_str().unwrap(), true, false).unwrap();
        assert!(!empty.exists());

        let full = tmp.0.join("full");
        std::fs::create_dir(&full).unwrap();
        std::fs::write(full.join("file.txt"), "x").unwrap();
        let err = delete(full.to_str().unwrap(), true, false).unwrap_err();
        assert!(err.is_os_error());
        delete(full.to_str().unwrap(), true, true).unwrap();
        assert!(!full.exists());
    }

    /// Deleting something which does not exist fails
    #[test]
    fn test_delete_missing() {
        let tmp = TempDir::new();
        let missing = tmp.0.join("missing");
        assert!(matches!(
            delete(missing.to_str().unwrap(), false, false),
            Err(Error::FileNotFound(_))
        ));
    }

    // TestINotify: Test events from Inotify class.

    type Queue = Arc<Mutex<VecDeque<(Event, Source, Kwargs)>>>;

    struct InotifyFixture {
        path: String,
        handler: InotifyHandler,
        fs: Arc<Filesystem>,
        queue: Queue,
        _tmp: TempDir,
    }

    impl InotifyFixture {
        /// `inotify.handler()`, after notify's thread has passed the events on: where the
        /// reference's are in the kernel's queue at once, notify's may come later (FSEvents under
        /// load), so it waits up to 2 s for the first (`call`'s timeout), then takes what follows.
        fn handler(&self) {
            std::thread::sleep(Duration::from_millis(200));
            self.handler.call(2000).unwrap();
            std::thread::sleep(Duration::from_millis(200));
            self.handler.call(0).unwrap();
        }

        fn clear(&self) {
            self.queue.lock().unwrap().clear();
        }

        fn get_nowait(&self) -> (Event, Source, Kwargs) {
            self.queue.lock().unwrap().pop_front().expect("queue.Empty")
        }

        /// The last event in the queue.
        fn last(&self) -> (Event, Source, Kwargs) {
            self.queue.lock().unwrap().pop_back().expect("an event")
        }
    }

    /// Create and cleanup the same structure as in `nodes` in a temporary directory.
    fn inotify() -> InotifyFixture {
        fn create_on_storage(root_dir: &str, node: &File) {
            let mut parts = node.abs_parts().unwrap();
            parts.insert(0, root_dir.to_string());
            let path = parts.join("/");
            if node.is_dir() {
                if !Path::new(&path).exists() {
                    // root node is already created
                    std::fs::create_dir(&path).unwrap();
                }
            } else {
                std::fs::write(&path, "").unwrap(); // create file
            }
            for n in node.children() {
                create_on_storage(root_dir, &n);
            }
        }

        let queue: Queue = Arc::new(Mutex::new(VecDeque::new()));
        let q = queue.clone();
        let event_cb: EventCallback = Arc::new(move |event, source, _ts, _id, kwargs| {
            q.lock().unwrap().push_back((event, source, kwargs));
        });

        let tmp = TempDir::new();
        let nodes = nodes();
        create_on_storage(&tmp.path(), &nodes);

        // attach storage:$tmp_dir as Filesystem:/test
        let fs = Arc::new(Filesystem::new("/", Some(event_cb)));
        fs.from_dir(&tmp.path(), "test", StorageType::Local, None)
            .unwrap();
        // Test event in queue
        let (event, source, data) = queue.lock().unwrap().pop_front().unwrap();
        assert_eq!(event, Event::MediumInserted);
        assert_eq!(source, Source::Wui);
        assert_eq!(data["mountpoint"], "/test");
        assert_eq!(data["name"], "test");
        assert_eq!(data["is_sfn"], false);
        assert!(!data.contains_key("files"));
        assert!(!data.contains_key("root"));
        let handler = InotifyHandler::new(fs.clone()).unwrap();

        InotifyFixture {
            path: tmp.path(),
            handler,
            fs,
            queue,
            _tmp: tmp,
        }
    }

    fn sdk_gcodes() -> Option<PathBuf> {
        let dir = std::env::var("FILES_SDK_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../reference/Prusa-Connect-SDK-Printer-c0d7d233")
            });
        let gcodes = dir.join("tests/gcodes/metadata");
        if gcodes.is_dir() {
            Some(gcodes)
        } else {
            // CI's job `references` sets REFERENCES_REQUIRED: there a skip is a failure.
            assert!(
                std::env::var_os("REFERENCES_REQUIRED").is_none(),
                "REFERENCES_REQUIRED: skipped: no SDK checkout (FILES_SDK_DIR) with tests/gcodes/metadata"
            );
            eprintln!("skipped: no SDK checkout (FILES_SDK_DIR) with tests/gcodes/metadata");
            None
        }
    }

    /// Test that creating a file is reflected in the Filesystem and that also Connect is
    /// notified by the means of an Event
    #[test]
    fn test_create_file() {
        let inotify = inotify();
        inotify.clear();

        let p = join(&inotify.path, &["simple.gcode"]);
        std::fs::write(&p, "").unwrap();
        inotify.handler();
        assert!(inotify.fs.get("/test/simple.gcode").is_some());
        assert!(inotify.fs.get("/test/does-not-exit.gcode").is_none());
        // check event to Connect
        let (event, source, data) = inotify.get_nowait();
        assert_eq!(event, Event::FileChanged);
        assert_eq!(source, Source::Wui);
        assert_eq!(data["file"]["m_timestamp"].to_string().len(), 10);
        assert_eq!(data["file"]["name"], "simple.gcode");
        assert_eq!(data["file"]["read_only"], false);
        assert_eq!(data["file"]["type"], "PRINT_FILE");
        assert_eq!(data["new_path"], "/test/simple.gcode");
        assert_eq!(data["old_path"], Value::Null);
    }

    /// Same as CREATE_file but this time a folder is used.
    #[test]
    fn test_create_dir() {
        let inotify = inotify();
        inotify.clear();

        let p = join(&inotify.path, &["folder"]);
        std::fs::create_dir(&p).unwrap();
        inotify.handler();
        let d = inotify.fs.get("/test/folder").unwrap();
        assert!(d.is_dir());

        // check event to Connect
        let (event, source, data) = inotify.get_nowait();
        assert_eq!(event, Event::FileChanged);
        assert_eq!(source, Source::Wui);
        assert_eq!(data["file"]["m_timestamp"].to_string().len(), 10);
        assert_eq!(data["file"]["name"], "folder");
        assert_eq!(data["file"]["read_only"], false);
        assert_eq!(data["file"]["type"], "FOLDER");
        assert_eq!(data["new_path"], "/test/folder");
        assert_eq!(data["old_path"], Value::Null);

        // test that a inotify watch has also been installed for the
        //  newly added dir
        let file_path = join(&p, &["file.gcode"]);
        std::fs::write(&file_path, "").unwrap();
        inotify.handler();
        assert!(inotify.fs.get("/test/folder/file.gcode").is_some());
    }

    /// Test deleting a file by creating it first, then deleting it and requesting it from the
    /// Filesystem. Also test that other file(s) were not affected
    #[test]
    fn test_delete_file_event() {
        let inotify = inotify();
        let p = join(&inotify.path, &["simple.gcode"]);
        std::fs::write(&p, "").unwrap();
        inotify.handler();
        assert!(inotify.fs.get("/test/simple.gcode").is_some());

        std::fs::remove_file(&p).unwrap();
        inotify.handler();
        assert!(inotify.fs.get("/test/simple.gcode").is_none());
        assert!(inotify.fs.get("/test/a/c/2.sl1").is_some());

        // check event to Connect
        let (event, source, data) = inotify.last();
        assert_eq!(event, Event::FileChanged);
        assert_eq!(source, Source::Wui);
        assert_eq!(data["old_path"], "/test/simple.gcode", "{data:?}");
        assert_eq!(data["new_path"], Value::Null);
    }

    /// Test creating, deleting and requesting .<filename>.cache file
    #[test]
    fn test_create_and_delete_cache_file() {
        let Some(gcodes_dir) = sdk_gcodes() else {
            return;
        };
        let inotify = inotify();
        // Create the gcode file
        let p = gcodes_dir.join("fdn_filename.gcode");
        let gcode_file = join(&inotify.path, &["fdn_filename.gcode"]);
        std::fs::copy(&p, &gcode_file).unwrap();
        let cache = maker_connect_gcode_metadata::metadata::MetaData::fdm(&gcode_file).cache_name();

        // Cache not in fs, nor inotify tree
        assert!(!Path::new(&cache).exists());
        assert!(inotify.fs.get(&cache).is_none());

        // Create cache
        inotify.handler();

        // Cache in fs, not in inotify tree
        assert!(Path::new(&cache).exists());
        assert!(inotify.fs.get(&cache).is_none());

        // Delete the gcode file
        std::fs::remove_file(&gcode_file).unwrap();
        // Delete cache
        inotify.handler();

        // Cache not in fs, nor inotify tree
        assert!(!Path::new(&cache).exists());
        assert!(inotify.fs.get(&cache).is_none());
    }

    /// Test that inotify handler ignores hidden .<filename> files
    #[test]
    fn test_inotify_ignore_hidden_files() {
        let Some(gcodes_dir) = sdk_gcodes() else {
            return;
        };
        let inotify = inotify();
        // Create the gcode files
        let p = gcodes_dir.join(".hidden_fdn_filename.gcode");
        std::fs::copy(
            &p,
            Path::new(&inotify.path).join(".hidden_fdn_filename.gcode"),
        )
        .unwrap();

        // Get .<filename>.cache name
        let meta =
            maker_connect_gcode_metadata::metadata::MetaData::fdm(p.to_str().unwrap()).cache_name();

        // Try to create cache
        inotify.handler();
        assert!(!Path::new(&meta).exists());
    }

    /// Test that after deleting a folder it is removed from the Filesystem.
    #[test]
    fn test_delete_dir() {
        let inotify = inotify();
        inotify.clear();

        let node = inotify.fs.get("/test/a/b").unwrap();
        let path = node.abs_path(&inotify.path).unwrap();
        assert!(inotify.fs.get("/test/a/b").is_some());
        std::fs::remove_dir(&path).unwrap();
        inotify.handler();
        assert!(inotify.fs.get("/test/a/b").is_none());

        // check event to Connect
        let (event, source, data) = inotify.get_nowait();
        assert_eq!(event, Event::FileChanged);
        assert_eq!(source, Source::Wui);
        assert_eq!(data["old_path"], "/test/a/b");
        assert_eq!(data["new_path"], Value::Null);
    }

    /// Test removing the root of a storage `storage.path_storage` in a `Filesystem`
    #[test]
    fn test_delete_root_dir() {
        let inotify = inotify();
        std::fs::remove_dir_all(&inotify.path).unwrap();
        inotify.handler();
        assert!(inotify.fs.get("/test/a/1.gcode").is_none());
        assert!(inotify.fs.get("/test/a/c").is_none());

        // check event to Connect
        let (event, source, data) = inotify.last();
        assert_eq!(event, Event::FileChanged);
        assert_eq!(source, Source::Wui);
        assert_eq!(data["old_path"], "/test/");
        assert_eq!(data["new_path"], "/test/");
        assert_eq!(data["file"]["type"], "FOLDER");
        assert!(
            !data["file"]
                .as_object()
                .unwrap()
                .contains_key("m_timestamp")
        );
        assert_eq!(data["file"]["name"], "test");
    }

    /// Create a file and move it to a different folder
    #[test]
    fn test_move_file() {
        let inotify = inotify();
        let src = inotify.fs.get("/test/a/1.gcode").unwrap();
        let src_path = src.abs_path(&inotify.path).unwrap();

        let dst = inotify.fs.get("/test/a/c").unwrap();
        let dst_path = dst.abs_path(&inotify.path).unwrap();

        std::fs::rename(&src_path, join(&dst_path, &["1.gcode"])).unwrap();
        inotify.handler();
        assert!(inotify.fs.get("/test/a/c/1.gcode").is_some());

        // check event to Connect
        let (event, source, data) = inotify.last();
        assert_eq!(event, Event::FileChanged);
        assert_eq!(source, Source::Wui);
        assert_eq!(data["old_path"], Value::Null);
        assert_eq!(data["file"]["name"], "1.gcode");
        assert_eq!(data["new_path"], "/test/a/c/1.gcode");
    }

    /// Write into a file and make sure that the change is reflected
    #[test]
    fn test_modify_file() {
        let inotify = inotify();
        let node = inotify.fs.get("/test/a/1.gcode").unwrap();
        assert_eq!(node.attrs()["size"], 0);
        assert_eq!(node.attrs()["read_only"], false);
        let path = node.abs_path(&inotify.path).unwrap();
        {
            let mut fh = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            fh.write_all(b"Hello World").unwrap();
        }
        // `os.chmod(path, 0o400)`; on Windows CPython sets the read-only attribute for it.
        #[cfg(unix)]
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).unwrap();
        #[cfg(not(unix))]
        {
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_readonly(true);
            std::fs::set_permissions(&path, perms).unwrap();
        }

        inotify.handler();
        let node = inotify.fs.get("/test/a/1.gcode").unwrap();
        assert_eq!(node.attrs()["size"], 11);
        assert_eq!(node.attrs()["read_only"], true);

        // check event to Connect
        let (event, source, data) = inotify.last();
        assert_eq!(event, Event::FileChanged);
        assert_eq!(source, Source::Wui);
        assert_eq!(data["file"]["name"], "1.gcode");
        assert!(
            data["file"]
                .as_object()
                .unwrap()
                .contains_key("m_timestamp")
        );
        assert_eq!(data["file"]["read_only"], true);
        assert_eq!(data["old_path"], "/test/a/1.gcode");
        assert_eq!(data["new_path"], "/test/a/1.gcode");
        assert!(data["free_space"].as_u64().unwrap() > 0);
    }

    #[test]
    fn test_connect_302() {
        let inotify = inotify();
        let nodes = nodes();
        inotify
            .fs
            .attach("wrong", nodes.clone(), "/t", true, StorageType::Local, None)
            .unwrap();
        inotify
            .fs
            .attach("right", nodes, "/tmp", true, StorageType::Local, None)
            .unwrap();

        let base_storage = inotify.handler.attach_for("/tmp/a/b").unwrap();
        assert_eq!(base_storage.storage, "right");
    }

    #[test]
    fn test_timestamp() {
        let inotify = inotify();
        let storage_b = File::node("storage", true);
        let storage_c = File::node("storage", true);
        inotify
            .fs
            .attach("b", storage_b, "/tmp/b", true, StorageType::Local, None)
            .unwrap();
        inotify
            .fs
            .attach("c", storage_c, "/tmp/c", true, StorageType::Local, None)
            .unwrap();

        let first = inotify.handler.attach_for("/tmp/b").unwrap();
        let second = inotify.handler.attach_for("/tmp/c").unwrap();

        assert!(first.last_updated() < second.last_updated());
    }
}
