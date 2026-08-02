use vtparse::{VTActor, VTParser};

use alacritty_terminal::{
    Grid, Term,
    vte::ansi::{Handler, Processor},
};

mod webview;

pub struct Vt {
    processor: Processor,
    parser: VTParser,
}

impl Vt {
    pub fn advance(&mut self, bytes: &[u8]) {}
}
