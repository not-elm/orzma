//! Effect values produced by the tmux mouse deciders and applied by the observer.

use bevy::prelude::*;
use ozmux_tmux::PaneId;
use tmux_control_parser::DividerAxis;

/// Word- vs line-granularity selection for a double/triple click.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MultiSelectKind {
    Word,
    Line,
}

/// A single decided tmux side effect. Geometry is resolved at gather time and
/// baked in, so the apply observer needs no world queries.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum TmuxMouseEffect {
    SelectPane(PaneId),
    ResizePane {
        axis: DividerAxis,
        primary: PaneId,
        size: u32,
    },
    BeginCopyDrag {
        pane: PaneId,
        snapshot_cursor: (u16, u16),
        anchor: (u16, u16),
    },
    ExtendCopyDrag {
        pane: PaneId,
        snapshot_cursor: (u16, u16),
        cell: (u16, u16),
    },
    MultiSelect {
        pane: PaneId,
        kind: MultiSelectKind,
        snapshot_cursor: (u16, u16),
        cell: (u16, u16),
    },
    CopySelection {
        pane: PaneId,
    },
}

/// Carries a frame's decided effects to `on_tmux_mouse_effects`; the
/// `#[event_target] entity` is the gesture's pane and is not queried by the
/// observer (every variant carries its own `PaneId`).
#[derive(EntityEvent, Debug, Clone)]
pub(super) struct TmuxMouseEffects {
    #[event_target]
    pub(super) entity: Entity,
    pub(super) effects: Vec<TmuxMouseEffect>,
}
