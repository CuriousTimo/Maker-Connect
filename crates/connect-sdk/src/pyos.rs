//! The `os`, `os.path` and `shutil` functions `files.py`, `fileinfo.py` and `download.py` stand on, ported from
//! CPython 3.13.12 (`reference/cpython-3.13`, PSF licence):
//!
//! * `os.path` (`posixpath.py`, `genericpath.py`): [`join`], [`split`], [`normpath`] (the C
//!   `os._path_normpath`, `Python/fileutils.c` `_Py_normpath_and_size` and `_Py_skiproot`),
//!   [`abspath`], [`realpath`] (`strict=False`, the SDK's), [`relpath`], [`commonpath`],
//!   [`exists`], [`lexists`], [`isdir`], [`islink`], [`isabs`], [`basename`], [`dirname`];
//! * `os` (`Lib/os.py`, `Modules/posixmodule.c`): [`getcwd`], [`stat`], [`lstat`], [`access`],
//!   [`statvfs`], [`unlink`], [`remove`], [`rmdir`], [`mkdir`], [`makedirs`], [`rename`], [`scandir`]
//!   and its [`DirEntry`], [`walk`];
//! * `shutil.rmtree` (`Lib/shutil.py`) with the SDK's arguments: [`rmtree`];
//! * the `OSError` these raise (`Objects/exceptions.c`): [`OsError`], its class by `errno` and
//!   its `repr`, which `command.py` sends as `error=repr(err)`.
//!
//! A path is bytes: a Python `str` path is `os.fsencode`d to the same bytes before any call
//! (UTF-8 with `surrogateescape`), and the functions here act on `/` and `.` only, which no
//! other character's encoding contains. Where Python compares `str`s (`commonpath`, `relpath`),
//! it takes the common prefix of the smallest and largest, which is the common prefix of all
//! under any order, so comparing bytes gives the same.
//!
//! Linux, macOS and Windows, one codebase (owner, 2026-11-25, #261: "focus nu op 1 codebase"):
//! the calls go through Rust's `std::fs` (and the crate `fs4` for `statvfs`) under CPython's
//! names, the same code on every OS. What reaches Connect is CPython's: an error's `repr`
//! (`command.py` sends `error=repr(err)`). Only what the OSes number or write apart is told apart:
//! an error's number (on Windows `winerror_to_errno`, `PC/errmap.h`), a path's bytes and a
//! drive. `shutil.rmtree` is CPython's branch without fd functions (`_rmtree_unsafe`), as `std`
//! has no `*at` calls.
//!
//! On Windows a path is written with `/`, which Windows accepts; a drive (`C:`) before the root is
//! the one addition to posixpath's rules ([`path_bytes`] gives a Windows path in this form).
//! What differs from CPython on Windows and from the POSIX calls is in
//! `docs/audits/connect-sdk-pyos.md`, "Not covered".

use std::ffi::CStr;
use std::os::raw::c_int;
use std::path::{Path, PathBuf};

use maker_connect_gcode_metadata::py::repr_str;

/// What an `OSError` names as its `filename`: a path, or the fd of a call on an fd.
#[derive(Debug, Clone, PartialEq)]
pub enum Filename {
    Path(Vec<u8>),
    Fd(c_int),
}

/// An `OSError`: from a call, `OSError(errno, strerror)` with its `filename`; raised by the
/// library's own code, `OSError(message)` (`errno` `None`).
#[derive(Debug, Clone, PartialEq)]
pub struct OsError {
    pub errno: Option<c_int>,
    /// `strerror`, or the message when `errno` is `None`.
    pub strerror: String,
    pub filename: Option<Filename>,
    /// The second path of a two-path call (`os.rename`'s `dst`, `path_error2`).
    pub filename2: Option<Filename>,
    /// Windows: the error code of a failed call, from which `errno` is made (`winerror`).
    pub winerror: Option<i32>,
}

/// An exception of these functions: an `OSError` (or a subclass), or a `ValueError`.
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    Os(OsError),
    Value(String),
}

impl OsError {
    /// `OSError(errno, strerror(errno))` with its `filename`.
    pub fn from_errno(errno: c_int, filename: Option<Filename>) -> OsError {
        // Python/errors.c PyErr_SetFromErrnoWithFilenameObjects: errno 0 is "Error".
        let strerror = if errno == 0 {
            "Error".to_string()
        } else {
            // SAFETY: strerror returns a NUL-terminated string.
            unsafe { CStr::from_ptr(libc::strerror(errno)) }
                .to_string_lossy()
                .into_owned()
        };
        OsError {
            errno: Some(errno),
            strerror,
            filename,
            filename2: None,
            winerror: None,
        }
    }

    /// The `OSError` Python raises for a failed call that Rust's `std` made: its `errno`,
    /// `strerror` and, on Windows, `winerror` (`oserror_parse_args`); no filename. The text is
    /// the OS's own, as `std` gives it (`strerror`; on Windows `FormatMessageW`), without trailing
    /// CR/LF and dots (`Python/errors.c`; no POSIX `strerror` ends in either).
    pub fn from_io(err: &std::io::Error) -> OsError {
        let Some(code) = err.raw_os_error() else {
            return OsError::message(&err.to_string());
        };
        let text = err.to_string();
        let text = text
            .strip_suffix(&format!(" (os error {code})"))
            .unwrap_or(&text)
            .trim_end_matches(|c: char| c <= ' ' || c == '.');
        OsError {
            errno: Some(errno_of(code)),
            strerror: text.to_string(),
            filename: None,
            filename2: None,
            winerror: winerror_of(code),
        }
    }

    fn message(message: &str) -> OsError {
        OsError {
            errno: None,
            strerror: message.to_string(),
            filename: None,
            filename2: None,
            winerror: None,
        }
    }

    /// The `OSError` of a failed `open(file, ...)` (`Modules/_io/fileio.c`
    /// `PyErr_SetFromErrnoWithFilenameObject(PyExc_OSError, nameobj)`) or of Rust's `std` on a
    /// named file: its `errno`, `strerror` and that `filename`.
    pub fn from_io_with_filename(err: &std::io::Error, filename: &[u8]) -> OsError {
        let mut os = OsError::from_io(err);
        if os.errno.is_some() {
            os.filename = Some(Filename::Path(filename.to_vec()));
        }
        os
    }

