//! The control-socket types under one name: `std`'s Unix-domain sockets on
//! Unix and the `uds_windows` crate's AF_UNIX sockets on Windows (Windows 10
//! 1809+). Both expose the same `connect` / `pair` / `try_clone` / `shutdown`
//! surface, so the session code is written once against this module.

#[cfg(all(test, unix))]
pub(crate) use std::os::unix::net::UnixListener;
#[cfg(unix)]
pub(crate) use std::os::unix::net::UnixStream;
#[cfg(all(test, windows))]
pub(crate) use uds_windows::UnixListener;
#[cfg(windows)]
pub(crate) use uds_windows::UnixStream;
