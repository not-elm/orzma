use crate::{damage::DamageVerdict, extension::ApcState, prelude::ControlFrame};
use alacritty_terminal::{Term, event::EventListener, vte::ansi::Processor};
use vtparse::VTParser;

mod alacritty;

pub use alacritty::AlacrittyVt;

pub trait OrzmaVt {
    fn advance(&mut self, chunk: &[u8]) -> DamageVerdict;
    /// Builds the frame for the staged damage. fn frames(&mut self) -> Vec<Frame>; /// Bell / Title / ResetTitle / Clipboard / CurrentDir / Webview.
    fn drain_control(&mut self) -> impl Iterator<Item = ControlFrame> + '_;

    /// DSR/DA reply bytes the owner must write back to the PTY.
    fn drain_replies_into(&self, buf: &mut Vec<u8>);

    /// Interactive ops stay synchronous and return whether an emit is due.
    fn scroll(&mut self, delta: i32);
}
