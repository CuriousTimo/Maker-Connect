use super::*;
use crate::ports::prusalink::python;
use serde_json::{Value, json};

/// The pieces the printer's end sends and the port writes: a full line, CR LF, a line in two,
/// an empty line, XON/XOFF and bytes over 0x7f (the cleared `IXON`, `ICRNL`, `ISTRIP`, `OPOST`).
const PIECES: [&str; 8] = [
    "ok T:20.0\n",
    "echo:busy: processing\r\n",
    "part",
    "ial\n",
    "\n",
    "\u{11}\u{13}x\n",
    "\u{ff}\u{80}\u{0}\n",
    "\r",
];

fn scripts() -> Vec<Value> {
    let mut seed: u64 = 243;
    let mut next = |n: u64| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) % n
    };
    let mut scripts = vec![];
    for round in 0..6 {
        // a line in two pieces, the first timing out
        let mut ops = match round {
            0 => vec![
                json!(["send", "part"]),
                json!(["readline"]),
                json!(["send", "ial\n"]),
                json!(["readline"]),
            ],
            _ => vec![],
        };
        // after a hangup nothing more is sent or received, after a close nothing received
        let (mut hung, mut closed) = (false, false);
        for _ in 0..40 {
            match next(24) {
                0..=6 if !hung => ops.push(json!(["send", PIECES[next(8) as usize]])),
                7..=13 => ops.push(json!(["readline"])),
                14..=16 => {
                    ops.push(json!(["write", PIECES[next(8) as usize]]));
                    if !hung && !closed {
                        ops.push(json!(["receive"]));
                    }
                }
                17..=19 => ops.push(json!(["dtr", next(2) == 0])),
                20..=21 => ops.push(json!(["state"])),
                // the end of a round: the other end hangs up, or the port is closed
                22..=23 if round % 2 == 0 && !hung => {
                    hung = true;
                    ops.push(json!(["hangup"]));
                }
                22..=23 => {
                    closed = true;
                    ops.push(json!(["close"]));
                }
                _ => ops.push(json!(["state"])),
            }
        }
        scripts.push(json!({"path": "pty", "baudrate": 115200, "timeout": 0.05, "ops": ops}));
    }
    for path in ["missing", "file", "dir", "pty"] {
        for baudrate in [115200, 9600] {
            scripts.push(json!({"path": path, "baudrate": baudrate, "timeout": 0.05, "ops": []}));
        }
    }
    scripts
}

fn latin1(data: &[u8]) -> Value {
    json!(data.iter().map(|b| *b as char).collect::<String>())
}

fn bytes(text: &Value) -> Vec<u8> {
    text.as_str().unwrap().chars().map(|c| c as u8).collect()
}

fn error(error: PyErr) -> Value {
    json!([error.class_name(), error.message()])
}

fn attributes(fd: c_int) -> Value {
    let tty = Posix.tcgetattr(fd).unwrap();
    json!([
        tty.iflag, tty.oflag, tty.cflag, tty.lflag, tty.ispeed, tty.ospeed, tty.cc
    ])
}