    /// `str(err)` (`OSError_str`): `[Errno n] strerror: 'filename'` (`-> 'filename2'`),
    /// `[Errno n] strerror` without a filename, the message alone (`BaseException_str` of one
    /// argument) without an `errno`; on Windows `[WinError n]` before the others.
    pub fn str(&self) -> String {
        let name = |f: &Filename| match f {
            // `os.fsdecode`; a path that is not UTF-8 is not covered (U+FFFD per bad byte).
            Filename::Path(p) => repr_str(&String::from_utf8_lossy(p)),
            Filename::Fd(fd) => fd.to_string(),
        };
        let Some(errno) = self.errno else {
            return self.strerror.clone();
        };
        // If available, winerror has the priority over myerrno
        let code = match self.winerror {
            Some(winerror) => format!("[WinError {winerror}]"),
            None => format!("[Errno {errno}]"),
        };
        match (&self.filename, &self.filename2) {
            (Some(f), Some(f2)) => {
                format!("{code} {}: {} -> {}", self.strerror, name(f), name(f2))
            }
            (Some(f), None) => format!("{code} {}: {}", self.strerror, name(f)),
            _ => format!("{code} {}", self.strerror),
        }
    }

    /// The class `OSError(errno, ...)` makes (`Objects/exceptions.c` `_PyExc_InitState`,
    /// `ADD_ERRNO`; `ENOTCAPABLE` is not in the reference's build).
    pub fn class_name(&self) -> &'static str {
        let Some(errno) = self.errno else {
            return "OSError";
        };
        match errno {
            libc::EAGAIN | libc::EALREADY | libc::EINPROGRESS => "BlockingIOError",
            e if e == libc::EWOULDBLOCK => "BlockingIOError",
            libc::EPIPE => "BrokenPipeError",
            #[cfg(unix)]
            libc::ESHUTDOWN => "BrokenPipeError",
            libc::ECHILD => "ChildProcessError",
            libc::ECONNABORTED => "ConnectionAbortedError",
            libc::ECONNREFUSED => "ConnectionRefusedError",
            libc::ECONNRESET => "ConnectionResetError",
            libc::EEXIST => "FileExistsError",
            libc::ENOENT => "FileNotFoundError",
            libc::EISDIR => "IsADirectoryError",
            libc::ENOTDIR => "NotADirectoryError",
            libc::EINTR => "InterruptedError",
            libc::EACCES | libc::EPERM => "PermissionError",
            libc::ESRCH => "ProcessLookupError",
            libc::ETIMEDOUT => "TimeoutError",
            // WSAETIMEDOUT (Windows)
            10060 => "TimeoutError",
            _ => "OSError",
        }
    }

    pub fn is_file_not_found(&self) -> bool {
        self.errno == Some(libc::ENOENT)
    }

    /// `repr(err)` (`BaseException_repr`): the class and its `args`. With a filename `args` is
    /// `(errno, strerror)`; a Windows error without one keeps all five (`oserror_init`).
    pub fn repr(&self) -> String {
        match (self.errno, self.winerror, &self.filename) {
            (Some(errno), Some(winerror), None) => format!(
                "{}({errno}, {}, None, {winerror}, None)",
                self.class_name(),
                repr_str(&self.strerror)
            ),
            (Some(errno), _, _) => format!(
                "{}({errno}, {})",
                self.class_name(),
                repr_str(&self.strerror)
            ),
            (None, _, _) => format!("{}({})", self.class_name(), repr_str(&self.strerror)),
        }
    }
}

impl Error {
    /// `repr(err)`.
    pub fn repr(&self) -> String {
        match self {
            Error::Os(err) => err.repr(),
            Error::Value(message) => format!("ValueError({})", repr_str(message)),
        }
    }
}

impl From<OsError> for Error {
    fn from(err: OsError) -> Error {
        Error::Os(err)
    }
}

/// The `errno` of an OS error code: the code itself, on Windows its `winerror_to_errno`.
/// Windows numbers its errors apart from `errno`: the one place the OSes differ here.
fn errno_of(code: i32) -> c_int {
    #[cfg(windows)]
    return winerror_to_errno(code);
    #[cfg(not(windows))]
    code
}

/// The `winerror` of an OS error code: on Windows the code, elsewhere none.
fn winerror_of(code: i32) -> Option<i32> {
    #[cfg(windows)]
    return Some(code);
    #[cfg(not(windows))]
    {
        let _ = code;
        None
    }
}

/// `winerror_to_errno` (`PC/errmap.h`), the codes by number as there.
#[cfg(windows)]
fn winerror_to_errno(winerror: i32) -> c_int {
    let mut winerror = winerror;
    // Unwrap FACILITY_WIN32 HRESULT errors.
    if (winerror as u32 & 0xFFFF_0000) == 0x8007_0000 {
        winerror &= 0x0000_FFFF;
    }
    // Winsock error codes (10000-11999) are errno values.
    if (10000..12000).contains(&winerror) {
        return match winerror {
            // WSAEINTR, WSAEBADF, WSAEACCES, WSAEFAULT, WSAEINVAL, WSAEMFILE
            10004 | 10009 | 10013 | 10014 | 10022 | 10024 => winerror - 10000,
            _ => winerror,
        };
    }
    match winerror {
        2 | 3 | 15 | 18 | 53 | 67 | 161 | 206 => libc::ENOENT,
        10 => libc::E2BIG,
        11 | 188..=202 => libc::ENOEXEC,
        6 | 114 | 130 => libc::EBADF,
        128 | 129 => libc::ECHILD,
        89 | 164 | 215 => libc::EAGAIN,
        7 | 8 | 9 | 1816 => libc::ENOMEM,
        5 | 16 | 19..=34 | 36 | 65 | 82 | 83 | 108 | 132 | 158 | 167 => libc::EACCES,
        80 | 183 => libc::EEXIST,
        17 => libc::EXDEV,
        267 => libc::ENOTDIR,
        4 => libc::EMFILE,
        112 => libc::ENOSPC,
        109 | 232 => libc::EPIPE,
        145 => libc::ENOTEMPTY,
        1113 => libc::EILSEQ,
        258 => libc::ETIMEDOUT,
        _ => libc::EINVAL,
    }
}

/// `path_converter` (`Modules/posixmodule.c`): the path for `std`; a NUL in it is a
/// `ValueError` (`FORMAT_EXCEPTION`), named by the function (`PATH_T_INITIALIZE_P` in
/// `Modules/clinic/posixmodule.c.h`; every one here names its argument `path`).
fn os_path(function: &str, path: &[u8]) -> Result<PathBuf, Error> {
    os_path_arg(function, "path", path)
}

