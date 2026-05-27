//! GUI-side Components for representing ozmux Sessions as Bevy entities.
//! `SessionEntityId` wraps the domain `SessionId` (which is Bevy-free).
//! `AttachedSession` marks the single session entity currently displayed
//! in the primary OS window.

use bevy::prelude::*;
use ozmux_multiplexer::SessionId;

/// Bevy Component wrapping the domain `SessionId`. Lives on each Bevy
/// entity that represents an ozmux Session. Sortable for `FocusSessionNext`
/// cycling (ordered by the underlying monotonic `SessionId(u32)`).
#[derive(Component, Debug, Clone, Copy, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct SessionEntityId(pub SessionId);

/// Zero-sized marker inserted on the session entity currently displayed
/// in the primary OS window. Exactly one session entity carries this at
/// any time. Moving the marker swaps the rendered session.
#[derive(Component, Default, Debug)]
pub struct AttachedSession;

/// Per-Session pointer to that session's UI subtree root entity. The
/// subtree root is a `Node` that holds the full session UI (pane frames,
/// splits, activity hosts). When the session is attached (active), the
/// subtree root's `ChildOf` is `SessionUiRoot`; when parked (inactive),
/// it is the Session entity itself — a non-`Node` parent that Bevy's
/// UI walker skips (`UiChildren::iter_ui_children` filters
/// `With<Node>`), so the parked subtree gets no layout and no
/// `ComputedNode` updates.
#[derive(Component, Debug, Clone, Copy)]
pub struct SessionUiSubtree(pub Entity);