/// The master's end: `os.write` (until the bytes are there, up to 1 s), `select` of 1 s then `os.read(1024)`, `os.close`, then up to
/// 1 s until the kernel has hung the terminal up.
fn op_master(op: &str, master: &mut Option<c_int>, slave: c_int, data: &Value) -> Value {
    let fd = master.unwrap();
    match op {
        "send" => {
            // the kernel passes the bytes on in its own time: wait until they are there
            let there = || {
                let mut n: c_int = 0;
                unsafe { libc::ioctl(slave, libc::FIONREAD, &mut n) };
                n as usize
            };
            let waiting = there();
            let sent = Posix.write(Some(fd), &bytes(data)).unwrap();
            for _ in 0..1000 {
                if there() >= waiting + sent {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            json!(sent)
        }
        "receive" => {
            let got = match Posix.select(Some(fd), 1.0).unwrap() {
                true => Posix.read(Some(fd), 1024).unwrap(),
                false => vec![],
            };
            latin1(&got)
        }
        _ => {
            Posix.close(fd).unwrap();
            *master = None;
            // the kernel hangs the terminal up after the close, in its own time: wait until it
            // has (an empty write fails), up to 1 s
            for _ in 0..1000 {
                if Posix.write(Some(slave), b"").is_err() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Value::Null
        }
    }
}

fn run(script: &Value, directory: &str) -> Value {
    let (mut master, mut slave) = (None, None);
    let path = match script["path"].as_str().unwrap() {
        "pty" => {
            let (mut m, mut s) = (0, 0);
            let made = unsafe {
                libc::openpty(
                    &mut m,
                    &mut s,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                )
            };
            assert_eq!(made, 0);
            (master, slave) = (Some(m), Some(s));
            let name = unsafe { std::ffi::CStr::from_ptr(libc::ttyname(s)) };
            name.to_str().unwrap().to_owned()
        }
        "missing" => format!("{directory}/missing"),
        "file" => format!("{directory}/file"),
        _ => directory.to_owned(),
    };
    let mut done = vec![];
    let opened = match Serial::new(
        &path,
        script["baudrate"].as_i64().unwrap(),
        script["timeout"].as_f64().unwrap(),
    ) {
        Ok(mut port) => {
            let opened = attributes(port.fd.unwrap());
            for op in script["ops"].as_array().unwrap() {
                let result = match op[0].as_str().unwrap() {
                    name @ ("send" | "receive" | "hangup") => {
                        op_master(name, &mut master, slave.unwrap(), &op[1])
                    }
                    "readline" => port
                        .readline()
                        .map(|line| latin1(&line))
                        .unwrap_or_else(error),
                    "write" => port
                        .write(&bytes(&op[1]))
                        .map(|n| json!(n))
                        .unwrap_or_else(error),
                    "dtr" => port
                        .set_dtr(op[1].as_bool().unwrap())
                        .map(|()| Value::Null)
                        .unwrap_or_else(error),
                    "close" => {
                        port.close();
                        Value::Null
                    }
                    _ => json!([port.is_open(), port.dtr()]),
                };
                done.push(result);
            }
            port.close();
            opened
        }
        Err(e) => error(e),
    };
    for fd in [master, slave].into_iter().flatten() {
        let _ = Posix.close(fd);
    }
    json!([opened, done])
}

/// PrusaLink's own `Serial` and ours over pseudo terminals, the same scripts: the terminal's
/// attributes after the open (or the open's exception: an unknown baudrate, a missing path, a file
/// and a directory), and each op's value or exception, as the printer's end sends lines (in
/// pieces, CR LF, XON/XOFF, bytes over 0x7f), the port reads and writes them, DTR is set, and the
/// other end hangs up or the port is closed.
#[test]
fn identical_to_prusalink() {
    let directory = std::env::temp_dir().join(format!("mc-serial-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("file"), "").unwrap();
    let directory_str = directory.to_str().unwrap().to_owned();
    let scripts = scripts();
    let theirs = python::run(
        include_str!("../driver_serial.py"),
        &json!({"directory": directory_str, "scripts": scripts}),
    );
    let ours: Vec<Value> = scripts.iter().map(|s| run(s, &directory_str)).collect();
    std::fs::remove_dir_all(&directory).unwrap();
    let Some(theirs) = theirs else {
        return;
    };
    let theirs = theirs.as_array().unwrap();
    assert_eq!(theirs.len(), scripts.len());
    for ((script, ours), theirs) in scripts.iter().zip(&ours).zip(theirs) {
        assert_eq!(
            serde_json::to_string(ours).unwrap(),
            serde_json::to_string(theirs).unwrap(),
            "{script}"
        );
    }
    // the walk reaches lines read, a timed out part, and both ends of a round
    let all = serde_json::to_string(&ours).unwrap();
    for seen in ["ok T:20.0\\n", "partial\\n", "SerialException", "TypeError"] {
        assert!(all.contains(seen), "{seen}");
    }
}

// PrusaLink's own tests (`tests/test_serial.py`, `171148ce`), over a pseudo terminal; their
// `patch.object(module...)` is a `Patched` `Os`, the real calls for the rest.

/// What a test replaces, as its `patch.object`: `None` is the real call.
#[derive(Default)]
struct Patch {
    opened: usize,
    tcgetattr_zero: bool,
    tcsetattr_noop: bool,
    tcflush_noop: bool,
    select_interrupted: bool,
    ioctl_eio: bool,
    ioctl_calls: Option<Vec<c_ulong>>,
}

#[derive(Clone, Default)]
struct Patched(std::sync::Arc<std::sync::Mutex<Patch>>);

impl Os for Patched {
    fn open(&self, path: &str, flags: c_int) -> Result<c_int, PyErr> {
        self.0.lock().unwrap().opened += 1;
        Posix.open(path, flags)
    }
    fn tcgetattr(&self, fd: c_int) -> Result<Tty, PyErr> {
        match self.0.lock().unwrap().tcgetattr_zero {
            // `return_value=[0, 0, 0, 0, 0, 0, [0] * 32]`
            true => Ok(Tty {
                iflag: 0,
                oflag: 0,
                cflag: 0,
                lflag: 0,
                ispeed: 0,
                ospeed: 0,
                cc: vec![0; 32],
            }),
            false => Posix.tcgetattr(fd),
        }
    }
    fn tcsetattr(&self, fd: c_int, when: c_int, tty: &Tty) -> Result<(), PyErr> {
        match self.0.lock().unwrap().tcsetattr_noop {
            true => Ok(()),
            false => Posix.tcsetattr(fd, when, tty),
        }
    }
    fn tcflush(&self, fd: c_int, queue: c_int) -> Result<(), PyErr> {
        match self.0.lock().unwrap().tcflush_noop {
            true => Ok(()),
            false => Posix.tcflush(fd, queue),
        }
    }
    fn ioctl(&self, fd: Option<c_int>, request: c_ulong, arg: u32) -> Result<(), PyErr> {
        let mut patch = self.0.lock().unwrap();
        if patch.ioctl_eio {
            // `side_effect=OSError(module.errno.EIO, "io")`
            return Err(PyErr::Other("OSError", format!("[Errno {}] io", libc::EIO)));
        }
        match &mut patch.ioctl_calls {
            Some(calls) => {
                calls.push(request);
                Ok(())
            }
            None => Posix.ioctl(fd, request, arg),
        }
    }
    fn select(&self, fd: Option<c_int>, timeout: f64) -> Result<bool, PyErr> {
        match self.0.lock().unwrap().select_interrupted {
            // `side_effect=InterruptedError("x")`
            true => Err(PyErr::Other("InterruptedError", "x".to_owned())),
            false => Posix.select(fd, timeout),
        }
    }
    fn read(&self, fd: Option<c_int>, n: usize) -> Result<Vec<u8>, PyErr> {
        Posix.read(fd, n)
    }
    fn write(&self, fd: Option<c_int>, data: &[u8]) -> Result<usize, PyErr> {
        Posix.write(fd, data)
    }
    fn close(&self, fd: c_int) -> Result<(), PyErr> {
        Posix.close(fd)
    }
}

/// `pair`: the serial port and the other end of the pseudo terminal
struct Pair {
    port: Serial,
    master: c_int,
    slave: c_int,
    patch: Patched,
}

fn pair() -> Pair {
    let (mut master, mut slave) = (0, 0);
    let made = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    assert_eq!(made, 0);
    let name = unsafe { std::ffi::CStr::from_ptr(libc::ttyname(slave)) };
    let patch = Patched::default();
    let port =
        Serial::with_os(Box::new(patch.clone()), name.to_str().unwrap(), 115200, 0.2).unwrap();
    Pair {
        port,
        master,
        slave,
        patch,
    }
}

impl Drop for Pair {
    fn drop(&mut self) {
        self.port.close();
        for descriptor in [self.master, self.slave] {
            let _ = Posix.close(descriptor);
        }
    }
}

fn send(master: c_int, data: &[u8]) {
    Posix.write(Some(master), data).unwrap();
}

/// `tmp_path / "file"`, empty
fn regular_file(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("mc-serial-{}-{name}", std::process::id()));
    std::fs::write(&path, "").unwrap();
    path
}

/// Only the known speed works
#[test]
fn test_unsupported_baudrate() {
    let patch = Patched::default();
    let error = Serial::with_os(Box::new(patch.clone()), "/dev/null", 9600, 1.0)
        .err()
        .unwrap();
    assert_eq!(error.class_name(), "SerialException");
    assert_eq!(patch.0.lock().unwrap().opened, 0); // rejected before anything is opened
}

/// The port is open until closed, closing twice is fine
#[test]
fn test_open_and_close() {
    let mut pair = pair();
    assert!(pair.port.is_open());
    pair.port.close();
    assert!(!pair.port.is_open());
    pair.port.close();
}

/// A line sent by the printer is returned with the newline
#[test]
fn test_read_line() {
    let mut pair = pair();
    send(pair.master, b"ok T:20.0\n");
    assert_eq!(pair.port.readline().unwrap(), b"ok T:20.0\n");
}

/// More lines at once are returned one by one
#[test]
fn test_read_buffers_the_rest() {
    let mut pair = pair();
    send(pair.master, b"one\ntwo\npart");
    assert_eq!(pair.port.readline().unwrap(), b"one\n");
    assert_eq!(pair.port.readline().unwrap(), b"two\n");
    assert_eq!(pair.port.readline().unwrap(), b""); // incomplete line times out
    send(pair.master, b"ial\n");
    assert_eq!(pair.port.readline().unwrap(), b"partial\n");
}

/// Nothing arrives, empty bytes are returned
#[test]
fn test_read_timeout() {
    let mut pair = pair();
    assert_eq!(pair.port.readline().unwrap(), b"");
}

/// Data go to the other end
#[test]
fn test_write() {
    let pair = pair();
    assert_eq!(pair.port.write(b"G28\n").unwrap(), 4);
    assert_eq!(Posix.read(Some(pair.master), 100).unwrap(), b"G28\n");
}

/// The disconnected port raises
#[test]
fn test_read_after_the_other_end_closed() {
    let mut pair = pair();
    Posix.close(pair.master).unwrap();
    pair.master = -1;
    let error = pair.port.readline().err().unwrap();
    assert_eq!(error.class_name(), "SerialException");
}

/// Interrupted read closes the port
#[test]
fn test_read_failure_closes_the_port() {
    let mut pair = pair();
    pair.patch.0.lock().unwrap().select_interrupted = true;
    let error = pair.port.readline().err().unwrap();
    assert_eq!(error.class_name(), "SerialException");
    assert!(!pair.port.is_open());
}

/// The flag is remembered, the line is toggled
#[test]
fn test_dtr() {
    let mut pair = pair();
    assert!(!pair.port.dtr());
    pair.patch.0.lock().unwrap().ioctl_calls = Some(vec![]);
    pair.port.set_dtr(true).unwrap();
    pair.port.set_dtr(false).unwrap();
    assert!(!pair.port.dtr());
    let calls = pair.patch.0.lock().unwrap().ioctl_calls.take().unwrap();
    assert_eq!(
        calls,
        [libc::TIOCMBIS as c_ulong, libc::TIOCMBIC as c_ulong]
    );
}

fn file_patch() -> Patched {
    let patch = Patched::default();
    {
        let mut p = patch.0.lock().unwrap();
        p.tcgetattr_zero = true;
        p.tcsetattr_noop = true;
        p.tcflush_noop = true;
    }
    patch
}

/// A regular file is accepted, the missing ioctl is only a warning
#[test]
fn test_file_without_ioctl() {
    let path = regular_file("without-ioctl");
    let mut port =
        Serial::with_os(Box::new(file_patch()), path.to_str().unwrap(), 115200, 1.0).unwrap();
    assert!(port.is_open());
    port.close();
    std::fs::remove_file(path).unwrap();
}

/// Only ENOTTY is ignored
#[test]
fn test_other_ioctl_errors_are_raised() {
    let path = regular_file("other-ioctl");
    let patch = file_patch();
    patch.0.lock().unwrap().ioctl_eio = true;
    let error = Serial::with_os(Box::new(patch), path.to_str().unwrap(), 115200, 1.0)
        .err()
        .unwrap();
    assert_eq!(error.class_name(), "OSError");
    std::fs::remove_file(path).unwrap();
}
