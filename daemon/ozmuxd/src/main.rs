//! ozmuxd entrypoint: resolves the socket path, runs the daemon, and blocks.
//!
//! SIGINT hard-terminates the process (no graceful handler — avoids an extra
//! dependency). The next start unlinks the stale socket before bind (see
//! `Server::serve`), so a restart is always safe.

use ozmuxd::{Server, default_socket_path};
use std::path::PathBuf;

fn main() -> std::io::Result<()> {
    let path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(default_socket_path);
    let _handle = Server::new().serve(&path)?;
    eprintln!("ozmuxd listening on {}", path.display());
    loop {
        std::thread::park();
    }
}
