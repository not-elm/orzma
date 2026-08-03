use vtparse::{VTActor, VTParser};

use alacritty_terminal::{
    Grid, Term,
    event::EventListener,
    vte::ansi::{Handler, Processor},
};

use crate::webview::WebviewApcState;

mod damage;
mod webview;

pub struct Vt {
    processor: Processor,
    term: Term<OrzmaTermEventHandler>,
    webview: WebviewApcState,
    webview_parser: VTParser,
}

impl Vt {
    pub fn advance(&mut self, bytes: &[u8]) {
        self.webview_parser.parse(bytes, &mut self.webview);
        self.processor.advance(&mut self.term, bytes);
    }
}

struct OrzmaTermEventHandler {}

impl EventListener for OrzmaTermEventHandler {}
