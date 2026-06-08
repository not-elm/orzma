//! ozmuxd entrypoint: resolves the socket path, runs the daemon, and blocks
//! until shutdown is requested.
//!
//! SIGINT/SIGTERM are handled gracefully: the handler flips an atomic flag, the
//! main loop observes it and drops the `ServerHandle`, which stops accepting,
//! drains per-connection threads, shuts down the central loop, and unlinks the
//! socket. The loop also polls `ServerHandle::shutdown_requested`, the seam a
//! wire-initiated shutdown will set to exit through the same path.

use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, sigaction};
use ozmuxd::{Server, default_socket_path};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

static SIGNAL_SHUTDOWN: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: i32) {
    SIGNAL_SHUTDOWN.store(true, Ordering::SeqCst);
}

fn install_signal_handlers() {
    let action = SigAction::new(
        SigHandler::Handler(on_signal),
        SaFlags::empty(),
        SigSet::empty(),
    );
    // SAFETY: on_signal only stores to a static AtomicBool, which is async-signal-safe.
    unsafe {
        let _ = sigaction(Signal::SIGINT, &action);
        let _ = sigaction(Signal::SIGTERM, &action);
    }
}

fn main() -> std::io::Result<()> {
    let path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(default_socket_path);
    install_signal_handlers();
    let handle = Server::new().serve(&path)?;
    eprintln!("ozmuxd listening on {}", path.display());
    while !SIGNAL_SHUTDOWN.load(Ordering::SeqCst) && !handle.shutdown_requested() {
        std::thread::park_timeout(Duration::from_millis(200));
    }
    drop(handle);
    Ok(())
}
