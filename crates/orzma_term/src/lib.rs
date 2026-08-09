//! PTY-backed terminal core: spawns the login shell under a PTY and
//! drives an [`OrzmaVt`] behind a frame coalescer.

use crate::{
    coalescer::Coalescer,
    error::{OrzmaTermError, OrzmaTermResult},
    input::{MouseButton, MouseReport, PtyInput, TerminalKey, TerminalModifiers},
    pty::Pty,
    signal::TermSignal,
};
use orzma_vt::prelude::*;
use std::path::PathBuf;

mod coalescer;
mod error;
mod input;
mod pty;
mod signal;

pub mod prelude {
    pub use crate::{OrzmaTerm, error::*, input::*, signal::*};
}

/// Spawn parameters consumed exactly once by `OrzmaTerm::spawn`.
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
    pub env: Vec<(EnvKey, EnvValue)>,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct EnvKey(pub String);

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct EnvValue(pub String);

/// A live terminal: the VT emulation plus the PTY it is wired to.
pub struct OrzmaTerm<V: OrzmaVt> {
    vt: V,
    coalescer: Coalescer,
    /// One-shot latch: set immediately before any user-originated PTY
    /// write, cleared when the coalescer consumes it.
    ///
    /// While set it *unlocks* — but does not by itself trigger — the
    /// immediate-flush path. [`Coalescer::should_flush_immediately`]
    /// returns `false` outright while this is `false`; only
    /// `AtMostOneRow`, or `ManyRows` under the row cap with the window
    /// closed, actually bypasses the IDLE / MAX_CAP deadlines. `Full`
    /// damage always goes through the window.
    ///
    /// # Invariants
    ///
    /// - Set BEFORE the PTY write, so an emit cycle racing the write
    ///   cannot miss it.
    /// - Machine-originated writes (VT replies such as DSR / DA / CPR)
    ///   must NOT set this: the flag encodes a write's provenance.
    /// - Cleared only on an `AtMostOneRow` immediate flush; the
    ///   `ManyRows` path leaves it set, so it can stay latched across
    ///   several chunks.
    unflushed_user_input: bool,
    pty: Pty,
}

impl<V: OrzmaVt> OrzmaTerm<V> {
    /// Spawns the login shell under a new PTY and builds the VT at the
    /// same grid size.
    pub fn spawn(options: SpawnOptions) -> OrzmaTermResult<Self> {
        let vt = V::new(options.cols, options.rows);
        let pty = Pty::spawn(&options)?;
        Ok(Self {
            vt,
            coalescer: Coalescer::default(),
            unflushed_user_input: false,
            pty,
        })
    }

    ///HACK:
    /// VecでTermEventを収集しているが、この関数はほぼ米フレームで呼ばれることが予想されるため、
    /// コールバック形式などにしたほうがいい？
    pub fn pump(&mut self) -> Vec<TermSignal> {
        todo!("OrzmaTerm::pump")
    }

    pub fn resize(&mut self, cols: u16, rows: u16) -> OrzmaTermResult {
        todo!("OrzmaTerm::resize")
    }

    #[inline]
    pub const fn vt_mut(&mut self) -> &mut V {
        &mut self.vt
    }

    /// Encodes a key press and writes it to the PTY.
    ///
    /// Snaps a scrolled-back viewport to the live tail first
    /// (scroll-on-input policy) so the echo is visible, and sets the
    /// user-input latch before the write.
    pub fn write_key_input(
        &mut self,
        key: &TerminalKey,
        mods: &TerminalModifiers,
    ) -> OrzmaTermResult {
        let modes = self.vt.modes();
        self.snap_to_live_tail();
        self.write_to_pty(PtyInput::encode_key(key, mods, modes.app_cursor).as_bytes())
    }

    /// Encodes one mouse report in the terminal's active mouse encoding
    /// and writes it to the PTY.
    ///
    /// Sets the user-input latch, but deliberately does NOT snap a
    /// scrolled-back viewport: the report's cell coordinates were
    /// computed by the host against the viewport the user is looking
    /// at, so yanking the view to the live tail on every report would
    /// make the screen jump under the pointer.
    pub fn write_mouse_input(&mut self, report: MouseReport) -> OrzmaTermResult {
        let sequence = report.encode(self.vt.modes().mouse_encoding);
        self.write_to_pty(&sequence)
    }

    /// Writes a paste of clipboard text to the PTY, honouring
    /// bracketed-paste mode (DECSET 2004) via [`PtyInput::encode_paste`].
    ///
    /// Empty text is a no-op: nothing reaches the PTY and the
    /// user-input latch stays untouched. Otherwise a scrolled-back
    /// viewport snaps to the live tail first (scroll-on-input policy),
    /// the latch is set before the write, and the whole frame goes out
    /// in a single write — a partially-written frame would leave the
    /// receiving app inside an unterminated paste.
    pub fn write_paste(&mut self, text: &str) -> OrzmaTermResult {
        if text.is_empty() {
            return Ok(());
        }
        let bracketed = self.vt.modes().bracketed_paste;
        self.snap_to_live_tail();
        self.write_to_pty(PtyInput::encode_paste(text, bracketed).as_bytes())
    }

    /// Sets the user-input latch and writes `buf` to the PTY.
    ///
    /// The latch is set before the write per the invariant on
    /// [`Self::unflushed_user_input`], and stays set when the write
    /// fails.
    fn write_to_pty(&mut self, buf: &[u8]) -> OrzmaTermResult {
        self.unflushed_user_input = true;
        self.pty.write_all(buf)
    }

    /// Snaps a scrolled-back viewport to the live tail (scroll-on-input
    /// policy), gated on [`OrzmaVt::at_scroll_bottom`] so a no-op call
    /// stages no damage.
    fn snap_to_live_tail(&mut self) {
        if !self.vt.at_scroll_bottom() {
            self.vt.scroll_to_bottom();
        }
    }
}