fn os_path_arg(function: &str, argument: &str, path: &[u8]) -> Result<PathBuf, Error> {
    if path.contains(&0) {
        return Err(Error::Value(format!(
            "{function}: embedded null character in {argument}"
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(PathBuf::from(std::ffi::OsStr::from_bytes(path)))
    }
    #[cfg(not(unix))]
    {
        Ok(PathBuf::from(String::from_utf8_lossy(path).into_owned()))
    }
}

/// A path from `std` (or from notify) as the functions here take it: its bytes; on Windows
/// UTF-8 with `/` for `\`, without the `\\?\` of a verbatim path.
pub fn path_bytes(path: &Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        let s = path.to_string_lossy().replace('\\', "/");
        match s.strip_prefix("//?/") {
            Some(rest) => rest.as_bytes().to_vec(),
            None => s.into_bytes(),
        }
    }
}

/// The `OSError` of a failed `std` call on `filename`.
fn io_error(err: &std::io::Error, filename: &[u8]) -> Error {
    Error::Os(OsError::from_io_with_filename(err, filename))
}

// ---------------------------------------------------------------------------------------------
// os.path: posixpath.py, genericpath.py

const SEP: u8 = b'/';

// The `st_mode` bits, the same on Linux, macOS and in CPython on Windows (`_S_IFDIR`, `_S_IFREG`).
const S_IFMT: u32 = 0o170000;
const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;
const S_IFLNK: u32 = 0o120000;

/// The drive before a path's root (`C:`): its length, on Windows only.
#[cfg(windows)]
fn drive(p: &[u8]) -> usize {
    if p.len() >= 2 && p[1] == b':' && p[0].is_ascii_alphabetic() {
        2
    } else {
        0
    }
}

/// The drive before a path's root: none outside Windows.
#[cfg(not(windows))]
fn drive(_p: &[u8]) -> usize {
    0
}

/// Whether `p` starts at a root: `/`, on Windows also after a drive.
fn isabs_bytes(p: &[u8]) -> bool {
    p[drive(p)..].starts_with(b"/")
}

/// The root a path starts at: `/`, on Windows its drive and `/`.
fn root_of(p: &[u8]) -> Vec<u8> {
    let mut root = p[..drive(p)].to_vec();
    root.push(SEP);
    root
}

/// `posixpath.join(a, *p)`.
pub fn join(a: &[u8], p: &[&[u8]]) -> Vec<u8> {
    let mut path = a.to_vec();
    for b in p {
        if drive(b) > 0 || b.starts_with(b"/") || path.is_empty() {
            path = b.to_vec();
        } else if path.ends_with(b"/") {
            path.extend_from_slice(b);
        } else {
            path.push(SEP);
            path.extend_from_slice(b);
        }
    }
    path
}

/// `posixpath.split(p)`: `(head, tail)`.
pub fn split(p: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let i = p.iter().rposition(|&c| c == SEP).map_or(0, |i| i + 1);
    let (mut head, tail) = (&p[..i], &p[i..]);
    if head.len() > drive(head) && head[drive(head)..].iter().any(|&c| c != SEP) {
        while head.last() == Some(&SEP) {
            head = &head[..head.len() - 1];
        }
    }
    (head.to_vec(), tail.to_vec())
}

/// `_Py_skiproot` (`Python/fileutils.c`), POSIX: `(drvsize, rootsize)`; on Windows a drive
/// (`C:`) and the one `/` after it.
fn skiproot(path: &[u8]) -> (usize, usize) {
    let at = |i: usize| path.get(i).copied().unwrap_or(0);
    let d = drive(path);
    if d > 0 {
        (d, usize::from(at(d) == SEP))
    } else if at(0) != SEP {
        (0, 0)
    } else if at(1) != SEP || at(2) == SEP {
        (0, 1)
    } else {
        (0, 2)
    }
}

/// `os._path_normpath` (`Modules/posixmodule.c` `os__path_normpath_impl`): `_Py_normpath_and_size`
/// (`Python/fileutils.c`) in place on a copy, POSIX (no `ALTSEP`); an empty result is `.`.
pub fn normpath(path: &[u8]) -> Vec<u8> {
    let size = path.len();
    if size == 0 {
        return b".".to_vec();
    }
    // The buffer with its terminating NUL, as the C string the code reads.
    let mut buf = path.to_vec();
    buf.push(0);
    let p_end = size as isize;
    let is_end = |x: isize| x == p_end;
    let is_sep = |buf: &[u8], x: isize| buf[x as usize] == SEP;
    let sep_or_end = |buf: &[u8], x: isize| is_end(x) || is_sep(buf, x);

    let mut p1: isize = 0;
    let mut p2: isize = 0;
    let mut min_p2: isize = 0;
    let mut last_c: u8 = 0;

    let (drvsize, rootsize) = skiproot(path);
    if drvsize != 0 || rootsize != 0 {
        p1 = (drvsize + rootsize) as isize;
        p2 = p1;
        min_p2 = p2 - 1;
        last_c = buf[min_p2 as usize];
    }
    if buf[p1 as usize] == b'.' && sep_or_end(&buf, p1 + 1) {
        p1 += 1;
        last_c = buf[p1 as usize];
        while !is_end(p1) && is_sep(&buf, p1) {
            p1 += 1;
        }
    }

    while !is_end(p1) {
        let c = buf[p1 as usize];
        if last_c == SEP {
            if c == b'.' {
                let sep_at_1 = sep_or_end(&buf, p1 + 1);
                let sep_at_2 = !sep_at_1 && sep_or_end(&buf, p1 + 2);
                if sep_at_2 && buf[(p1 + 1) as usize] == b'.' {
                    let mut p3 = p2;
                    loop {
                        if p3 == min_p2 {
                            break;
                        }
                        p3 -= 1;
                        if buf[p3 as usize] != SEP {
                            break;
                        }
                    }
                    while p3 != min_p2 && buf[(p3 - 1) as usize] != SEP {
                        p3 -= 1;
                    }
                    if p2 == min_p2
                        || (buf[p3 as usize] == b'.'
                            && buf[(p3 + 1) as usize] == b'.'
                            && is_sep(&buf, p3 + 2))
                    {
                        // Previous segment is also ../, so append instead.
                        // Relative path does not absorb ../ at minP2 as well.
                        buf[p2 as usize] = b'.';
                        p2 += 1;
                        buf[p2 as usize] = b'.';
                        p2 += 1;
                        last_c = b'.';
                    } else if buf[p3 as usize] == SEP {
                        // Absolute path, so absorb segment
                        p2 = p3 + 1;
                    } else {
                        p2 = p3;
                    }
                    p1 += 1;
                } else if sep_at_1 {
                } else {
                    last_c = c;
                    buf[p2 as usize] = c;
                    p2 += 1;
                }
            } else if c == SEP {
            } else {
                last_c = c;
                buf[p2 as usize] = c;
                p2 += 1;
            }
        } else {
            last_c = c;
            buf[p2 as usize] = c;
            p2 += 1;
        }
        p1 += 1;
    }
    buf[p2 as usize] = 0;
    if p2 != min_p2 {
        loop {
            p2 -= 1;
            if p2 == min_p2 || buf[p2 as usize] != SEP {
                break;
            }
            buf[p2 as usize] = 0;
        }
    } else {
        p2 -= 1;
    }
    let normsize = (p2 + 1) as usize;
    if normsize == 0 {
        b".".to_vec()
    } else {
        buf[..normsize].to_vec()
    }
}

/// `os.getcwd()` (`posix_getcwd`).
pub fn getcwd() -> Result<Vec<u8>, Error> {
    std::env::current_dir()
        .map(|p| path_bytes(&p))
        .map_err(|e| Error::Os(OsError::from_io(&e)))
}

/// `posixpath.abspath(path)`.
pub fn abspath(path: &[u8]) -> Result<Vec<u8>, Error> {
    let mut path = path.to_vec();
    if !isabs_bytes(&path) {
        path = join(&getcwd()?, &[&path]);
    }
    Ok(normpath(&path))
}

/// `posixpath.realpath(filename)` with `strict=False` (the SDK's): an `OSError` of `lstat` or
/// `readlink` is ignored.
pub fn realpath(filename: &[u8]) -> Result<Vec<u8>, Error> {
    use std::collections::HashMap;

    // The stack of unresolved path parts; `None` marks a resolved symlink target, the symlink's
    // path below it.
    let mut rest: Vec<Option<Vec<u8>>> = filename[drive(filename)..]
        .split(|&c| c == SEP)
        .rev()
        .map(|s| Some(s.to_vec()))
        .collect();
    let mut part_count = rest.len();
    let mut path = if isabs_bytes(filename) {
        root_of(filename)
    } else {
        getcwd()?
    };
    // `/`; on Windows the drive's root.
    let mut root = root_of(&path);
    let mut seen: HashMap<Vec<u8>, Option<Vec<u8>>> = HashMap::new();

    while part_count != 0 {
        let Some(name) = rest.pop().expect("part_count counts the parts") else {
            // resolved symlink target
            let link = rest.pop().flatten().expect("the symlink below its marker");
            seen.insert(link, Some(path.clone()));
            continue;
        };
        part_count -= 1;
        if name.is_empty() || name == b"." {
            continue;
        }
        if name == b".." {
            let i = path
                .iter()
                .rposition(|&c| c == SEP)
                .expect("path is absolute");
            path.truncate(i);
            if path.len() < root.len() {
                path = root.clone();
            }
            continue;
        }
        let newpath = if path == root {
            [&path[..], &name].concat()
        } else {
            [&path[..], b"/", &name].concat()
        };
        let target = match lstat(&newpath) {
            Err(Error::Os(_)) => None,
            Err(err) => return Err(err),
            Ok(st) => {
                if st.st_mode & S_IFMT != S_IFLNK {
                    path = newpath;
                    continue;
                }
                if let Some(resolved) = seen.get(&newpath) {
                    match resolved {
                        // use cached value
                        Some(p) => path = p.clone(),
                        // The symlink is not resolved, so we must have a symlink loop.
                        None => path = newpath,
                    }
                    continue;
                }
                match readlink(&newpath) {
                    Err(Error::Os(_)) => None,
                    Err(err) => return Err(err),
                    Ok(target) => Some(target),
                }
            }
        };
        let Some(target) = target else {
            // An error occurred and was ignored.
            path = newpath;
            continue;
        };
        if isabs_bytes(&target) {
            root = root_of(&target);
            path = root.clone();
        }
        seen.insert(newpath.clone(), None);
        rest.push(Some(newpath));
        rest.push(None);
        let parts: Vec<_> = target[drive(&target)..]
            .split(|&c| c == SEP)
            .rev()
            .collect();
        part_count += parts.len();
        rest.extend(parts.into_iter().map(|s| Some(s.to_vec())));
    }
    Ok(path)
}

/// `genericpath.commonprefix(m)` on lists of path parts, as `relpath` calls it.
fn commonprefix<'a>(m: &[Vec<&'a [u8]>]) -> Vec<&'a [u8]> {
    let Some(s1) = m.iter().min() else {
        return Vec::new();
    };
    let s2 = m.iter().max().expect("not empty");
    for (i, c) in s1.iter().enumerate() {
        if *c != s2[i] {
            return s1[..i].to_vec();
        }
    }
    s1.clone()
}

