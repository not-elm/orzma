use crate::{extension::ApcState, vt::OrzmaVt};
use alacritty_terminal::{
    Grid, Term,
    event::EventListener,
    term::TermMode,
    vte::ansi::{Handler, Processor},
};
use vtparse::{VTActor, VTParser};

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

impl OrzmaVt for AlacrittyVt {
    fn advance(&mut self, chunk: &[u8]) -> crate::prelude::DamageVerdict {
        todo!()
    }

    fn frames(&mut self) -> Vec<crate::frame::Frame> {
        todo!()
    }

    fn drain_control(&mut self) -> impl Iterator<Item = crate::prelude::ControlFrame> + '_ {
        todo!()
    }

    fn drain_replies_into(&self, buf: &mut Vec<u8>) {
        todo!()
    }

    fn scroll(&mut self, delta: i32) {
        todo!()
    }
}

struct OrzmaTermEventHandler {}

impl EventListener for OrzmaTermEventHandler {}
