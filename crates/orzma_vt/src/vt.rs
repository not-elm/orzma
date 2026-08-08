use crate::{
    damage::DamageVerdict, frame::Frame, modes::VtModes, prelude::VtSignal,
};

#[cfg(feature = "alacritty")]
mod alacritty;

#[cfg(feature = "alacritty")]
pub use alacritty::AlacrittyVt;

pub trait OrzmaVt: Sized {
    fn new(cols: u16, rows: u16) -> Self;

    /// Interprets a chunk of the PTY byte stream, mutating the terminal
    /// state, and classifies the resulting damage. ("Interpret" per
    /// ECMA-48 § 2.3.3: a receiving device interprets the coded
    /// representations of control functions.)
    fn interpret(&mut self, chunk: &[u8]) -> Option<DamageVerdict>;

    /// Builds the frame for the staged damage.
    fn frames(&mut self) -> Vec<Frame>;

    fn drain_signals(&mut self) -> impl Iterator<Item = VtSignal> + '_;

    /// DSR/DA reply bytes the owner must write back to the PTY.
    fn drain_replies_into(&self, buf: &mut Vec<u8>);

    /// Interactive ops stay synchronous and return whether an emit is due.
    fn scroll(&mut self, delta: i32);

    /// Snapshot of the input-relevant terminal modes.
    fn modes(&self) -> VtModes;
}
