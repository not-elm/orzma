use crate::{damage::DamageVerdict, extension::ApcState, frame::Frame, prelude::ControlFrame};

mod alacritty;

pub use alacritty::AlacrittyVt;

pub trait OrzmaVt: Sized {
    fn new(cols: u16, rows: u16) -> Self;

    fn advance(&mut self, chunk: &[u8]) -> DamageVerdict;

    /// Builds the frame for the staged damage.
    fn frames(&mut self) -> Vec<Frame>;

    fn drain_control(&mut self) -> impl Iterator<Item = ControlFrame> + '_;

    /// DSR/DA reply bytes the owner must write back to the PTY.
    fn drain_replies_into(&self, buf: &mut Vec<u8>);

    /// Interactive ops stay synchronous and return whether an emit is due.
    fn scroll(&mut self, delta: i32);
}
