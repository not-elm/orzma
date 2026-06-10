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
    ///
    /// # Invariants
    ///
    /// The caller is responsible for OZMUX_* keys (`OZMUX_PANE_ID`,
    /// `OZMUX_SURFACE_ID`, `OZMUX_SESSION_ID`) and for any PATH augmentation.
    ///
    /// **PATH ordering is load-bearing**: if the caller prepends
    /// extension `bin/` directories to PATH, the ozmux `__builtin/`
    /// directory MUST appear FIRST so that built-in shims win over
    /// same-named extension binaries. This responsibility lives with
    /// the caller (see `ozmux_extension_host::path_prefix`).
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
    /// Spawns a shell under a new PTY, builds an alacritty `Term`
    /// matching the same cols/rows, and returns the fully wired
    /// bundle.
    pub fn spawn(opts: SpawnOptions) -> anyhow::Result<Self> {
        let pty_pair = native_pty_system().openpty(PtySize {
            rows: opts.rows,
            cols: opts.cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd = CommandBuilder::new(&opts.shell);
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
