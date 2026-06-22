//! `TerminalBundle` — single-shot constructor that opens a PTY,
//! spawns the child shell, builds the alacritty `Term`, and produces
//! a coherent set of Components.
//!
//! Passing `cols`/`rows` exactly once at construction makes the
//! PTY-grid mismatch unrepresentable.

use crate::coalescer::Coalescer;
use crate::handle::TerminalHandle;
use crate::pty::{PtyHandle, spawn_pty_thread};
use crate::title::TerminalTitle;
use crate::vt::listener::{ControlFrame, TermListener};
use bevy::ecs::bundle::Bundle;
use crossbeam_channel::unbounded;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// Spawn parameters consumed exactly once by `TerminalBundle::spawn`.
pub struct SpawnOptions {
    /// Terminal column count.
    pub cols: u16,
    /// Terminal row count.
    pub rows: u16,
    /// Shell program to launch (absolute path or `$PATH`-resolvable name).
    pub shell: String,
    /// Initial working directory for the spawned shell.
    pub cwd: Option<PathBuf>,
    /// Arbitrary environment variables forwarded to the shell.
    pub env: Vec<(String, String)>,
    /// Shared gate controlling whether OSC 5379 webview sequences are processed.
    /// Off by default — callers that want webview support enable this at spawn time.
    pub osc_webview_gate: Arc<AtomicBool>,
}

/// All four Components a working terminal entity needs.
#[derive(Bundle)]
pub struct TerminalBundle {
    pub handle: TerminalHandle,
    pub pty: PtyHandle,
    pub coalescer: Coalescer,
    pub title: TerminalTitle,
}

impl TerminalBundle {
    /// Spawns the shell directly (its argv0 is the shell path) under a new PTY,
    /// builds an alacritty `Term` matching the same cols/rows, and returns the
    /// fully wired bundle.
    pub fn spawn(opts: SpawnOptions) -> anyhow::Result<Self> {
        Self::spawn_inner(opts, false)
    }

    /// Spawns the shell as a LOGIN shell, otherwise identical to
    /// [`TerminalBundle::spawn`].
    ///
    /// On macOS this wraps the shell in `/usr/bin/login` so it sources
    /// `/etc/zprofile` (`path_helper`) and `~/.zprofile` (e.g. `brew shellenv`).
    /// Without that, a `.app` launched from Finder runs under launchd's minimal
    /// `PATH`, leaving `/opt/homebrew/bin` off `PATH` so user tools (nvim,
    /// Homebrew) are not found. Falls back to a direct spawn on other platforms
    /// and when `$USER`/`$HOME` are unavailable.
    pub fn spawn_login_shell(opts: SpawnOptions) -> anyhow::Result<Self> {
        Self::spawn_inner(opts, true)
    }

    fn spawn_inner(opts: SpawnOptions, login_shell: bool) -> anyhow::Result<Self> {
        let pty_pair = native_pty_system().openpty(PtySize {
            rows: opts.rows,
            cols: opts.cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd = build_shell_command(&opts.shell, login_shell);
        if let Some(cwd) = opts.cwd.as_ref() {
            cmd.cwd(cwd);
        }
        for (k, v) in &opts.env {
            cmd.env(k, v);
        }

        let child = pty_pair.slave.spawn_command(cmd)?;
        let child_killer = child.clone_killer();
        drop(pty_pair.slave);

        let reader = pty_pair.master.try_clone_reader()?;
        let writer = pty_pair.master.take_writer()?;

        let (chunk_tx, chunk_rx) = unbounded::<Vec<u8>>();
        let (exit_tx, exit_rx) = unbounded::<Option<i32>>();
        let (reply_tx, reply_rx) = unbounded::<Vec<u8>>();
        let (control_tx, control_rx) = unbounded::<ControlFrame>();

        spawn_pty_thread(reader, child, chunk_tx, exit_tx);

        let listener = TermListener {
            reply_tx,
            control_tx: control_tx.clone(),
        };
        let handle = TerminalHandle::new(
            opts.cols,
            opts.rows,
            listener,
            reply_rx,
            control_rx,
            control_tx,
            opts.osc_webview_gate,
        );

        let pty = PtyHandle::new(pty_pair.master, writer, chunk_rx, exit_rx, child_killer);

        Ok(Self {
            handle,
            pty,
            coalescer: Coalescer::new(),
            title: TerminalTitle::default(),
        })
    }
}

/// Builds the shell `CommandBuilder`: on macOS the login-shell case wraps the
/// shell in `/usr/bin/login`; every other case spawns the shell directly.
fn build_shell_command(shell: &str, login_shell: bool) -> CommandBuilder {
    #[cfg(target_os = "macos")]
    if login_shell {
        return macos_login_command(shell);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = login_shell;
    CommandBuilder::new(shell)
}

/// `/usr/bin/login -flp <user> /bin/zsh -fc "exec -a -<name> <shell>"` — runs
/// `shell` as a macOS login shell (so it sources the login profile). Falls back
/// to a direct spawn when `$USER`/`$HOME` are unavailable.
#[cfg(target_os = "macos")]
fn macos_login_command(shell: &str) -> CommandBuilder {
    let user = std::env::var("USER").ok().filter(|s| !s.is_empty());
    let home = std::env::var("HOME").ok().filter(|s| !s.is_empty());
    let (Some(user), Some(home)) = (user, home) else {
        return CommandBuilder::new(shell);
    };
    let shell_name = shell.rsplit('/').next().unwrap_or(shell);
    let exec = format!("exec -a -{shell_name} {shell}");
    // NOTE: the inner shell must be zsh/bash, not sh — `exec -a` (which sets
    // argv0 to `-<name>` to make a login shell) is unavailable in POSIX sh.
    let flags = if PathBuf::from(&home).join(".hushlogin").exists() {
        "-qflp"
    } else {
        "-flp"
    };
    let mut cmd = CommandBuilder::new("/usr/bin/login");
    cmd.args([flags, user.as_str(), "/bin/zsh", "-fc", exec.as_str()]);
    cmd
}
