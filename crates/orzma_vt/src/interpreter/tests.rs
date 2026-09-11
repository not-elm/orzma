//! Unit tests for the interpreter, one module per control function.

use super::*;
use crate::device::color::{Color, Rgb};
use crate::device::modes::{InsertReplaceMode, MouseEncoding, MouseTracking};
use crate::frame::Frame;
use crate::placement::{AnchoredPlacement, InstanceId, MAX_PLACEMENTS, PlacementSize};
use crate::screen::cell::Cell;
use crate::screen::grid::GridSize;
use crate::screen::grid::coords::{GridColumn, GridLine};
use crate::screen::grid::run::Style;
use crate::screen::viewport::ViewportLine;
use crate::{OrzmaVt, Vt};

/// Runs `chunk` through a fresh terminal and hands back the device
/// it wrote to together with everything the chunk produced.
fn interpret_fully(chunk: &[u8]) -> (DeviceState, InterpretOutput) {
    interpret_sized(4, chunk)
}

/// Runs `chunk` on a grid wide enough for the default tabulation
/// stride: twenty columns put the right edge at 19, so the stops at
/// 8 and 16 are reachable and the one at 24 is not.
fn interpret_wide(chunk: &[u8]) -> DeviceState {
    interpret_sized(20, chunk).0
}

fn interpret_sized(cols: u16, chunk: &[u8]) -> (DeviceState, InterpretOutput) {
    let mut vt = OrzmaVt::new(GridSize { cols, rows: 3 }, 10);
    let output = vt.interpret(chunk);
    (vt.device, output)
}

/// Runs `chunk` through a fresh interpreter and hands back the
/// device it wrote to.
fn interpret(chunk: &[u8]) -> DeviceState {
    interpret_fully(chunk).0
}

/// Reports the chunk liveness `chunk` produced from a fresh device.
fn damage_of(chunk: &[u8]) -> bool {
    interpret_fully(chunk).1.damaged
}

/// Every glyph of the device's first visible row, left to right.
fn first_row_glyphs(device: &DeviceState) -> Vec<char> {
    device
        .active_screen()
        .viewport_row(ViewportLine(0))
        .iter()
        .map(|cell| cell.c)
        .collect()
}

/// Reports the reply bytes `chunk` produced, through the public
/// entry point rather than the crate-internal interpreter.
fn replies_of(chunk: &[u8]) -> Vec<u8> {
    let mut vt = OrzmaVt::new(GridSize { cols: 4, rows: 3 }, 10);
    vt.interpret(chunk).replies
}

/// One terminal kept across chunks, so a test can build up state
/// with one chunk and then observe what a later chunk alone
/// produced, through the same entry points the owner uses.
struct Session(OrzmaVt);

impl Session {
    fn new() -> Self {
        Self::sized(GridSize { cols: 4, rows: 3 })
    }

    /// One terminal of the given size, for a capture whose scroll
    /// region needs more rows than the default three.
    fn sized(size: GridSize) -> Self {
        Self(OrzmaVt::new(size, 10))
    }

    /// Interprets `chunk` and hands back what it alone produced.
    fn feed(&mut self, chunk: &[u8]) -> InterpretOutput {
        self.0.interpret(chunk)
    }

    /// Emits the pending frame, if anything observable changed.
    fn frame(&mut self) -> Option<Frame> {
        self.0.frame()
    }

    /// Mounts a one-cell placement at the active screen's cursor.
    fn mount(&mut self, id: InstanceId) {
        assert!(
            self.0
                .device
                .mount_placement(PlacementSize { rows: 1, cols: 1 }, id),
            "a mount under the cap is accepted"
        );
    }

    /// Emits the pending frame and hands back the placement list it
    /// carries.
    fn listed_placements(&mut self) -> Vec<AnchoredPlacement> {
        self.frame()
            .expect("the chunk emits a frame")
            .placements
            .expect("the placement list changed")
    }

    fn active_screen(&self) -> ScreenKind {
        self.0.device.modes().active_screen
    }

    fn cursor_column(&self) -> u16 {
        self.0.device.active_screen().cursor_column().0
    }

    fn char_at(&self, line: u16, column: u16) -> char {
        self.0
            .device
            .active_screen()
            .viewport_row(ViewportLine(line))[column]
            .c
    }
}

/// The glyphs of the first four columns of the top viewport row.
/// Runs `setup` and then `chunk` over one session, and reports the
/// liveness `chunk` alone produced.
fn liveness_after(setup: &[u8], chunk: &[u8]) -> bool {
    let mut session = Session::new();
    session.feed(setup);
    session.feed(chunk).damaged
}

mod alignment;
mod alternate_screen;
mod auto_wrap;
mod character_editing;
mod character_set;
mod cursor;
mod cursor_checkpoint;
mod device_attributes;
mod device_status;
mod erase;
mod interpret_output;
mod keypad;
mod line_editing;
mod line_movement;
mod modes;
mod mouse;
mod parser_limits;
mod printing;
mod private_modes;
mod reset;
mod reverse_index;
mod sgr;
mod tabulation;
mod title;
mod unsupported_sequences;
mod webview_apc;
