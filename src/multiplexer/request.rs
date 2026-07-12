//! Centralized multiplexer request events: the shapes every PR-2/PR-3
//! shortcut-applier and confirm-prompt handler consumes. Defined once here so
//! downstream observer/handler tasks only add handlers and never
//! forward-reference a type.
//!
//! Each request is either an `EntityEvent` (consumed by an `On<E>` observer)
//! or a `Message` (consumed by a `MessageReader` system, gated
//! `on_message::<T>`) — the handler kind is fixed by the derive.

use crate::multiplexer::layout::SplitAxis;
use bevy::prelude::*;
use orzma_configs::shortcuts::PaneDirection;

/// Requests splitting `pane` along `axis`, creating a new sibling pane.
/// Consumed by an `On<SplitPaneRequest>` observer.
#[derive(EntityEvent)]
pub(crate) struct SplitPaneRequest {
    /// The pane to split.
    #[event_target]
    pub pane: Entity,
    /// The divider axis for the new sibling.
    pub axis: SplitAxis,
}

/// Requests closing `pane`, running the shared `close_pane` cascade.
/// Consumed by an `On<KillPaneRequest>` observer.
#[derive(EntityEvent)]
pub(crate) struct KillPaneRequest {
    /// The pane to close.
    #[event_target]
    pub pane: Entity,
}

/// Requests moving keyboard focus to the neighbor pane in `dir`. The active
/// pane is resolved by the consuming handler, not carried on the request.
/// Consumed by a `MessageReader<SelectPaneRequest>` system.
#[derive(Message)]
pub(crate) struct SelectPaneRequest {
    /// The direction to move focus in.
    pub dir: PaneDirection,
}

/// Requests resizing the focused pane's border in `dir`.
/// Consumed by a `MessageReader<ResizePaneRequest>` system.
#[derive(Message)]
pub(crate) struct ResizePaneRequest {
    /// The direction to resize toward.
    pub dir: PaneDirection,
}

/// Requests toggling zoom on the active pane.
/// Consumed by a `MessageReader<ZoomPaneRequest>` system.
#[derive(Message)]
pub(crate) struct ZoomPaneRequest;

/// Which window a `SelectWindowRequest` targets.
#[derive(Clone, Copy)]
pub(crate) enum WindowSelect {
    /// The next window in index order.
    Next,
    /// The previous window in index order.
    Previous,
    /// The window at this tmux display index.
    Index(u8),
}

/// Requests opening a new window in the current session.
/// Consumed by a `MessageReader<NewWindowRequest>` system.
#[derive(Message)]
pub(crate) struct NewWindowRequest;

/// Requests closing `window`, running the window-close cascade.
/// Consumed by an `On<KillWindowRequest>` observer.
#[derive(EntityEvent)]
pub(crate) struct KillWindowRequest {
    /// The window to close.
    #[event_target]
    pub window: Entity,
}

/// Requests switching the active window per `WindowSelect`.
/// Consumed by a `MessageReader<SelectWindowRequest>` system.
#[derive(Message)]
pub(crate) struct SelectWindowRequest(pub WindowSelect);

/// Requests opening the rename prompt for the active window; the prompt
/// commits `MultiplexerWindow.name` directly on confirm.
/// Consumed by a `MessageReader<RenameWindowRequest>` system.
#[derive(Message)]
pub(crate) struct RenameWindowRequest;

/// Requests opening the kill-pane confirm prompt for `pane`. On confirm, the
/// prompt fires `KillPaneRequest`.
/// Consumed by a `MessageReader<OpenKillPaneConfirm>` system.
#[derive(Message)]
pub(crate) struct OpenKillPaneConfirm {
    /// The pane the confirm prompt targets.
    pub pane: Entity,
}

/// Requests opening the kill-window confirm prompt for `window`. On confirm,
/// the prompt fires `KillWindowRequest`.
/// Consumed by a `MessageReader<OpenKillWindowConfirm>` system.
#[derive(Message)]
pub(crate) struct OpenKillWindowConfirm {
    /// The window the confirm prompt targets.
    pub window: Entity,
}