/// `posixpath.relpath(path, start=None)`.
pub fn relpath(path: &[u8], start: Option<&[u8]>) -> Result<Vec<u8>, Error> {
    if path.is_empty() {
        return Err(Error::Value("no path specified".into()));
    }
    let start = start.unwrap_or(b".");
    let start_abs = abspath(start)?;
    let path_abs = abspath(path)?;
    let lstrip = |s: &[u8]| -> Vec<u8> {
        let i = s.iter().position(|&c| c != SEP).unwrap_or(s.len());
        s[i..].to_vec()
    };
    let (start_tail, path_tail) = (lstrip(&start_abs), lstrip(&path_abs));
    let parts = |s: &'_ [u8]| -> Vec<Vec<u8>> {
        if s.is_empty() {
            Vec::new()
        } else {
            s.split(|&c| c == SEP).map(<[u8]>::to_vec).collect()
        }
    };
    let (start_list, path_list) = (parts(&start_tail), parts(&path_tail));
    let lists = [
        start_list.iter().map(Vec::as_slice).collect::<Vec<_>>(),
        path_list.iter().map(Vec::as_slice).collect::<Vec<_>>(),
    ];
    let i = commonprefix(&lists).len();
    let mut rel_list: Vec<&[u8]> = vec![b".."; start_list.len() - i];
    rel_list.extend(path_list[i..].iter().map(Vec::as_slice));
    if rel_list.is_empty() {
        return Ok(b".".to_vec());
    }
    Ok(rel_list.join(&SEP))
}

