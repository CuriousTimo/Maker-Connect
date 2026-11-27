//! The byte stream under HTTP and the WebSocket: a TCP socket, or ESP-TLS on that socket with the
//! ESP-IDF certificate bundle (public CA roots, as the agent's `rustls-tls-webpki-roots`).

use std::io::{self, Read, Write};
use std::mem::ManuallyDrop;
use std::net::{Shutdown, TcpStream};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, RawFd};

use door_controller::traits::NetError;
use esp_idf_svc::sys::EspError;
use esp_idf_svc::tls::{Config, EspTls, Socket};

/// The socket handed to ESP-TLS; ESP-TLS closes it when the session is dropped (`release`, then
/// `esp_tls_conn_destroy`). When `EspTls::adopt` fails it never takes the socket, so it is closed
/// here on drop instead of leaking.
pub struct Adopted(RawFd);

impl Socket for Adopted {
    fn handle(&self) -> i32 {
        self.0
    }

    fn release(&mut self) -> Result<(), EspError> {
        // ESP-TLS closes it from here on.
        self.0 = -1;
        Ok(())
    }
}

impl Drop for Adopted {
    fn drop(&mut self) {
        if self.0 >= 0 {
            // Never taken by ESP-TLS: ours to close.
            drop(unsafe { TcpStream::from_raw_fd(self.0) });
        }
    }
}

pub enum Stream {
    Plain(TcpStream),
    Tls { fd: RawFd, tls: EspTls<Adopted> },
}

/// mbedTLS `MBEDTLS_ERR_SSL_WANT_READ`, what ESP-TLS returns when a socket read timed out.
const WANT_READ: i32 = -0x6900;

impl Stream {
    pub fn tls(tcp: TcpStream, host: &str) -> Result<Self, NetError> {
        let fd = tcp.into_raw_fd();
        let mut tls = EspTls::adopt(Adopted(fd)).map_err(|_| NetError::Internal)?;
        let cfg = Config {
            common_name: Some(host),
            ..Config::new()
        };
        tls.negotiate(host, &cfg).map_err(|_| NetError::Tls)?;
        Ok(Stream::Tls { fd, tls })
    }

    /// The TCP socket, for timeouts and `peek`.
    pub fn tcp(&mut self) -> ManuallyDrop<TcpStream> {
        let fd = match self {
            Stream::Plain(s) => s.as_raw_fd(),
            Stream::Tls { fd, .. } => *fd,
        };
        // SAFETY: the fd stays open while `self` lives; ManuallyDrop never closes it.
        ManuallyDrop::new(unsafe { TcpStream::from_raw_fd(fd) })
    }

    /// Bytes ESP-TLS has decrypted but not handed out yet.
    pub fn buffered(&self) -> bool {
        match self {
            Stream::Plain(_) => false,
            Stream::Tls { tls, .. } => unsafe {
                esp_idf_svc::sys::esp_tls_get_bytes_avail(tls.context_handle()) > 0
            },
        }
    }

    pub fn shutdown(&mut self) -> io::Result<()> {
        self.tcp().shutdown(Shutdown::Both)
    }
}

fn esp_to_io(e: EspError) -> io::Error {
    if e.code() == WANT_READ {
        return io::Error::from(io::ErrorKind::WouldBlock);
    }
    let os = io::Error::last_os_error();
    match os.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => {
            io::Error::from(io::ErrorKind::WouldBlock)
        }
        _ => io::Error::other(e),
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Stream::Plain(s) => s.read(buf),
            Stream::Tls { tls, .. } => tls.read(buf).map_err(esp_to_io),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Stream::Plain(s) => s.write(buf),
            Stream::Tls { tls, .. } => tls.write(buf).map_err(esp_to_io),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Stream::Plain(s) => s.flush(),
            Stream::Tls { .. } => Ok(()),
        }
    }
}
