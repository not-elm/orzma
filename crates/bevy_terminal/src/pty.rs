//! `PtyHandle` — Bevy `Component` newtype wrapping the Bevy-free
//! `ozmux_vt::pty::Pty` I/O core. The wrapper exists only to make the
//! PTY a queryable `Component`; all behaviour lives in `Pty`.

use bevy::ecs::component::Component;
use crossbeam_channel::TryRecvError;
use ozmux_vt::pty::Pty;

/// Per-entity PTY ownership. Constructed by `TerminalBundle::spawn`;
/// every mutator is driven through `TerminalHandle::write` / `resize` to
/// preserve the input-ordering invariants.
#[derive(Component)]
pub struct PtyHandle(pub(crate) Pty);

impl PtyHandle {
    /// Resizes the PTY master.
    pub(crate) fn resize(&mut self, cols: u16, rows: u16) -> anyhow::Result<()> {
        self.0.resize(cols, rows)
    }

    /// Writes bytes to the PTY master.
    pub(crate) fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.0.write_all(bytes)
    }

    /// Non-blocking poll for the next PTY chunk.
    pub(crate) fn try_recv_chunk(&self) -> Result<Vec<u8>, TryRecvError> {
        self.0.try_recv_chunk()
    }

    /// Non-blocking poll for the one-shot child-exit signal.
    pub(crate) fn try_recv_exit(&self) -> Result<Option<i32>, TryRecvError> {
        self.0.try_recv_exit()
    }
}
