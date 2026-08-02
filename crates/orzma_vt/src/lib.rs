use vtparse::{VTActor, VTParser};

use crate::{parser::Parser, vt_state::VTState};

mod grid;
mod parser;
mod vt_state;

pub struct Vt {
    state: VTState,
}

impl Vt {
    pub fn advance(&mut self, bytes: &[u8]) {}
}
