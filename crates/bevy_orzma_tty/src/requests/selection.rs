//! Per-operation selection request events the host UI fires at a
//! terminal entity.
//!
//! The payload vocabulary ([`SelectionKind`], [`CellSide`],
//! [`GridPoint`]) is owned by the VT layer; this module re-exports
//! it so the requests and their payload types travel together — each
//! request carries exactly what the VT applies.
//!
//! The apply observers are stubs until a selection capability trait
//! lands on the new `Vt` protocol.

use bevy::prelude::*;
pub use orzma_vt::prelude::{CellSide, GridPoint, SelectionKind};

/// Fired by the host UI to anchor a new selection at an explicit
/// grid cell (mouse press).
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTtySelectionStart {
    #[event_target]
    pub terminal: Entity,
    /// Grid cell the press landed on; the host UI resolves the
    /// clicked viewport cell against the displayed frame's display
    /// offset before firing.
    pub cell: GridPoint,
    /// Which half of the cell the anchor sits in.
    pub side: CellSide,
    /// Granularity of the new selection.
    pub kind: SelectionKind,
}

/// Fired by the host UI to anchor a new selection at the vi cursor
/// (vi-mode `v` / `V`), whose position only the VT knows.
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTtySelectionStartAtViCursor {
    #[event_target]
    pub terminal: Entity,
    /// Granularity of the new selection.
    pub kind: SelectionKind,
}

/// Fired by the host UI to move the moving end of the active selection
/// (mouse drag).
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTtySelectionUpdate {
    #[event_target]
    pub terminal: Entity,
    /// Grid cell the moving end is dragged to. May reach into
    /// scrollback history (a negative line) when the drag leaves the
    /// viewport.
    pub cell: GridPoint,
    /// Which half of the cell the moving end sits in.
    pub side: CellSide,
}

/// Fired by the host UI to switch selection granularity while keeping
/// the anchor (vi-mode `v` while `V` is active, and the reverse).
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTtySelectionKindChange {
    #[event_target]
    pub terminal: Entity,
    /// The granularity to switch to.
    pub kind: SelectionKind,
}

/// Fired by the host UI to drop any active selection.
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTtySelectionClear {
    #[event_target]
    pub terminal: Entity,
}

pub(super) struct SelectionPlugin;

impl Plugin for SelectionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(start_selection)
            .add_observer(start_selection_at_vi_cursor)
            .add_observer(update_selection)
            .add_observer(change_selection_kind)
            .add_observer(clear_selection);
    }
}

fn start_selection(_e: On<RequestTtySelectionStart>) {}

fn start_selection_at_vi_cursor(_e: On<RequestTtySelectionStartAtViCursor>) {}

fn update_selection(_e: On<RequestTtySelectionUpdate>) {}

fn change_selection_kind(_e: On<RequestTtySelectionKindChange>) {}

fn clear_selection(_e: On<RequestTtySelectionClear>) {}
