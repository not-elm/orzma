//! Pure mouse-button routing for the Bevy terminal.
//!
//! This module is Bevy- and PTY-agnostic. The Bevy
//! `dispatch_mouse_buttons` system hit-tests the cursor, tracks click
//! count, builds a `ButtonEvent`, and calls `ButtonAction::route` to
//! decide whether the event becomes a local selection mutation or
//! PTY-bound mouse-protocol bytes.
//!
//! See `docs/superpowers/specs/2026-05-27-mouse-selection-design.md`
//! for the full decision table.

use crate::mouse_encode::{CellCoord, ProtocolModifiers};
use alacritty_terminal::index::Side;
use alacritty_terminal::selection::SelectionType;
use alacritty_terminal::term::TermMode;

/// Discrete event kinds the router understands. Press/Release are
/// transitions on a single button; Drag is "motion while a button is
/// held".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonEventKind {
    Press,
    Release,
    Drag,
}

/// Logical mouse button identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButtonKind {
    Left,
    Middle,
    Right,
}

/// One mouse-button event, projected into pane-relative cell coords.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ButtonEvent {
    pub kind: ButtonEventKind,
    pub button: MouseButtonKind,
    pub cell: CellCoord,
    pub side: Side,
    /// 1, 2, or 3 — caller-tracked. Drag and Release ignore this.
    pub click_count: u8,
}

/// Subset of MouseConfig used by `ButtonAction::route`.
#[derive(Clone, Debug, Default)]
pub struct ButtonConfig {
    /// Hard cap on the number of PTY-bound reports emitted per route
    /// call. Mirrors `WheelConfig::max_protocol_events_per_frame`.
    pub max_protocol_events_per_frame: u32,
}

/// What `ButtonAction::route` decided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ButtonAction {
    /// Nothing to do this event.
    Noop,
    /// Write these pre-encoded bytes to the PTY.
    WriteToPty(Vec<u8>),
    /// Drop any active local selection AND write these bytes to the
    /// PTY. Used on forwarded press events so a previous highlight does
    /// not visually persist past a click that goes to the app.
    ClearAndWriteToPty(Vec<u8>),
    /// Begin a new local selection of `ty` at `(cell, side)`.
    StartLocalSelection {
        ty: SelectionType,
        cell: CellCoord,
        side: Side,
    },
    /// Extend the current local selection's moving end to `(cell, side)`.
    UpdateLocalSelection { cell: CellCoord, side: Side },
    /// Drop the local selection without writing to the PTY.
    ClearLocalSelection,
}

impl ButtonAction {
    /// Pure decision function — see module doc and spec §4 decision
    /// table. The router is stateless: click counting and drag-state
    /// tracking live in the Bevy glue.
    pub fn route(
        _modes: TermMode,
        _evt: ButtonEvent,
        _mods: ProtocolModifiers,
        _cfg: &ButtonConfig,
    ) -> Self {
        // Skeleton — Task 11/12 fill in the decision table.
        ButtonAction::Noop
    }
}
