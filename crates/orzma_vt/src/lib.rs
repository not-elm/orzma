use vtparse::{VTActor, VTParser};

use alacritty_terminal::{
    Grid, Term,
    event::EventListener,
    vte::ansi::{Handler, Processor},
};

use crate::{apc::ApcState, control_frame::ControlFrame, damage::DamageVerdict, frame::Frame};

mod apc;
mod control_frame;
mod damage;
mod frame;

pub mod prelude {
    pub use crate::{OrzmaVt, Vt, apc::*, control_frame::*, damage::DamageVerdict};
}

pub trait OrzmaVt {
    fn advance(&mut self, chunk: &[u8]) -> DamageVerdict;

    /// Builds the frame for the staged damage.
    fn frames(&mut self) -> Vec<Frame>;

    /// Bell / Title / ResetTitle / Clipboard / CurrentDir / Webview.
    fn drain_control(&mut self) -> impl Iterator<Item = ControlFrame> + '_;

    /// DSR/DA reply bytes the owner must write back to the PTY.
    fn drain_replies_into(&self, buf: &mut Vec<u8>);

    /// Interactive ops stay synchronous and return whether an emit is due.
    fn scroll(&mut self, delta: i32);
}

pub struct Vt {
    processor: Processor,
    term: Term<OrzmaTermEventHandler>,
    apc_state: ApcState,
    apc_parser: VTParser,
}

impl Vt {
    pub fn advance(&mut self, bytes: &[u8]) {
        self.apc_parser.parse(bytes, &mut self.apc_state);
        self.processor.advance(&mut self.term, bytes);
    }
}

struct OrzmaTermEventHandler {}
impl EventListener for OrzmaTermEventHandler {}
