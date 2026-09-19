//! Tests for [`OrzmaTty`], one file per operation under test.

use super::*;
use crate::error::OrzmaTtyError;
use crate::input::{
    CellCoord, MouseButton, MouseReportKind, ProtocolModifiers, WheelConfig, WheelInput,
    WheelModifiers,
};
use crate::test_support::{CaptureSink, FailingMaster, FailingSink, FakeVt};
use crossbeam_channel::{Sender, unbounded};
use std::thread;
use std::time::Duration;

mod coalescer;
mod focus;
mod host;
mod input;
mod pump;
mod resize;
mod scroll;
mod sync;
mod wheel;

fn grid(cols: u16, rows: u16) -> GridSize {
    GridSize::new(cols, rows).expect("a valid size")
}

/// An 80x24 [`OrzmaTty::detached`] terminal over a `FakeVt`, plus
/// the sink its PTY writes land on.
fn detached_term() -> (OrzmaTty<FakeVt>, CaptureSink) {
    let sink = CaptureSink::default();
    let term = OrzmaTty::detached(
        FakeVt::new(grid(80, 24)),
        grid(80, 24),
        Box::new(sink.clone()),
    )
    .expect("OrzmaTty::detached");
    (term, sink)
}

/// A terminal whose VT tracks button events with SGR reports, as nvim
/// leaves it.
fn tracking_term() -> (OrzmaTty<FakeVt>, CaptureSink) {
    let (mut term, sink) = detached_term();
    term.vt.modes.mouse_tracking = MouseTracking::Drag;
    term.vt.modes.mouse_encoding = MouseEncoding::Sgr;
    (term, sink)
}

/// An 80x24 terminal whose chunk and exit streams the test feeds
/// through the returned senders.
fn channelled_term() -> (OrzmaTty<FakeVt>, Sender<Vec<u8>>, Sender<Option<i32>>) {
    let (chunk_tx, chunk_rx) = unbounded::<Vec<u8>>();
    let (exit_tx, exit_rx) = unbounded::<Option<i32>>();
    let term = OrzmaTty::detached_with_channels(
        FakeVt::new(grid(80, 24)),
        grid(80, 24),
        Box::new(CaptureSink::default()),
        chunk_rx,
        exit_rx,
    )
    .expect("OrzmaTty::detached_with_channels");
    (term, chunk_tx, exit_tx)
}

/// Collects the signals out of a pumped output, in order.
fn signals_of(output: &PumpOutput) -> Vec<TtySignal> {
    output.signals().cloned().collect()
}

/// Collects the `ChildExit` codes out of a pumped output.
fn child_exits(output: &PumpOutput) -> Vec<Option<i32>> {
    output
        .signals()
        .filter_map(|signal| match signal {
            TtySignal::ChildExit { code } => Some(*code),
            _ => None,
        })
        .collect()
}

/// A scripted interpret result that consumed `consumed` bytes.
fn update(consumed: usize, closed: bool) -> InterpretOutput {
    InterpretOutput {
        damaged: true,
        signals: Vec::new(),
        replies: Vec::new(),
        consumed,
        synchronized_update_closed: closed,
    }
}

/// A minimal frame for scripting `FakeVt::frames`; its values are
/// arbitrary placeholders.
fn a_frame() -> Frame {
    Frame {
        size: GridSize { cols: 80, rows: 24 },
        rows: Vec::new(),
        cursor: Cursor::default(),
        display_offset: DisplayOffset(0),
        vi_cursor: None,
        selection: None,
        placements: None,
        palette: None,
        hyperlinks: Vec::new(),
    }
}
