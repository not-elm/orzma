//! ozmuxd entrypoint: resolves the socket path, runs the daemon, and blocks
//! until shutdown is requested.
//!
//! SIGINT/SIGTERM are handled gracefully: the handler flips an atomic flag, the
//! main loop observes it and drops the `ServerHandle`, which stops accepting,
//! drains per-connection threads, shuts down the central loop, and unlinks the
//! socket. The loop also polls `ServerHandle::shutdown_requested`, the seam a
//! wire-initiated shutdown will set to exit through the same path.

use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, sigaction};
use ozmux_proto::{ClientMessage, PROTOCOL_VERSION, write_message};
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
    let arg1 = std::env::args().nth(1);
    if arg1.as_deref() == Some("--kill") {
        let path = std::env::args()
            .nth(2)
            .map(PathBuf::from)
            .unwrap_or_else(default_socket_path);
        return kill_running_daemon(path);
    }
    let path = arg1.map(PathBuf::from).unwrap_or_else(default_socket_path);
    install_signal_handlers();
    let handle = Server::new().serve(&path)?;
    eprintln!("ozmuxd listening on {}", path.display());
    while !SIGNAL_SHUTDOWN.load(Ordering::SeqCst) && !handle.shutdown_requested() {
        std::thread::park_timeout(Duration::from_millis(200));
    }
    drop(handle);
    Ok(())
}

fn kill_running_daemon(path: PathBuf) -> std::io::Result<()> {
    let mut stream = match std::os::unix::net::UnixStream::connect(&path) {
        Ok(s) => s,
        Err(_) => {
            eprintln!("ozmuxd: no daemon running at {}", path.display());
            return Ok(());
        }
    };
    write_message(
        &mut stream,
        &ClientMessage::Hello {
            protocol_version: PROTOCOL_VERSION,
            viewport: (80, 24),
        },
    )?;
    write_message(&mut stream, &ClientMessage::Shutdown)?;
    eprintln!("ozmuxd: shutdown requested at {}", path.display());
    Ok(())
}
