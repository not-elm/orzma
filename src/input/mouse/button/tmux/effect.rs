//! Effect values produced by the tmux mouse deciders and applied by the observer.

use bevy::prelude::*;
use ozma_tty_engine::{Point, SelectionType, Side};
use ozmux_tmux::PaneId;
use tmux_control_parser::DividerAxis;

/// Word- vs line-granularity selection for a double/triple click.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MultiSelectKind {
    Word,
    Line,
}

/// A single decided tmux side effect. Geometry is resolved at gather time and
/// baked in, so the apply observer needs no world queries. `SelectPane` /
/// `ResizePane` are tmux control-mode commands; the copy-drag variants drive
/// the pane's local terminal selection directly via `TerminalSelection*`
/// events.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum TmuxMouseEffect {
    SelectPane(PaneId),
    ResizePane {
        axis: DividerAxis,
        primary: PaneId,
        size: u32,
    },
    BeginCopyDrag {
        entity: Entity,
        anchor: Point,
        side: Side,
        ty: SelectionType,
    },
    ExtendCopyDrag {
        entity: Entity,
        cell: Point,
        side: Side,
    },
    MultiSelect {
        entity: Entity,
        kind: MultiSelectKind,
        cell: Point,
        side: Side,
    },
    CopySelection {
        entity: Entity,
    },
}

/// Carries a frame's decided effects to `on_tmux_mouse_effects`; the
/// `#[event_target] entity` is the gesture's pane and is not queried by the
/// observer (`SelectPane`/`ResizePane` carry their own `PaneId`; the copy-drag
/// variants carry their own `Entity`).
#[derive(EntityEvent, Debug, Clone)]
pub(super) struct TmuxMouseEffects {
    #[event_target]
    pub entity: Entity,
    pub effects: Vec<TmuxMouseEffect>,
}