/// `posixpath.commonpath(paths)`.
pub fn commonpath(paths: &[&[u8]]) -> Result<Vec<u8>, Error> {
    if paths.is_empty() {
        return Err(Error::Value("commonpath() arg is an empty sequence".into()));
    }
    let isabs = isabs_bytes(paths[0]);
    if paths.iter().any(|p| isabs_bytes(p) != isabs) {
        return Err(Error::Value("Can't mix absolute and relative paths".into()));
    }
    let split_paths: Vec<Vec<&[u8]>> = paths
        .iter()
        .map(|p| {
            p[drive(p)..]
                .split(|&c| c == SEP)
                .filter(|c| !c.is_empty() && *c != b".")
                .collect()
        })
        .collect();
    let s1 = split_paths.iter().min().expect("not empty");
    let s2 = split_paths.iter().max().expect("not empty");
    let mut common = &s1[..];
    for (i, c) in s1.iter().enumerate() {
        if *c != s2[i] {
            common = &s1[..i];
            break;
        }
    }
    let mut out = if isabs { root_of(paths[0]) } else { Vec::new() };
    out.extend_from_slice(&common.join(&SEP));
    Ok(out)
}

/// `posixpath.isabs(s)`; on Windows also after a drive.
pub fn isabs(s: &str) -> bool {
    isabs_bytes(s.as_bytes())
}

/// `posixpath.basename(p)`.
pub fn basename(p: &str) -> &str {
    let i = p.rfind('/').map_or(0, |i| i + 1);
    &p[i..]
}

/// `posixpath.dirname(p)`.
pub fn dirname(p: &str) -> &str {
    let i = p.rfind('/').map_or(0, |i| i + 1);
    let head = &p[..i];
    let d = drive(head.as_bytes());
    if head.len() > d && head.bytes().skip(d).any(|c| c != SEP) {
        head.trim_end_matches('/')
    } else {
        head
    }
}

/// `genericpath.exists(path)`: `os.stat`, any `OSError` or `ValueError` is `False`.
pub fn exists(path: &[u8]) -> bool {
    stat(path).is_ok()
}

/// `genericpath.lexists(path)`.
pub fn lexists(path: &[u8]) -> bool {
    lstat(path).is_ok()
}

/// `genericpath.isdir(s)`.
pub fn isdir(s: &[u8]) -> bool {
    stat(s).is_ok_and(|st| st.is_dir())
}

/// `genericpath.islink(path)`.
pub fn islink(path: &[u8]) -> bool {
    lstat(path).is_ok_and(|st| st.is_link())
}

// ---------------------------------------------------------------------------------------------
// os: Modules/posixmodule.c

/// The fields of `os.stat_result` the SDK reads (`fileinfo.py`, `files.py`: `st_size`,
/// `st_mtime`), and the file's type that `os.path` and `DirEntry` test.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StatResult {
    /// The type bits of `st_mode` (`S_IFMT`): a directory, a symlink, a regular file or other.
    pub st_mode: u32,
    pub st_size: i64,
    /// `fill_time`: `sec + 1e-9*nsec`.
    pub st_mtime: f64,
}

impl StatResult {
    fn from_metadata(m: &std::fs::Metadata) -> StatResult {
        let t = m.file_type();
        let st_mode = if t.is_symlink() {
            S_IFLNK
        } else if t.is_dir() {
            S_IFDIR
        } else if t.is_file() {
            S_IFREG
        } else {
            0
        };
        let since = |t: std::time::SystemTime| match t.duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => d.as_secs() as f64 + 1e-9 * f64::from(d.subsec_nanos()),
            Err(before) => {
                let d = before.duration();
                -(d.as_secs() as f64 + 1e-9 * f64::from(d.subsec_nanos()))
            }
        };
        StatResult {
            st_mode,
            st_size: m.len() as i64,
            st_mtime: m.modified().map_or(0.0, since),
        }
    }

    fn fmt(&self) -> u32 {
        self.st_mode & S_IFMT
    }

    pub fn is_dir(&self) -> bool {
        self.fmt() == S_IFDIR
    }

    pub fn is_link(&self) -> bool {
        self.fmt() == S_IFLNK
    }
}

/// `win32_xstat_impl`'s fast path (`posixmodule.c:2164-2193`): `GetFileInformationByName`,
/// loaded from `api-ms-win-core-file-l2-1-4` (`Include/internal/pycore_fileutils_windows.h:48-76`),
/// and the four errors it ends on there. Otherwise CPython goes on to its slow path, which is
/// `std::fs::metadata` here (the fields of [`StatResult`] are the same from either).
#[cfg(windows)]
fn xstat_fast_path_error(p: &Path) -> Option<std::io::Error> {
    use std::os::windows::ffi::OsStrExt;
    use std::sync::OnceLock;
    use windows_sys::Win32::Foundation::{
        ERROR_BAD_NET_NAME, ERROR_FILE_NOT_FOUND, ERROR_NOT_READY, ERROR_PATH_NOT_FOUND,
        GetLastError,
    };
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    type ByName = unsafe extern "system" fn(*const u16, i32, *mut std::ffi::c_void, u32) -> i32;
    static BY_NAME: OnceLock<Option<ByName>> = OnceLock::new();
    let by_name = (*BY_NAME.get_or_init(|| {
        let dll: Vec<u16> = "api-ms-win-core-file-l2-1-4\0".encode_utf16().collect();
        // SAFETY: a NUL-terminated name; a null module is checked.
        let module = unsafe { LoadLibraryW(dll.as_ptr()) };
        if module.is_null() {
            return None;
        }
        // SAFETY: a NUL-terminated name in a loaded module; the function's type is the
        // header's `PGetFileInformationByName`.
        unsafe { GetProcAddress(module, c"GetFileInformationByName".as_ptr().cast()) }.map(
            |f| unsafe { std::mem::transmute::<unsafe extern "system" fn() -> isize, ByName>(f) },
        )
    }))?;
    // FileStatBasicByNameInfo, into a FILE_STAT_BASIC_INFORMATION (104 bytes, 8-aligned)
    const FILE_STAT_BASIC_BY_NAME_INFO: i32 = 3;
    let mut info = [0u64; 13];
    let wide: Vec<u16> = p.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: a NUL-terminated path and a buffer of the size given.
    let ok = unsafe {
        by_name(
            wide.as_ptr(),
            FILE_STAT_BASIC_BY_NAME_INFO,
            info.as_mut_ptr().cast(),
            104,
        )
    };
    if ok != 0 {
        return None;
    }
    // SAFETY: no call in between.
    let err = unsafe { GetLastError() };
    matches!(
        err,
        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND | ERROR_NOT_READY | ERROR_BAD_NET_NAME
    )
    .then(|| std::io::Error::from_raw_os_error(err as i32))
}

