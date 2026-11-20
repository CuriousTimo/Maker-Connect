//! The `os`, `os.path` and `shutil` functions `files.py` and `fileinfo.py` stand on, ported from
//! CPython 3.13.12 (`reference/cpython-3.13`, PSF licence):
//!
//! * `os.path` (`posixpath.py`, `genericpath.py`): [`join`], [`split`], [`normpath`] (the C
//!   `os._path_normpath`, `Python/fileutils.c` `_Py_normpath_and_size` and `_Py_skiproot`),
//!   [`abspath`], [`realpath`] (`strict=False`, the SDK's), [`relpath`], [`commonpath`],
//!   [`exists`], [`lexists`], [`isdir`], [`islink`];
//! * `os` (`Lib/os.py`, `Modules/posixmodule.c`): [`getcwd`], [`stat`], [`lstat`], [`access`],
//!   [`statvfs`], [`unlink`], [`rmdir`], [`scandir`] and its [`DirEntry`], [`walk`];
//! * `shutil.rmtree` (`Lib/shutil.py`, its `_rmtree_safe_fd`, as on Linux and macOS) with the
//!   SDK's arguments: [`rmtree`];
//! * the `OSError` these raise (`Objects/exceptions.c`): [`OsError`], its class by `errno` and
//!   its `repr`, which `command.py` sends as `error=repr(err)`.
//!
//! A path is bytes: a Python `str` path is `os.fsencode`d to the same bytes before any call
//! (UTF-8 with `surrogateescape`), and the functions here act on `/` and `.` only, which no
//! other character's encoding contains. Where Python compares `str`s (`commonpath`, `relpath`),
//! it takes the common prefix of the smallest and largest, which is the common prefix of all
//! under any order, so comparing bytes gives the same.
//!
//! Unix only, as the reference: on Windows `os.path` is `ntpath`, and the SDK's `files.py`
//! imports `inotify_simple`, which needs `fcntl`.

use std::ffi::{CStr, CString};
use std::os::raw::c_int;
use std::os::unix::ffi::OsStrExt;

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
}

/// An exception of these functions: an `OSError` (or a subclass), or a `ValueError`.
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    Os(OsError),
    Value(String),
}

impl OsError {
    /// `PyErr_SetFromErrnoWithFilenameObject`: the last `errno`, its `strerror`, `filename`.
    fn last(filename: Option<Filename>) -> OsError {
        let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        OsError::from_errno(errno, filename)
    }

