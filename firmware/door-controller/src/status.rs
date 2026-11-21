//! Port of `status.hpp` and `status.cpp`.

use crate::traits::NetError;

/// Buddy 6.4.0 `status.hpp:12-22`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnlineError {
    NoError,
    Dns,
    Connection,
    Tls,
    Auth,
    Server,
    Internal,
    Network,
    Confused,
}

/// Buddy 6.4.0 `status.hpp:25-36`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionStatus {
    Unknown,
    Off,
    NoConfig,
    Ok,
    Connecting,
    Error,
    RegistrationRequesting,
    RegistrationCode,
    RegistrationDone,
    RegistrationError,
}

/// Buddy 6.4.0 `status.hpp:39-44`. `retry`: `None` is no limit, `Some(0)` runs out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ErrWithRetry {
    pub err: OnlineError,
    pub retry: Option<u8>,
}

/// Buddy 6.4.0 `status.hpp:49` (`std::variant`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommResult {
    Nothing,
    Status(ConnectionStatus),
    Error(OnlineError),
    ErrWithRetry(ErrWithRetry),
}

/// Buddy 6.4.0 `status.cpp:7-29` `err_to_status`.
pub fn err_to_status(error: NetError) -> OnlineError {
    match error {
        NetError::Connect => OnlineError::Connection,
        NetError::Dns => OnlineError::Dns,
        NetError::Internal => OnlineError::Internal,
        NetError::Network | NetError::Timeout | NetError::UnexpectedEof => OnlineError::Network,
        NetError::Parse => OnlineError::Confused,
        NetError::Tls => OnlineError::Tls,
    }
}