fn do_stat(path: &[u8], follow_symlinks: bool) -> Result<StatResult, Error> {
    let p = os_path(if follow_symlinks { "stat" } else { "lstat" }, path)?;
    #[cfg(windows)]
    if let Some(e) = xstat_fast_path_error(&p) {
        return Err(io_error(&e, path));
    }
    let m = if follow_symlinks {
        std::fs::metadata(&p)
    } else {
        std::fs::symlink_metadata(&p)
    };
    m.map(|m| StatResult::from_metadata(&m))
        .map_err(|e| io_error(&e, path))
}

/// `os.stat(path)`.
pub fn stat(path: &[u8]) -> Result<StatResult, Error> {
    do_stat(path, true)
}

/// `os.lstat(path)`.
pub fn lstat(path: &[u8]) -> Result<StatResult, Error> {
    do_stat(path, false)
}

/// `os.readlink(path)`.
fn readlink(path: &[u8]) -> Result<Vec<u8>, Error> {
    let p = os_path("readlink", path)?;
    std::fs::read_link(&p)
        .map(|target| path_bytes(&target))
        .map_err(|e| io_error(&e, path))
}

/// `os.access(path, mode)` (`posixmodule.c` `os_access_impl`), `False` on any failure. Unix:
/// `access(path, mode)`, so the owner and the effective ids count. Windows: the attributes of the
/// path itself (`GetFileAttributesW`, which does not follow a symlink), "access is possible if
/// ... write access wasn't requested, or the file isn't read-only, or it's a directory".
pub fn access(path: &[u8], mode: c_int) -> Result<bool, Error> {
    let p = os_path("access", path)?;
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(p.as_os_str().as_bytes())
            .expect("os_path rejects an embedded null character");
        // SAFETY: `c` is a valid NUL-terminated string for the duration of the call.
        Ok(unsafe { libc::access(c.as_ptr(), mode) } == 0)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_READONLY: u32 = 0x1;
        const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
        Ok(match std::fs::symlink_metadata(&p) {
            Err(_) => false,
            Ok(m) => {
                let attr = m.file_attributes();
                mode & W_OK == 0
                    || attr & FILE_ATTRIBUTE_READONLY == 0
                    || attr & FILE_ATTRIBUTE_DIRECTORY != 0
            }
        })
    }
}

/// `os.W_OK`: 2 on Linux, macOS and Windows.
pub const W_OK: c_int = 2;
/// `os.R_OK`: 4 on Linux, macOS and Windows (there `posixmodule.c:3283-3285`).
pub const R_OK: c_int = 4;

/// The fields of `os.statvfs_result` the SDK reads, in blocks of one byte: the SDK multiplies
/// `f_bavail` and `f_blocks` by `f_bsize` (`files.py:301-303`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StatvfsResult {
    pub f_bsize: u64,
    pub f_blocks: u64,
    pub f_bavail: u64,
}

/// `os.statvfs(path)`: the space free to the user and the total. POSIX: the crate `fs4`
/// (`statvfs` in `f_frsize` blocks, as `shutil.disk_usage`). Windows, which has no `os.statvfs`:
/// `shutil.disk_usage`'s `nt._getdiskusage` (`posixmodule.c` `os__getdiskusage_impl`), its total
/// and its free bytes.
pub fn statvfs(path: &[u8]) -> Result<StatvfsResult, Error> {
    let p = os_path("statvfs", path)?;
    #[cfg(not(windows))]
    {
        let st = fs4::statvfs(&p).map_err(|e| io_error(&e, path))?;
        Ok(StatvfsResult {
            f_bsize: 1,
            f_blocks: st.total_space(),
            f_bavail: st.available_space(),
        })
    }
    #[cfg(windows)]
    {
        let (total, free) = getdiskusage(&p)?;
        Ok(StatvfsResult {
            f_bsize: 1,
            f_blocks: total,
            f_bavail: free,
        })
    }
}

/// `os__getdiskusage_impl` (`posixmodule.c`): `GetDiskFreeSpaceExW` of the path, on
/// `ERROR_DIRECTORY` of its `_dirnameW` (cut at the last `\` or `/`, not past `MAX_PATH`);
/// `(total, free)`, else `PyErr_SetFromWindowsErr`, an error without a filename.
#[cfg(windows)]
fn getdiskusage(p: &Path) -> Result<(u64, u64), Error> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{ERROR_DIRECTORY, GetLastError};
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    const MAX_PATH: usize = 260;
    let mut wide: Vec<u16> = p.as_os_str().encode_wide().chain([0]).collect();
    let (mut caller, mut total, mut free) = (0u64, 0u64, 0u64);
    // SAFETY: a NUL-terminated path and three out-values.
    let mut ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut caller, &mut total, &mut free) };
    if ok == 0 {
        // SAFETY: no call in between.
        let mut err = unsafe { GetLastError() };
        if err == ERROR_DIRECTORY {
            let length = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
            if length < MAX_PATH {
                let mut i = length;
                while i > 0 && wide[i] != u16::from(b'\\') && wide[i] != u16::from(b'/') {
                    i -= 1;
                }
                wide[i] = 0;
                // SAFETY: as above, the path cut short at `i`.
                ok = unsafe {
                    GetDiskFreeSpaceExW(wide.as_ptr(), &mut caller, &mut total, &mut free)
                };
            }
            // SAFETY: no call in between.
            err = unsafe { GetLastError() };
        }
        if ok == 0 {
            let e = std::io::Error::from_raw_os_error(err as i32);
            return Err(Error::Os(OsError::from_io(&e)));
        }
    }
    Ok((total, free))
}

/// `os.unlink(path)`.
pub fn unlink(path: &[u8]) -> Result<(), Error> {
    std::fs::remove_file(os_path("unlink", path)?).map_err(|e| io_error(&e, path))
}

/// `os.remove(path)`: the same as `os.unlink` (`os_remove_impl`), named `remove`.
pub fn remove(path: &[u8]) -> Result<(), Error> {
    std::fs::remove_file(os_path("remove", path)?).map_err(|e| io_error(&e, path))
}

/// `os.mkdir(path)` with the default `mode=0o777`, the SDK's (`os.makedirs` without a mode);
/// `std` makes it with 0o777 less the umask too.
pub fn mkdir(path: &[u8]) -> Result<(), Error> {
    std::fs::create_dir(os_path("mkdir", path)?).map_err(|e| io_error(&e, path))
}

