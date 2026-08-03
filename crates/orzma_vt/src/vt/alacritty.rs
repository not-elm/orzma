use alacritty_terminal::{
    Grid, Term,
    event::EventListener,
    vte::ansi::{Handler, Processor},
};
use vtparse::{VTActor, VTParser};

use crate::apc::ApcState;

pub struct AlacrittyVt {
    processor: Processor,
    term: Term<OrzmaTermEventHandler>,
    apc_state: ApcState,
    apc_parser: VTParser,
}

impl AlacrittyVt {
    pub fn advance(&mut self, bytes: &[u8]) {
        self.apc_parser.parse(bytes, &mut self.apc_state);
        self.processor.advance(&mut self.term, bytes);
    }
}

struct OrzmaTermEventHandler {}

impl EventListener for OrzmaTermEventHandler {}
