//! The control-plane socket types under one name: `std`'s Unix-domain
//! sockets on Unix and the `uds_windows` crate's AF_UNIX sockets on
//! Windows (Windows 10 1809+). Both expose the same `bind` / `connect` /
//! `incoming` / `try_clone` / `shutdown` surface.

#[cfg(unix)]
pub use std::os::unix::net::{UnixListener, UnixStream};
#[cfg(windows)]
pub use uds_windows::{UnixListener, UnixStream};
