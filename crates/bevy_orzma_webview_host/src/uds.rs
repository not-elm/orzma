//! The control-plane socket types under one name: `std`'s Unix-domain
//! sockets on Unix and `uds_windows`' AF_UNIX sockets on Windows
//! (Windows 10 1809+), which expose the same surface.

#[cfg(unix)]
pub use std::os::unix::net::{UnixListener, UnixStream};
#[cfg(windows)]
pub use uds_windows::{UnixListener, UnixStream};
