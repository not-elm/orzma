//! `Pty` — owns the PTY master, writer, child killer, and the
//! channels fed by the blocking-read OS thread for one spawned shell.

use crate::{
    SpawnOptions,
    error::{OrzmaTermError, OrzmaTermResult},
};
use crossbeam_channel::{Receiver, Sender, unbounded};
use portable_pty::{Child, ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
#[cfg(target_os = "macos")]
use std::path::PathBuf;
use std::sync::Mutex;
use std::thread;

/// PTY ownership for one spawned shell.
///
/// `Mutex` is required because `dyn MasterPty + Send` and `dyn Write +
/// Send` are `!Sync`, while downstream wrappers (`bevy_orzma_term`'s
/// `Component`) need the owning `OrzmaTerm` to be `Send + Sync`.
pub struct Pty {
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    chunk_rx: Receiver<Vec<u8>>,
    exit_rx: Receiver<Option<i32>>,
    child_killer: Box<dyn ChildKiller + Send + Sync>,
}

impl Pty {
    /// Opens a PTY at the given grid size, spawns `options.shell` under
    /// it as a login shell, and starts the blocking reader/wait OS
    /// thread.
    pub fn spawn(options: &SpawnOptions) -> OrzmaTermResult<Self> {
        let pty_pair = native_pty_system()
            .openpty(PtySize {
                rows: options.rows,
                cols: options.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(OrzmaTermError::PtyOpen)?;

        let mut cmd = build_shell_command(&options.shell);
        if let Some(cwd) = options.cwd.as_ref() {
            cmd.cwd(cwd);
        }
        for (key, value) in &options.env {
            cmd.env(&key.0, &value.0);
        }

        let child = pty_pair
            .slave
            .spawn_command(cmd)
            .map_err(OrzmaTermError::SpawnShell)?;
        let mut child_killer = child.clone_killer();
        // NOTE: the slave must be dropped here so the reader sees EOF
        // when the child exits.
        drop(pty_pair.slave);

        let (reader, writer) = match master_pipes(pty_pair.master.as_ref()) {
            Ok(pipes) => pipes,
            Err(e) => {
                // NOTE: the child is already running at this point; kill
                // it or it leaks past the failed spawn.
                let _ = child_killer.kill();
                return Err(OrzmaTermError::PtyPipe(e));
            }
        };

        let (chunk_tx, chunk_rx) = unbounded::<Vec<u8>>();
        let (exit_tx, exit_rx) = unbounded::<Option<i32>>();
        spawn_reader_thread(reader, child, chunk_tx, exit_tx);

        Ok(Self {
            master: Mutex::new(pty_pair.master),
            writer: Mutex::new(writer),
            chunk_rx,
            exit_rx,
            child_killer,
        })
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        // NOTE: SIGHUP the child so the blocking reader thread's read()
        // returns EOF and exits cleanly. portable-pty's ChildKiller
        // makes this idempotent — kill() on an already-exited child is
        // a no-op.
        let _ = self.child_killer.kill();
    }
}

/// Builds the shell `CommandBuilder`: on macOS the shell is wrapped in
/// `/usr/bin/login` so it runs as a login shell and sources
/// `/etc/zprofile` (`path_helper`) and `~/.zprofile` — without that, an
/// app launched from Finder runs under launchd's minimal `PATH`. Every
/// other platform spawns the shell directly.
fn build_shell_command(shell: &str) -> CommandBuilder {
    #[cfg(target_os = "macos")]
    {
        macos_login_command(shell)
    }
    #[cfg(not(target_os = "macos"))]
    {
        CommandBuilder::new(shell)
    }
}

/// `/usr/bin/login -flp <user> /bin/zsh -fc "exec -a -<name> <shell>"` —
/// runs `shell` as a macOS login shell (so it sources the login
/// profile). Falls back to a direct spawn when `$USER`/`$HOME` are
/// unavailable.
#[cfg(target_os = "macos")]
fn macos_login_command(shell: &str) -> CommandBuilder {
    let user = std::env::var("USER").ok().filter(|s| !s.is_empty());
    let home = std::env::var("HOME").ok().filter(|s| !s.is_empty());
    let (Some(user), Some(home)) = (user, home) else {
        return CommandBuilder::new(shell);
    };
    let shell_name = shell.rsplit('/').next().unwrap_or(shell);
    let exec = format!("exec -a -{shell_name} {shell}");
    // NOTE: the inner shell must be zsh/bash, not sh — `exec -a` (which
    // sets argv0 to `-<name>` to make a login shell) is unavailable in
    // POSIX sh.
    let flags = if PathBuf::from(&home).join(".hushlogin").exists() {
        "-qflp"
    } else {
        "-flp"
    };
    let mut cmd = CommandBuilder::new("/usr/bin/login");
    cmd.args([flags, user.as_str(), "/bin/zsh", "-fc", exec.as_str()]);
    cmd
}

fn master_pipes(
    master: &dyn MasterPty,
) -> anyhow::Result<(Box<dyn Read + Send>, Box<dyn Write + Send>)> {
    Ok((master.try_clone_reader()?, master.take_writer()?))
}

/// Spawns a dedicated OS thread that drains PTY output into `chunk_tx`
/// and sends a single `exit_tx` message (`Some(code)` on graceful exit,
/// `None` on wait failure) once the reader returns 0 or errors out.
///
/// The OS thread (vs. an async task) is required because the PTY read
/// syscall is blocking.
fn spawn_reader_thread(
    mut reader: Box<dyn Read + Send>,
    mut child: Box<dyn Child + Send + Sync>,
    chunk_tx: Sender<Vec<u8>>,
    exit_tx: Sender<Option<i32>>,
) {
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    // NOTE: a send failure means the receiver — and thus
                    // the whole `Pty`, whose `Drop` already killed the
                    // child — is gone. `break` (not `return`) so the
                    // `child.wait()` below still reaps the child.
                    if chunk_tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let code = child.wait().ok().map(|s| s.exit_code() as i32);
        let _ = exit_tx.send(code);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn spawn_emits_chunk_and_exit_zero() {
        let pty = Pty::spawn(&SpawnOptions {
            cols: 80,
            rows: 24,
            shell: "/bin/echo".into(),
            cwd: None,
            env: Vec::new(),
        })
        .expect("Pty::spawn failed");
        let chunk = pty
            .chunk_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("no PTY chunk arrived");
        assert!(!chunk.is_empty());
        let code = pty
            .exit_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("no exit code arrived");
        assert_eq!(code, Some(0));
    }
}
