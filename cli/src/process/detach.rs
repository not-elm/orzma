//! Detach-on-Unix helper for child processes that should outlive the CLI.
//!
//! Calls `setsid(2)` between fork and exec so the child becomes its own
//! session leader and is not killed when the CLI's controlling tty goes
//! away. Used by `ozmux daemon start` (for the daemon) and `ozmux session
//! new --open` (for the Tauri client).

use std::io;
use std::os::unix::process::CommandExt;
use std::process::Command;

/// Configure `cmd` so that, on Unix, the child process detaches from the
/// CLI's session via `setsid(2)`. Callers are expected to also redirect
/// stdio (typically `Stdio::null()`) before calling `spawn()`.
pub(crate) fn configure_detached(cmd: &mut Command) {
    // SAFETY: setsid is async-signal-safe (POSIX.1-2008 Table 2-5) and the
    // closure runs between fork and exec where no Rust destructors fire.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
}
