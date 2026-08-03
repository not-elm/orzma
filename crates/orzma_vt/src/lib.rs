use vtparse::{VTActor, VTParser};

use alacritty_terminal::{
    Grid, Term,
    event::EventListener,
    vte::ansi::{Handler, Processor},
};

use crate::{
    control_frame::ControlFrame, damage::DamageVerdict, frame::Frame, webview::WebviewApcState,
};

mod control_frame;
mod damage;
mod frame;
mod webview;

pub mod prelude {
    pub use crate::{OrzmaVt, control_frame::*, damage::DamageVerdict};
}

pub trait OrzmaVt {
    fn ingest(&mut self, chunk: &[u8]) -> DamageVerdict;

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
    webview_apc_state: WebviewApcState,
    webview_apc_parser: VTParser,
}

impl Vt {
    pub fn advance(&mut self, bytes: &[u8]) {
        self.webview_apc_parser
            .parse(bytes, &mut self.webview_apc_state);
        self.processor.advance(&mut self.term, bytes);
    }
}

struct OrzmaTermEventHandler {}

impl EventListener for OrzmaTermEventHandler {}
