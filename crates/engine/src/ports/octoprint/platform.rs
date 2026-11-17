//! Port of OctoPrint 1.11.8 `src/octoprint/util/platform/__init__.py`: the two helpers
//! `comm.py` takes from it (`get_os`, `set_close_exec`).
//!
//! OctoPrint is Copyright (C) 2017 The OctoPrint Project, AGPL-3.0.

/// `platform/__init__.py:64` `_OPERATING_SYSTEMS` as `sys.platform` of CPython 3.13 gives it:
/// `win32`, `linux*`, `darwin`, `freebsd*`, `android`.
///
/// `platform/__init__.py:73 get_os()`: a canonical OS identifier, `"unmapped"` for any other.
pub fn get_os() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "freebsd") {
        "freebsd"
    } else if cfg!(target_os = "android") {
        "android"
    } else {
        "unmapped"
    }
}

/// `platform/__init__.py:16 set_close_exec(handle)` (the `fcntl` variant, used where `fcntl` has
/// `FD_CLOEXEC`): "Set ``close_exec`` flag on handle, if supported by the OS." `Err` is the
/// `OSError` of a failed `fcntl`.
///
/// The Windows variant (:26, `SetHandleInformation`) is not ported: the engine is built for Unix
/// systems only (`serial.rs`).
#[cfg(unix)]
pub fn set_close_exec(handle: std::os::fd::RawFd) -> Result<(), std::io::Error> {
    // SAFETY: fcntl on an integer descriptor with F_GETFD / F_SETFD reads and writes nothing else.
    unsafe {
        let mut flags = libc::fcntl(handle, libc::F_GETFD);
        if flags < 0 {
            return Err(std::io::Error::last_os_error());
        }
        flags |= libc::FD_CLOEXEC;
        if libc::fcntl(handle, libc::F_SETFD, flags) < 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! OURS: OctoPrint has no test of these two functions.
    use super::*;

    #[test]
    fn get_os_is_a_known_name() {
        assert!(
            [
                "windows", "linux", "macos", "freebsd", "android", "unmapped"
            ]
            .contains(&get_os())
        );
        #[cfg(target_os = "macos")]
        assert_eq!("macos", get_os());
        #[cfg(target_os = "linux")]
        assert_eq!("linux", get_os());
    }

    #[cfg(unix)]
    #[test]
    fn set_close_exec_sets_the_flag() {
        use std::os::fd::AsRawFd;
        let f = std::fs::File::open("/dev/null").unwrap();
        // std opens with O_CLOEXEC; clear it first, then set it through the port
        unsafe {
            libc::fcntl(f.as_raw_fd(), libc::F_SETFD, 0);
            assert_eq!(0, libc::fcntl(f.as_raw_fd(), libc::F_GETFD));
        }
        set_close_exec(f.as_raw_fd()).unwrap();
        assert_eq!(libc::FD_CLOEXEC, unsafe {
            libc::fcntl(f.as_raw_fd(), libc::F_GETFD)
        });
        assert!(set_close_exec(-1).is_err());
    }
}