    fn from_errno(errno: c_int, filename: Option<Filename>) -> OsError {
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
        }
    }

    fn message(message: &str) -> OsError {
        OsError {
            errno: None,
            strerror: message.to_string(),
            filename: None,
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
            libc::EPIPE | libc::ESHUTDOWN => "BrokenPipeError",
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
            _ => "OSError",
        }
    }

    pub fn is_file_not_found(&self) -> bool {
        self.errno == Some(libc::ENOENT)
    }

    /// `repr(err)` (`BaseException_repr`): the class and its `args`.
    pub fn repr(&self) -> String {
        match self.errno {
            Some(errno) => format!(
                "{}({errno}, {})",
                self.class_name(),
                repr_str(&self.strerror)
            ),
            None => format!("{}({})", self.class_name(), repr_str(&self.strerror)),
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

/// `path_converter` (`Modules/posixmodule.c`): the path as a C string; a NUL in it is a
/// `ValueError` (`FORMAT_EXCEPTION`), named by the function (`PATH_T_INITIALIZE_P` in
/// `Modules/clinic/posixmodule.c.h`; every one here names its argument `path`).
fn cpath(function: &str, path: &[u8]) -> Result<CString, Error> {
    CString::new(path)
        .map_err(|_| Error::Value(format!("{function}: embedded null character in path")))
}

fn set_errno(value: c_int) {
    // SAFETY: the thread's errno.
    unsafe {
        #[cfg(target_os = "linux")]
        {
            *libc::__errno_location() = value;
        }
        #[cfg(not(target_os = "linux"))]
        {
            *libc::__error() = value;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// os.path: posixpath.py, genericpath.py

const SEP: u8 = b'/';

/// `posixpath.join(a, *p)`.
pub fn join(a: &[u8], p: &[&[u8]]) -> Vec<u8> {
    let mut path = a.to_vec();
    for b in p {
        if b.starts_with(b"/") || path.is_empty() {
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
    if !head.is_empty() && head.iter().any(|&c| c != SEP) {
        while head.last() == Some(&SEP) {
            head = &head[..head.len() - 1];
        }
    }
    (head.to_vec(), tail.to_vec())
}

/// `_Py_skiproot` (`Python/fileutils.c`), POSIX: `(drvsize, rootsize)`.
fn skiproot(path: &[u8]) -> (usize, usize) {
    let at = |i: usize| path.get(i).copied().unwrap_or(0);
    if at(0) != SEP {
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
        .map(|p| p.as_os_str().as_bytes().to_vec())
        .map_err(|e| Error::Os(OsError::from_errno(e.raw_os_error().unwrap_or(0), None)))
}

/// `posixpath.abspath(path)`.
pub fn abspath(path: &[u8]) -> Result<Vec<u8>, Error> {
    let mut path = path.to_vec();
    if !path.starts_with(b"/") {
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
    let mut rest: Vec<Option<Vec<u8>>> = filename
        .split(|&c| c == SEP)
        .rev()
        .map(|s| Some(s.to_vec()))
        .collect();
    let mut part_count = rest.len();
    let mut path = if filename.starts_with(b"/") {
        b"/".to_vec()
    } else {
        getcwd()?
    };
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
            if path.is_empty() {
                path = b"/".to_vec();
            }
            continue;
        }
        let newpath = if path == b"/" {
            [&path[..], &name].concat()
        } else {
            [&path[..], b"/", &name].concat()
        };
        let target = match lstat(&newpath) {
            Err(Error::Os(_)) => None,
            Err(err) => return Err(err),
            Ok(st) => {
                if st.st_mode & libc::S_IFMT as u32 != libc::S_IFLNK as u32 {
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
        if target.starts_with(b"/") {
            path = b"/".to_vec();
        }
        seen.insert(newpath.clone(), None);
        rest.push(Some(newpath));
        rest.push(None);
        let parts: Vec<_> = target.split(|&c| c == SEP).rev().collect();
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
    let isabs = paths[0].starts_with(b"/");
    if paths.iter().any(|p| p.starts_with(b"/") != isabs) {
        return Err(Error::Value("Can't mix absolute and relative paths".into()));
    }
    let split_paths: Vec<Vec<&[u8]>> = paths
        .iter()
        .map(|p| {
            p.split(|&c| c == SEP)
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
    let mut out = if isabs { b"/".to_vec() } else { Vec::new() };
    out.extend_from_slice(&common.join(&SEP));
    Ok(out)
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

/// The fields of `os.stat_result` the SDK reads, and those `rmtree` compares.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StatResult {
    pub st_mode: u32,
    pub st_ino: u64,
    pub st_dev: u64,
    pub st_size: i64,
    /// `fill_time`: `sec + 1e-9*nsec`.
    pub st_mtime: f64,
}

impl StatResult {
    // The field types differ per platform (`st_dev` is an `i32` on macOS).
    #[allow(clippy::unnecessary_cast)]
    fn from_struct(st: &libc::stat) -> StatResult {
        StatResult {
            st_mode: st.st_mode as u32,
            st_ino: st.st_ino as u64,
            st_dev: st.st_dev as u64,
            st_size: st.st_size as i64,
            st_mtime: st.st_mtime as f64 + 1e-9 * st.st_mtime_nsec as f64,
        }
    }

    fn fmt(&self) -> u32 {
        self.st_mode & libc::S_IFMT as u32
    }

    pub fn is_dir(&self) -> bool {
        self.fmt() == libc::S_IFDIR as u32
    }

    pub fn is_link(&self) -> bool {
        self.fmt() == libc::S_IFLNK as u32
    }

    /// `genericpath.samestat`.
    pub fn samestat(&self, other: &StatResult) -> bool {
        self.st_ino == other.st_ino && self.st_dev == other.st_dev
    }
}

fn do_stat(path: &[u8], follow_symlinks: bool) -> Result<StatResult, Error> {
    let c = cpath(if follow_symlinks { "stat" } else { "lstat" }, path)?;
    // SAFETY: an out-parameter filled by the call.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: a NUL-terminated path and the struct above.
    let result = unsafe {
        if follow_symlinks {
            libc::stat(c.as_ptr(), &mut st)
        } else {
            libc::lstat(c.as_ptr(), &mut st)
        }
    };
    if result != 0 {
        return Err(OsError::last(Some(Filename::Path(path.to_vec()))).into());
    }
    Ok(StatResult::from_struct(&st))
}

/// `os.stat(path)`.
pub fn stat(path: &[u8]) -> Result<StatResult, Error> {
    do_stat(path, true)
}

/// `os.lstat(path)`.
pub fn lstat(path: &[u8]) -> Result<StatResult, Error> {
    do_stat(path, false)
}

fn fstatat(dir_fd: c_int, name: &[u8], follow_symlinks: bool) -> Result<StatResult, Error> {
    let c = cpath("stat", name)?;
    // SAFETY: an out-parameter filled by the call.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    let flags = if follow_symlinks {
        0
    } else {
        libc::AT_SYMLINK_NOFOLLOW
    };
    // SAFETY: an fd, a NUL-terminated name and the struct above.
    if unsafe { libc::fstatat(dir_fd, c.as_ptr(), &mut st, flags) } != 0 {
        return Err(OsError::last(Some(Filename::Path(name.to_vec()))).into());
    }
    Ok(StatResult::from_struct(&st))
}

fn fstat(fd: c_int) -> Result<StatResult, Error> {
    // SAFETY: an out-parameter filled by the call.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: an fd and the struct above.
    if unsafe { libc::fstat(fd, &mut st) } != 0 {
        return Err(OsError::last(None).into());
    }
    Ok(StatResult::from_struct(&st))
}

/// `os.readlink(path)`.
fn readlink(path: &[u8]) -> Result<Vec<u8>, Error> {
    let c = cpath("readlink", path)?;
    let mut buf = vec![0u8; libc::PATH_MAX as usize + 1];
    // SAFETY: a NUL-terminated path and a buffer of the given length.
    let n = unsafe { libc::readlink(c.as_ptr(), buf.as_mut_ptr().cast(), buf.len()) };
    if n < 0 {
        return Err(OsError::last(Some(Filename::Path(path.to_vec()))).into());
    }
    buf.truncate(n as usize);
    Ok(buf)
}

/// `os.access(path, mode)`: `False` on any failure.
pub fn access(path: &[u8], mode: c_int) -> Result<bool, Error> {
    let c = cpath("access", path)?;
    // SAFETY: a NUL-terminated path.
    Ok(unsafe { libc::access(c.as_ptr(), mode) } == 0)
}

pub const W_OK: c_int = libc::W_OK;

/// The fields of `os.statvfs_result` the SDK reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StatvfsResult {
    pub f_bsize: u64,
    pub f_blocks: u64,
    pub f_bavail: u64,
}

/// `os.statvfs(path)`: on macOS from `statfs(2)`, `f_bsize` its `f_iosize` (`os_statvfs_impl`,
/// `_pystatvfs_fromstructstatfs`).
pub fn statvfs(path: &[u8]) -> Result<StatvfsResult, Error> {
    let c = cpath("statvfs", path)?;
    #[cfg(target_os = "macos")]
    {
        // SAFETY: an out-parameter filled by the call.
        let mut st: libc::statfs = unsafe { std::mem::zeroed() };
        // SAFETY: a NUL-terminated path and the struct above.
        if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
            return Err(OsError::last(Some(Filename::Path(path.to_vec()))).into());
        }
        Ok(StatvfsResult {
            f_bsize: st.f_iosize as i64 as u64,
            f_blocks: st.f_blocks,
            f_bavail: st.f_bavail,
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        // SAFETY: an out-parameter filled by the call.
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: a NUL-terminated path and the struct above.
        if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
            return Err(OsError::last(Some(Filename::Path(path.to_vec()))).into());
        }
        Ok(StatvfsResult {
            f_bsize: st.f_bsize as u64,
            f_blocks: st.f_blocks as u64,
            f_bavail: st.f_bavail as u64,
        })
    }
}

/// `os.unlink(path)`.
pub fn unlink(path: &[u8]) -> Result<(), Error> {
    let c = cpath("unlink", path)?;
    // SAFETY: a NUL-terminated path.
    if unsafe { libc::unlink(c.as_ptr()) } != 0 {
        return Err(OsError::last(Some(Filename::Path(path.to_vec()))).into());
    }
    Ok(())
}

/// `os.rmdir(path)`.
pub fn rmdir(path: &[u8]) -> Result<(), Error> {
    let c = cpath("rmdir", path)?;
    // SAFETY: a NUL-terminated path.
    if unsafe { libc::rmdir(c.as_ptr()) } != 0 {
        return Err(OsError::last(Some(Filename::Path(path.to_vec()))).into());
    }
    Ok(())
}

/// `os.DirEntry`, POSIX with `d_type`: its `stat` and `lstat` cached once fetched.
#[derive(Debug)]
pub struct DirEntry {
    pub name: Vec<u8>,
    /// The joined path; for a scan of an fd, the name.
    pub path: Vec<u8>,
    dir_fd: Option<c_int>,
    d_type: u8,
    stat: Option<StatResult>,
    lstat: Option<StatResult>,
}

impl DirEntry {
    /// `DirEntry_fetch_stat`.
    fn fetch_stat(&self, follow_symlinks: bool) -> Result<StatResult, Error> {
        match self.dir_fd {
            Some(fd) => fstatat(fd, &self.path, follow_symlinks),
            None => do_stat(&self.path, follow_symlinks),
        }
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
        if self.d_type != libc::DT_UNKNOWN {
            Ok(self.d_type == libc::DT_LNK)
        } else {
            self.test_mode(false, libc::S_IFLNK as u32)
        }
    }

    /// `DirEntry.is_dir(follow_symlinks=...)`.
    pub fn is_dir(&mut self, follow_symlinks: bool) -> Result<bool, Error> {
        self.test_mode(follow_symlinks, libc::S_IFDIR as u32)
    }

    /// `DirEntry_test_mode`.
    fn test_mode(&mut self, follow_symlinks: bool, mode_bits: u32) -> Result<bool, Error> {
        let is_symlink = self.d_type == libc::DT_LNK;
        let need_stat = self.d_type == libc::DT_UNKNOWN || (follow_symlinks && is_symlink);
        if need_stat {
            match self.stat(follow_symlinks) {
                // If file doesn't exist (anymore), then return False
                Err(Error::Os(err)) if err.is_file_not_found() => Ok(false),
                Err(err) => Err(err),
                Ok(st) => Ok(st.st_mode & libc::S_IFMT as u32 == mode_bits),
            }
        } else if is_symlink {
            Ok(false)
        } else if mode_bits == libc::S_IFDIR as u32 {
            Ok(self.d_type == libc::DT_DIR)
        } else {
            Ok(self.d_type == libc::DT_REG)
        }
    }
}

/// What `os.scandir` scans: a path, or an fd (`HAVE_FDOPENDIR`).
enum ScanOf {
    Path(Vec<u8>),
    Fd(c_int),
}

/// `os.scandir`'s iterator; closed when done, on an error, or dropped.
pub struct ScandirIterator {
    of: ScanOf,
    dirp: *mut libc::DIR,
}

/// `os.scandir(path)`.
pub fn scandir(path: &[u8]) -> Result<ScandirIterator, Error> {
    let c = cpath("scandir", path)?;
    // SAFETY: a NUL-terminated path.
    let dirp = unsafe { libc::opendir(c.as_ptr()) };
    if dirp.is_null() {
        return Err(OsError::last(Some(Filename::Path(path.to_vec()))).into());
    }
    Ok(ScandirIterator {
        of: ScanOf::Path(path.to_vec()),
        dirp,
    })
}

/// `os.scandir(fd)`: `fdopendir` on a duplicate (`_Py_dup`), as `closedir` closes it.
fn scandir_fd(fd: c_int) -> Result<ScandirIterator, Error> {
    // SAFETY: duplicating an fd.
    let dup = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    if dup == -1 {
        return Err(OsError::last(None).into());
    }
    // SAFETY: an fd we own.
    let dirp = unsafe { libc::fdopendir(dup) };
    if dirp.is_null() {
        let err = OsError::last(Some(Filename::Fd(fd)));
        // SAFETY: the duplicate, ours.
        unsafe { libc::close(dup) };
        return Err(err.into());
    }
    Ok(ScandirIterator {
        of: ScanOf::Fd(fd),
        dirp,
    })
}

impl ScandirIterator {
    /// `ScandirIterator_closedir`.
    fn closedir(&mut self) {
        if self.dirp.is_null() {
            return;
        }
        let dirp = std::mem::replace(&mut self.dirp, std::ptr::null_mut());
        // SAFETY: a DIR we opened, closed once.
        unsafe {
            if matches!(self.of, ScanOf::Fd(_)) {
                libc::rewinddir(dirp);
            }
            libc::closedir(dirp);
        }
    }
}

impl Iterator for ScandirIterator {
    type Item = Result<DirEntry, Error>;

    /// `ScandirIterator_iternext`, POSIX.
    fn next(&mut self) -> Option<Self::Item> {
        if self.dirp.is_null() {
            return None;
        }
        loop {
            set_errno(0);
            // SAFETY: an open DIR.
            let direntp = unsafe { libc::readdir(self.dirp) };
            if direntp.is_null() {
                let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                let err = (errno != 0).then(|| {
                    let filename = match &self.of {
                        ScanOf::Path(p) => Filename::Path(p.clone()),
                        ScanOf::Fd(fd) => Filename::Fd(*fd),
                    };
                    OsError::from_errno(errno, Some(filename))
                });
                self.closedir();
                return err.map(|e| Err(e.into()));
            }
            // SAFETY: readdir's entry, valid until the next call.
            let (name, d_type) = unsafe {
                (
                    CStr::from_ptr((*direntp).d_name.as_ptr())
                        .to_bytes()
                        .to_vec(),
                    (*direntp).d_type,
                )
            };
            if name == b"." || name == b".." {
                continue;
            }
            let (path, dir_fd) = match &self.of {
                // join_path_filename
                ScanOf::Path(p) => {
                    let mut joined = p.clone();
                    if !joined.is_empty() && joined.last() != Some(&SEP) {
                        joined.push(SEP);
                    }
                    joined.extend_from_slice(&name);
                    (joined, None)
                }
                ScanOf::Fd(fd) => (name.clone(), Some(*fd)),
            };
            return Some(Ok(DirEntry {
                name,
                path,
                dir_fd,
                d_type,
                stat: None,
                lstat: None,
            }));
        }
    }
}

impl Drop for ScandirIterator {
    fn drop(&mut self) {
        self.closedir();
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

/// `os.walk(top, topdown, onerror, followlinks)`, a generator: a directory is scanned when the
/// walk reaches it, and top-down the subdirectories to visit are chosen when the next item is
/// asked for.
pub struct Walk {
    stack: Vec<Pending>,
    topdown: bool,
    onerror: Option<OnError>,
    followlinks: bool,
    /// Top-down: the directory just yielded and its subdirectories.
    resume: Option<(Vec<u8>, Vec<Vec<u8>>)>,
    done: bool,
}

/// `os.walk(top, topdown=..., onerror=..., followlinks=...)`.
pub fn walk(top: &[u8], topdown: bool, onerror: Option<OnError>, followlinks: bool) -> Walk {
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
        if let Some((top, dirs)) = self.resume.take() {
            // Traverse into sub-directories
            for dirname in dirs.iter().rev() {
                let new_path = join(&top, &[dirname]);
                if self.followlinks || !islink(&new_path) {
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
                let is_dir = match entry.is_dir(true) {
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
                    let walk_into = self.followlinks
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

#[derive(Clone, Copy, PartialEq)]
enum Func {
    Lstat,
    Open,
    Islink,
    Scandir,
    Close,
    Rmdir,
}

type RmtreeItem = (Func, Option<c_int>, Vec<u8>, Option<DirEntry>);

fn with_filename(err: Error, path: &[u8]) -> Error {
    match err {
        Error::Os(mut err) => {
            err.filename = Some(Filename::Path(path.to_vec()));
            Error::Os(err)
        }
        err => err,
    }
}

/// `os.open(name, O_RDONLY | O_NONBLOCK, dir_fd=...)`: `O_CLOEXEC` added, retried on `EINTR`.
fn open_dir_fd(name: &[u8], dir_fd: Option<c_int>) -> Result<c_int, Error> {
    let c = cpath("open", name)?;
    let flags = libc::O_RDONLY | libc::O_NONBLOCK | libc::O_CLOEXEC;
    loop {
        // SAFETY: a NUL-terminated name, an fd or none.
        let fd = unsafe {
            match dir_fd {
                Some(dfd) => libc::openat(dfd, c.as_ptr(), flags, 0o777),
                None => libc::open(c.as_ptr(), flags, 0o777),
            }
        };
        if fd >= 0 {
            return Ok(fd);
        }
        let err = OsError::last(Some(Filename::Path(name.to_vec())));
        if err.errno != Some(libc::EINTR) {
            return Err(err.into());
        }
    }
}

fn close(fd: c_int) -> Result<(), Error> {
    // SAFETY: an fd of ours, closed once.
    if unsafe { libc::close(fd) } < 0 {
        return Err(OsError::last(None).into());
    }
    Ok(())
}

fn unlinkat(dir_fd: Option<c_int>, name: &[u8], flags: c_int) -> Result<(), Error> {
    let c = cpath(
        if flags & libc::AT_REMOVEDIR != 0 {
            "rmdir"
        } else {
            "unlink"
        },
        name,
    )?;
    // SAFETY: a NUL-terminated name, an fd or none.
    let result = unsafe {
        match dir_fd {
            Some(fd) => libc::unlinkat(fd, c.as_ptr(), flags),
            None if flags & libc::AT_REMOVEDIR != 0 => libc::rmdir(c.as_ptr()),
            None => libc::unlink(c.as_ptr()),
        }
    };
    if result != 0 {
        return Err(OsError::last(Some(Filename::Path(name.to_vec()))).into());
    }
    Ok(())
}

/// `_rmtree_safe_fd(stack, onexc)` with `onexc` re-raising (the SDK passes no handler).
fn rmtree_safe_fd(stack: &mut Vec<RmtreeItem>) -> Result<(), Error> {
    let (mut func, dirfd, path, orig_entry) = stack.pop().expect("a non-empty stack");
    let mut orig_entry = orig_entry;
    let top_level = orig_entry.is_none();
    let name = match &orig_entry {
        None => path.clone(),
        Some(entry) => entry.name.clone(),
    };
    let result = (|| -> Result<(), Error> {
        if func == Func::Close {
            return close(dirfd.expect("an fd to close"));
        }
        if func == Func::Rmdir {
            return unlinkat(dirfd, &name, libc::AT_REMOVEDIR);
        }
        // Note: To guard against symlink races, we use the standard
        // lstat()/open()/fstat() trick.
        let orig_st = match (&mut orig_entry, dirfd) {
            (None, None) => lstat(&name)?,
            (None, Some(fd)) => fstatat(fd, &name, false)?,
            (Some(entry), _) => entry.stat(false)?,
        };

        func = Func::Open;
        let topfd = open_dir_fd(&name, dirfd)?;

        func = Func::Islink;
        let same = fstat(topfd).map(|st| orig_st.samestat(&st));
        let checked = match same {
            Ok(false) => Err(Error::Os(OsError::message(
                // Symlinks to directories are forbidden, see GH-46010.
                "Cannot call rmtree on a symbolic link",
            ))),
            Ok(true) => {
                stack.push((Func::Rmdir, dirfd, path.clone(), orig_entry.take()));
                Ok(())
            }
            Err(err) => Err(err),
        };
        // The os.close item's entry is not read: close reports `path`, and its errors are raised.
        stack.push((Func::Close, Some(topfd), path.clone(), None));
        checked?;

        func = Func::Scandir;
        let entries: Vec<DirEntry> = scandir_fd(topfd)?.collect::<Result<_, _>>()?;
        for mut entry in entries {
            let fullname = join(&path, &[&entry.name]);
            match entry.is_dir(false) {
                Ok(true) => {
                    // Traverse into sub-directory.
                    stack.push((Func::Lstat, Some(topfd), fullname, Some(entry)));
                    continue;
                }
                Ok(false) => {}
                Err(Error::Os(err)) if err.is_file_not_found() => continue,
                Err(Error::Os(_)) => {}
                Err(err) => return Err(err),
            }
            match unlinkat(Some(topfd), &entry.name, 0) {
                Ok(()) => {}
                Err(Error::Os(err)) if err.is_file_not_found() => continue,
                // onexc(os.unlink, fullname, err): raise
                Err(err) => return Err(err),
            }
        }
        Ok(())
    })();
    match result {
        Ok(()) => Ok(()),
        Err(Error::Os(err)) if err.is_file_not_found() => {
            if top_level || func == Func::Close {
                Err(with_filename(Error::Os(err), &path))
            } else {
                Ok(())
            }
        }
        Err(err @ Error::Os(_)) => Err(with_filename(err, &path)),
        Err(err) => Err(err),
    }
}

/// `shutil.rmtree(path)`, as the SDK calls it: no `ignore_errors`, `onerror`, `onexc` or
/// `dir_fd`, so the first error is raised.
pub fn rmtree(path: &[u8]) -> Result<(), Error> {
    let mut stack: Vec<RmtreeItem> = vec![(Func::Lstat, None, path.to_vec(), None)];
    let mut result = Ok(());
    while !stack.is_empty() {
        if let Err(err) = rmtree_safe_fd(&mut stack) {
            result = Err(err);
            break;
        }
    }
    // Close any file descriptors still on the stack.
    while let Some((func, fd, _, _)) = stack.pop() {
        if func != Func::Close {
            continue;
        }
        // onexc(os.close, path, err) raises in the `finally`: that error replaces any other.
        if let Err(err) = close(fd.expect("an fd to close")) {
            result = Err(err);
            break;
        }
    }
    result
}
