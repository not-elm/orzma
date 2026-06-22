//! Copy-mode reply correlation: a generic channel so the binary owns the
//! copy-mode logic while this crate keeps the single transport drain.
//!
//! The binary registers a copy-mode control command (state query, capture, or
//! show-buffer) via [`CopyModeQueries::register`], keyed by the `CommandId`
//! `AdoptedHandle::send` returned. When the matching `%begin/%end` reply lands,
//! [`drain_copy_replies`] correlates it back to its pane + kind and emits a
//! [`CopyModeReply`] the binary consumes — the binary never drains the transport
//! channel itself (that would steal events from the one real drainer).

use bevy::prelude::{Message, Resource};
use std::collections::HashMap;
use tmux_control::{ClientEvent, CommandId, TransportEvent};
use tmux_control_parser::PaneId;

/// Which copy-mode control command a pending reply belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CopyQueryKind {
    /// A `display-message` reading the pane's copy-mode state snapshot.
    State,
    /// A `capture-pane` of the scrolled copy-mode viewport.
    Capture,
    /// A `show-buffer` reading the top paste buffer for the clipboard bridge.
    Buffer,
}

/// In-flight copy-mode control commands, keyed by [`CommandId`], so each reply
/// drained by `ozmux_tmux` is routed back to the pane + kind that issued it.
#[derive(Resource, Default)]
pub struct CopyModeQueries {
    pending: HashMap<CommandId, (PaneId, CopyQueryKind)>,
}

impl CopyModeQueries {
    /// Records an in-flight copy-mode command so its reply is correlated.
    pub fn register(&mut self, id: CommandId, pane: PaneId, kind: CopyQueryKind) {
        self.pending.insert(id, (pane, kind));
    }

    /// Removes and returns the `(pane, kind)` registered for `id`, if any.
    fn take(&mut self, id: CommandId) -> Option<(PaneId, CopyQueryKind)> {
        self.pending.remove(&id)
    }

    /// Drops every in-flight entry (on disconnect, so a reconnect starts clean).
    pub(crate) fn clear(&mut self) {
        self.pending.clear();
    }
}

/// One correlated copy-mode command reply, surfaced to the binary's refresh
/// plugin. `ok` mirrors the `%end`/`%error` status; `output` is the reply body.
#[derive(Message, Clone, Debug)]
pub struct CopyModeReply {
    /// The pane the command targeted.
    pub pane: PaneId,
    /// Which copy-mode command the reply answers.
    pub kind: CopyQueryKind,
    /// Whether the command succeeded (`%end` vs `%error`).
    pub ok: bool,
    /// The reply body lines.
    pub output: Vec<String>,
}

/// Correlates every `CommandComplete` in `events` whose id is registered in
/// `queries`, removing the entry and returning a [`CopyModeReply`] per match.
/// Replies whose id is not registered are left untouched (the existing typed
/// correlation paths consume those).
pub(crate) fn drain_copy_replies(
    queries: &mut CopyModeQueries,
    events: &[TransportEvent],
) -> Vec<CopyModeReply> {
    let mut replies = Vec::new();
    for event in events {
        if let TransportEvent::Protocol(ClientEvent::CommandComplete { id, ok, output, .. }) = event
            && let Some((pane, kind)) = queries.take(*id)
        {
            replies.push(CopyModeReply {
                pane,
                kind,
                ok: *ok,
                output: output.clone(),
            });
        }
    }
    replies
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete(id: u64, ok: bool, output: &[&str]) -> TransportEvent {
        TransportEvent::Protocol(ClientEvent::CommandComplete {
            id: CommandId(id),
            number: 0,
            ok,
            output: output.iter().map(|s| (*s).to_string()).collect(),
        })
    }

    #[test]
    fn drains_registered_reply_with_pane_and_kind() {
        let mut queries = CopyModeQueries::default();
        queries.register(CommandId(7), PaneId(3), CopyQueryKind::State);
        let events = vec![complete(7, true, &["1\t0\t8\t0\t0\t0\t0\t0\t0\t0\t0\t0"])];

        let replies = drain_copy_replies(&mut queries, &events);

        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].pane, PaneId(3));
        assert_eq!(replies[0].kind, CopyQueryKind::State);
        assert!(replies[0].ok);
        assert_eq!(replies[0].output.len(), 1);
        assert!(
            drain_copy_replies(&mut queries, &events).is_empty(),
            "the entry is consumed on the first drain"
        );
    }

    #[test]
    fn leaves_unregistered_reply_untouched() {
        let mut queries = CopyModeQueries::default();
        queries.register(CommandId(7), PaneId(3), CopyQueryKind::State);
        let events = vec![complete(99, true, &["other"])];

        assert!(drain_copy_replies(&mut queries, &events).is_empty());
        assert_eq!(
            queries.take(CommandId(7)),
            Some((PaneId(3), CopyQueryKind::State)),
            "an unrelated reply must not consume a registered entry"
        );
    }

    #[test]
    fn surfaces_failed_capture_reply() {
        let mut queries = CopyModeQueries::default();
        queries.register(CommandId(4), PaneId(9), CopyQueryKind::Capture);
        let events = vec![complete(4, false, &[])];

        let replies = drain_copy_replies(&mut queries, &events);
        assert_eq!(replies.len(), 1);
        assert!(!replies[0].ok);
        assert_eq!(replies[0].kind, CopyQueryKind::Capture);
    }

    #[test]
    fn clear_drops_pending_entries() {
        let mut queries = CopyModeQueries::default();
        queries.register(CommandId(1), PaneId(1), CopyQueryKind::Buffer);
        queries.clear();
        assert!(drain_copy_replies(&mut queries, &[complete(1, true, &[])]).is_empty());
    }
}