/// `os.makedirs(name, mode=0o777, exist_ok=False)` (`Lib/os.py`).
pub fn makedirs(name: &[u8], exist_ok: bool) -> Result<(), Error> {
    let (mut head, mut tail) = split(name);
    if tail.is_empty() {
        (head, tail) = split(&head);
    }
    if !head.is_empty() && !tail.is_empty() && !exists(&head) {
        match makedirs(&head, exist_ok) {
            // Defeats race condition when another thread created the path
            Err(Error::Os(err)) if err.errno == Some(libc::EEXIST) => {}
            other => other?,
        }
        if tail == b"." {
            return Ok(());
        }
    }
    match mkdir(name) {
        // Cannot rely on checking for EEXIST, since the operating system could give priority
        // to other errors like EACCES or EROFS
        Err(Error::Os(_)) if exist_ok && isdir(name) => Ok(()),
        other => other,
    }
}

/// `os.rename(src, dst)` (`internal_rename`): a failure names both (`path_error2`). An existing
/// `dst` is replaced, as by POSIX `rename(2)`, also on Windows (`std`).
pub fn rename(src: &[u8], dst: &[u8]) -> Result<(), Error> {
    let s = os_path_arg("rename", "src", src)?;
    let d = os_path_arg("rename", "dst", dst)?;
    std::fs::rename(&s, &d).map_err(|e| {
        let mut err = OsError::from_io_with_filename(&e, src);
        if err.errno.is_some() {
            err.filename2 = Some(Filename::Path(dst.to_vec()));
        }
        Error::Os(err)
    })
}

/// `os.rmdir(path)`.
pub fn rmdir(path: &[u8]) -> Result<(), Error> {
    std::fs::remove_dir(os_path("rmdir", path)?).map_err(|e| io_error(&e, path))
}

// What `std` says an entry is (`d_type`): only these are told apart, as by `DirEntry`.
const DT_UNKNOWN: u8 = 0;
const DT_OTHER: u8 = 1;
const DT_DIR: u8 = 4;
const DT_REG: u8 = 8;
const DT_LNK: u8 = 10;

/// `os.DirEntry`, with `d_type`: its `stat` and `lstat` cached once fetched.
#[derive(Debug)]
pub struct DirEntry {
    pub name: Vec<u8>,
    /// The joined path.
    pub path: Vec<u8>,
    d_type: u8,
    stat: Option<StatResult>,
    lstat: Option<StatResult>,
}

impl DirEntry {
    /// `DirEntry_fetch_stat`.
    fn fetch_stat(&self, follow_symlinks: bool) -> Result<StatResult, Error> {
        do_stat(&self.path, follow_symlinks)
    }

    fn get_lstat(&mut self) -> Result<StatResult, Error> {
        if self.lstat.is_none() {
            self.lstat = Some(self.fetch_stat(false)?);
        }
        Ok(self.lstat.expect("set above"))
    }

    /// `DirEntry.stat(follow_symlinks=...)`.
    pub fn stat(&mut self, follow_symlinks: bool) -> Result<StatResult, Error> {
        if !follow_symlinks {
            return self.get_lstat();
        }
        if self.stat.is_none() {
            let st = if self.is_symlink()? {
                self.fetch_stat(true)?
            } else {
                self.get_lstat()?
            };
            self.stat = Some(st);
        }
        Ok(self.stat.expect("set above"))
    }

    /// `DirEntry.is_symlink()`.
    pub fn is_symlink(&mut self) -> Result<bool, Error> {
        if self.d_type != DT_UNKNOWN {
            Ok(self.d_type == DT_LNK)
        } else {
            self.test_mode(false, S_IFLNK)
        }
    }

    /// `DirEntry.is_dir(follow_symlinks=...)`.
    pub fn is_dir(&mut self, follow_symlinks: bool) -> Result<bool, Error> {
        self.test_mode(follow_symlinks, S_IFDIR)
    }

    /// `DirEntry_test_mode`.
    fn test_mode(&mut self, follow_symlinks: bool, mode_bits: u32) -> Result<bool, Error> {
        let is_symlink = self.d_type == DT_LNK;
        let need_stat = self.d_type == DT_UNKNOWN || (follow_symlinks && is_symlink);
        if need_stat {
            match self.stat(follow_symlinks) {
                // If file doesn't exist (anymore), then return False
                Err(Error::Os(err)) if err.is_file_not_found() => Ok(false),
                Err(err) => Err(err),
                Ok(st) => Ok(st.st_mode & S_IFMT == mode_bits),
            }
        } else if is_symlink {
            Ok(false)
        } else if mode_bits == S_IFDIR {
            Ok(self.d_type == DT_DIR)
        } else {
            Ok(self.d_type == DT_REG)
        }
    }
}

/// `os.scandir`'s iterator; closed when done or on an error.
pub struct ScandirIterator {
    path: Vec<u8>,
    it: Option<std::fs::ReadDir>,
}

/// `os.scandir(path)`.
pub fn scandir(path: &[u8]) -> Result<ScandirIterator, Error> {
    let p = os_path("scandir", path)?;
    let it = std::fs::read_dir(&p).map_err(|e| io_error(&e, path))?;
    Ok(ScandirIterator {
        path: path.to_vec(),
        it: Some(it),
    })
}

impl Iterator for ScandirIterator {
    type Item = Result<DirEntry, Error>;

    /// `ScandirIterator_iternext`: `.` and `..` are left out (by `std`, as by CPython).
    fn next(&mut self) -> Option<Self::Item> {
        let entry = match self.it.as_mut()?.next() {
            None => {
                self.it = None;
                return None;
            }
            Some(Err(e)) => {
                self.it = None;
                return Some(Err(io_error(&e, &self.path)));
            }
            Some(Ok(entry)) => entry,
        };
        let name = path_bytes(Path::new(&entry.file_name()));
        let d_type = match entry.file_type() {
            Ok(t) if t.is_symlink() => DT_LNK,
            Ok(t) if t.is_dir() => DT_DIR,
            Ok(t) if t.is_file() => DT_REG,
            Ok(_) => DT_OTHER,
            Err(_) => DT_UNKNOWN,
        };
        // join_path_filename
        let mut joined = self.path.clone();
        if !joined.is_empty() && joined.last() != Some(&SEP) {
            joined.push(SEP);
        }
        joined.extend_from_slice(&name);
        Some(Ok(DirEntry {
            name,
            path: joined,
            d_type,
            stat: None,
            lstat: None,
        }))
    }
}

/// What `os.walk` yields: `(dirpath, dirnames, filenames)`.
pub type WalkItem = (Vec<u8>, Vec<Vec<u8>>, Vec<Vec<u8>>);

/// The `onerror` of `os.walk`: called with the `OSError` of a scan; an `Err` aborts the walk.
pub type OnError = Box<dyn FnMut(Error) -> Result<(), Error>>;

enum Pending {
    Path(Vec<u8>),
    Yield(WalkItem),
}

