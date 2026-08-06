//! Alacritty-backed [`OrzmaVt`] implementation.

use crate::{extension::ApcState, vt::OrzmaVt};
use alacritty_terminal::{
    Grid, Term,
    event::EventListener,
    grid::Dimensions,
    term::{Config, TermMode},
    vte::ansi::{Handler, Processor},
};
use std::iter;
use vtparse::{VTActor, VTParser};

pub struct AlacrittyVt {
    processor: Processor,
    term: Term<OrzmaTermEventHandler>,
    apc_state: ApcState,
    apc_parser: VTParser,
}

impl AlacrittyVt {
    /// Builds a VT backed by an alacritty `Term` at the given grid size.
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            processor: Processor::new(),
            term: Term::new(
                Config::default(),
                &LocalDim::new(cols, rows),
                OrzmaTermEventHandler {},
            ),
            apc_state: ApcState::default(),
            apc_parser: VTParser::new(),
        }
    }

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
        // TODO: drain control frames captured from the APC stream.
        iter::empty()
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

/// Grid size handed to `Term::new` / `Term::resize`.
///
/// Alacritty's own `TermSize` is `pub(crate)`, so a minimal local
/// equivalent lives here. `total_lines == screen_lines` on purpose:
/// scrollback capacity comes from `Config::scrolling_history`, not
/// from the size type.
struct LocalDim {
    cols: usize,
    rows: usize,
}

impl LocalDim {
    fn new(cols: u16, rows: u16) -> Self {
        Self {
            cols: cols.into(),
            rows: rows.into(),
        }
    }
}

impl Dimensions for LocalDim {
    fn columns(&self) -> usize {
        self.cols
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn total_lines(&self) -> usize {
        self.rows
    }
}