/// `os.walk`'s `followlinks`: `False`, `True`, or `os._walk_symlinks_as_files` (`shutil.rmtree`'s).
#[derive(Clone, Copy, PartialEq)]
enum Links {
    No,
    Follow,
    AsFiles,
}

/// `os.walk(top, topdown, onerror, followlinks)`, a generator: a directory is scanned when the
/// walk reaches it, and top-down the subdirectories to visit are chosen when the next item is
/// asked for.
pub struct Walk {
    stack: Vec<Pending>,
    topdown: bool,
    onerror: Option<OnError>,
    followlinks: Links,
    /// Top-down: the directory just yielded and its subdirectories.
    resume: Option<(Vec<u8>, Vec<Vec<u8>>)>,
    done: bool,
}

/// `os.walk(top, topdown=..., onerror=..., followlinks=...)`.
pub fn walk(top: &[u8], topdown: bool, onerror: Option<OnError>, followlinks: bool) -> Walk {
    walk_links(
        top,
        topdown,
        onerror,
        if followlinks {
            Links::Follow
        } else {
            Links::No
        },
    )
}

fn walk_links(top: &[u8], topdown: bool, onerror: Option<OnError>, followlinks: Links) -> Walk {
    Walk {
        stack: vec![Pending::Path(top.to_vec())],
        topdown,
        onerror,
        followlinks,
        resume: None,
        done: false,
    }
}

impl Walk {
    fn on_error(&mut self, err: Error) -> Result<(), Error> {
        match &mut self.onerror {
            Some(f) => f(err),
            None => Ok(()),
        }
    }

    fn step(&mut self) -> Result<Option<WalkItem>, Error> {
        // `if followlinks`: the sentinel is true too.
        let followlinks = self.followlinks != Links::No;
        if let Some((top, dirs)) = self.resume.take() {
            // Traverse into sub-directories
            for dirname in dirs.iter().rev() {
                let new_path = join(&top, &[dirname]);
                if followlinks || !islink(&new_path) {
                    self.stack.push(Pending::Path(new_path));
                }
            }
        }
        while let Some(top) = self.stack.pop() {
            let top = match top {
                Pending::Yield(item) => return Ok(Some(item)),
                Pending::Path(top) => top,
            };
            let mut dirs = Vec::new();
            let mut nondirs = Vec::new();
            let mut walk_dirs = Vec::new();

            let scandir_it = match scandir(&top) {
                Ok(it) => it,
                Err(err @ Error::Os(_)) => {
                    self.on_error(err)?;
                    continue;
                }
                Err(err) => return Err(err),
            };
            let mut cont = false;
            for entry in scandir_it {
                let mut entry = match entry {
                    Ok(entry) => entry,
                    Err(err) => {
                        self.on_error(err)?;
                        cont = true;
                        break;
                    }
                };
                // `is_dir(follow_symlinks=False) and not is_junction()` for the sentinel: a
                // junction is a symlink to `std`, so `is_dir(False)` is already false for it.
                let is_dir = match entry.is_dir(self.followlinks != Links::AsFiles) {
                    Ok(is_dir) => is_dir,
                    Err(Error::Os(_)) => false,
                    Err(err) => return Err(err),
                };
                if is_dir {
                    dirs.push(entry.name.clone());
                } else {
                    nondirs.push(entry.name.clone());
                }
                if !self.topdown && is_dir {
                    let walk_into = followlinks
                        || match entry.is_symlink() {
                            Ok(is_symlink) => !is_symlink,
                            Err(Error::Os(_)) => true,
                            Err(err) => return Err(err),
                        };
                    if walk_into {
                        walk_dirs.push(entry.path.clone());
                    }
                }
            }
            if cont {
                continue;
            }
            if self.topdown {
                // Yield before sub-directory traversal if going top down
                self.resume = Some((top.clone(), dirs.clone()));
                return Ok(Some((top, dirs, nondirs)));
            }
            // Yield after sub-directory traversal if going bottom up
            self.stack.push(Pending::Yield((top, dirs, nondirs)));
            for new_path in walk_dirs.into_iter().rev() {
                self.stack.push(Pending::Path(new_path));
            }
        }
        Ok(None)
    }
}

impl Iterator for Walk {
    type Item = Result<WalkItem, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let item = self.step().transpose();
        if !matches!(item, Some(Ok(_))) {
            self.done = true;
        }
        item
    }
}

// ---------------------------------------------------------------------------------------------
// shutil.rmtree: Lib/shutil.py

/// `_rmtree_unsafe(path, onexc)` with `onexc` re-raising (the SDK passes no handler).
fn rmtree_unsafe(path: &[u8]) -> Result<(), Error> {
    let onerror: OnError = Box::new(|err| match err {
        Error::Os(err) if err.is_file_not_found() => Ok(()),
        // onexc(os.scandir, err.filename, err): raise
        err => Err(err),
    });
    for item in walk_links(path, false, Some(onerror), Links::AsFiles) {
        let (dirpath, dirnames, filenames) = item?;
        for name in dirnames {
            let fullname = join(&dirpath, &[&name]);
            match rmdir(&fullname) {
                Ok(()) => {}
                Err(Error::Os(err)) if err.is_file_not_found() => continue,
                // onexc(os.rmdir, fullname, err): raise
                Err(err) => return Err(err),
            }
        }
        for name in filenames {
            let fullname = join(&dirpath, &[&name]);
            match unlink(&fullname) {
                Ok(()) => {}
                Err(Error::Os(err)) if err.is_file_not_found() => continue,
                // onexc(os.unlink, fullname, err): raise
                Err(err) => return Err(err),
            }
        }
    }
    match rmdir(path) {
        Err(Error::Os(err)) if err.is_file_not_found() => Ok(()),
        // onexc(os.rmdir, path, err): raise
        other => other,
    }
}

/// `shutil.rmtree(path)`, as the SDK calls it: no `ignore_errors`, `onerror`, `onexc` or
/// `dir_fd`, so the first error is raised. The branch without fd functions, on every OS (`std`
/// has none): `os.lstat`, the symlink check (`_rmtree_islink`; a Windows junction is a symlink
/// to `std`) and `_rmtree_unsafe`.
pub fn rmtree(path: &[u8]) -> Result<(), Error> {
    // onexc(os.lstat, path, err): raise
    let st = lstat(path)?;
    if st.is_link() {
        // symlinks to directories are forbidden, see bug #1669
        return Err(Error::Os(OsError::message(
            "Cannot call rmtree on a symbolic link",
        )));
    }
    rmtree_unsafe(path)
}
