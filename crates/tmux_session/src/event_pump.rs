//! Draining, logging, and routing of tmux transport events: into
//! `ConnectionState` and the global projection events the observers consume.

use crate::components::WindowFlags;
use crate::enumerate::{WINDOW_FLAGS_SUBSCRIPTION, parse_window_rows};
use crate::events::{
    TmuxActivePaneChanged, TmuxActiveWindowChanged, TmuxLayoutChanged, TmuxSessionChanged,
    TmuxWindowAdded, TmuxWindowClosed, TmuxWindowFlagsChanged, TmuxWindowRenamed,
    TmuxWindowsRetained,
};
use crate::keybindings::{KeyBinding, ModeKeys, parse_list_keys, parse_prefix};
use crate::output::PaneOutput;
use crate::state::{ConnectionState, next_state};
use bevy::prelude::Commands;
use crossbeam_channel::Receiver;
use std::collections::{HashMap, HashSet};
use tmux_control::{ClientEvent, CommandId, ControlEvent, TransportEvent};
use tmux_control_parser::{PaneId, SessionId, WindowId};

/// Upper bound on events drained per frame, so a pane flooding `%output`
/// cannot stall the schedule with unbounded parse/apply work in one tick;
/// any remainder stays queued and is drained on the next frame.
const MAX_EVENTS_PER_FRAME: usize = 4096;

/// Drains up to [`MAX_EVENTS_PER_FRAME`] currently-available transport events
/// from `events`, logging each. Non-blocking: returns once the channel is
/// empty or the per-frame cap is hit.
pub(crate) fn drain_transport(events: &Receiver<TransportEvent>) -> Vec<TransportEvent> {
    let mut drained = Vec::new();
    while drained.len() < MAX_EVENTS_PER_FRAME {
        match events.try_recv() {
            Ok(event) => {
                log_transport_event(&event);
                drained.push(event);
            }
            Err(_) => break,
        }
    }
    drained
}

/// Folds `events` through [`next_state`] from `current`, returning the resulting
/// `ConnectionState` if the batch changed it, or `None` if it ended unchanged.
///
/// Returning the next state (rather than mutating in place) lets the caller
/// write it back through `ResMut` only on a real transition, so change
/// detection fires once per transition instead of every frame.
pub(crate) fn advance_state(
    current: &ConnectionState,
    events: &[TransportEvent],
) -> Option<ConnectionState> {
    let mut next: Option<ConnectionState> = None;
    for event in events {
        next = Some(next_state(next.as_ref().unwrap_or(current), event));
    }
    next.filter(|n| n != current)
}

/// Translates a drained transport batch into global projection events, in
/// stream order, triggering each via `commands`. The enumeration reply (the
/// `CommandComplete` whose id matches `pending`) is decomposed into per-row
/// `TmuxWindowAdded` + `TmuxLayoutChanged` (+ `TmuxActiveWindowChanged` for the
/// active row), followed by one `TmuxWindowsRetained` prune. Untracked events
/// (e.g. `%output`) are ignored here (routed separately as `PaneOutput`).
/// `own_client` gates `%client-session-changed` — see [`detect_session_switch`].
pub(crate) fn trigger_events(
    commands: &mut Commands,
    pending: &mut Option<CommandId>,
    events: &[TransportEvent],
    own_client: Option<&str>,
) {
    for event in events {
        match event {
            TransportEvent::Protocol(ClientEvent::Notification(notification)) => {
                trigger_notification(commands, own_client, notification);
            }
            TransportEvent::Protocol(ClientEvent::CommandComplete { id, ok, output, .. })
                if *pending == Some(*id) =>
            {
                *pending = None;
                if *ok {
                    trigger_seed(commands, output);
                } else {
                    tracing::warn!("list-windows enumeration command failed");
                }
            }
            _ => {}
        }
    }
}

/// Returns the first non-empty trimmed output line of the `CommandComplete`
/// whose id matches `pending`, clearing `pending`. `what` labels the warning
/// logged when the command failed.
fn take_reply_line(
    pending: &mut Option<CommandId>,
    events: &[TransportEvent],
    what: &str,
) -> Option<String> {
    for event in events {
        if let TransportEvent::Protocol(ClientEvent::CommandComplete { id, ok, output, .. }) = event
            && *pending == Some(*id)
        {
            *pending = None;
            if *ok {
                return output
                    .iter()
                    .map(|line| line.trim())
                    .find(|line| !line.is_empty())
                    .map(str::to_owned);
            }
            tracing::warn!("{what} query command failed");
            return None;
        }
    }
    None
}

/// Returns the client name from a `CommandComplete` whose id matches
/// `pending` (first non-empty trimmed output line), and clears `pending`.
///
/// Iterates `events` and looks for `CommandComplete { ok: true, .. }` whose
/// id equals `*pending`. On a match the first non-empty trimmed output line is
/// returned and `*pending` is set to `None`. Returns `None` when no matching
/// event exists in the batch or the output is blank.
pub(crate) fn take_client_name(
    pending: &mut Option<CommandId>,
    events: &[TransportEvent],
) -> Option<String> {
    take_reply_line(pending, events, "client-name")
}

/// Returns the tmux server version from a `CommandComplete` whose id matches
/// `pending` (first non-empty trimmed output line), and clears `pending`.
pub(crate) fn take_version(
    pending: &mut Option<CommandId>,
    events: &[TransportEvent],
) -> Option<String> {
    take_reply_line(pending, events, "version")
}

/// Returns the `aggressive-resize` option value from a `CommandComplete` whose
/// id matches `pending` (first non-empty trimmed output line), and clears `pending`.
pub(crate) fn take_aggressive_resize(
    pending: &mut Option<CommandId>,
    events: &[TransportEvent],
) -> Option<String> {
    take_reply_line(pending, events, "aggressive-resize")
}

/// Drains matching `capture-pane` replies from `events`.
///
/// For every `CommandComplete` whose id is in `capture_pending`:
/// - If the pane has a cursor-position query in-flight (`panes_with_cursor_pending`),
///   the captured lines are stored in `capture_awaiting_cursor` and emitted later
///   by [`take_cursor_positions`] once the cursor reply arrives.
/// - Otherwise the pane is emitted immediately as a [`PaneOutput`] (original
///   behaviour when cursor querying is unavailable or already drained).
///
/// tmux `-CC` does not replay existing content on attach, so this seeds the
/// first paint; the live `%output` stream keeps it current thereafter.
pub(crate) fn take_pane_captures(
    capture_pending: &mut HashMap<CommandId, PaneId>,
    capture_awaiting_cursor: &mut HashMap<PaneId, Vec<String>>,
    panes_with_cursor_pending: &HashSet<PaneId>,
    events: &[TransportEvent],
) -> Vec<PaneOutput> {
    let mut out = Vec::new();
    for event in events {
        if let TransportEvent::Protocol(ClientEvent::CommandComplete { id, ok, output, .. }) = event
            && let Some(pane) = capture_pending.remove(id)
        {
            if *ok {
                if panes_with_cursor_pending.contains(&pane) {
                    capture_awaiting_cursor.insert(pane, output.clone());
                } else {
                    out.push(PaneOutput {
                        pane,
                        data: capture_to_bytes(output),
                    });
                }
            } else {
                tracing::warn!(pane = pane.0, "capture-pane command failed");
            }
        }
    }
    out
}

/// Drains matching cursor-position `display-message` replies from `events`,
/// pairing each with its cached capture content and returning the combined
/// [`PaneOutput`] with the real cursor position restored.
///
/// FIFO ordering guarantees the capture reply arrives before this cursor reply,
/// so `capture_awaiting_cursor` should always contain the entry. If it does not
/// (e.g. capture failed), the cursor reply is silently dropped.
pub(crate) fn take_cursor_positions(
    cursor_pending: &mut HashMap<CommandId, PaneId>,
    panes_with_cursor_pending: &mut HashSet<PaneId>,
    capture_awaiting_cursor: &mut HashMap<PaneId, Vec<String>>,
    events: &[TransportEvent],
) -> Vec<PaneOutput> {
    let mut out = Vec::new();
    for event in events {
        if let TransportEvent::Protocol(ClientEvent::CommandComplete { id, ok, output, .. }) = event
            && let Some(pane) = cursor_pending.remove(id)
        {
            panes_with_cursor_pending.remove(&pane);
            let Some(lines) = capture_awaiting_cursor.remove(&pane) else {
                continue;
            };
            let (cx, cy) = if *ok {
                parse_cursor_pos(output).unwrap_or((0, 0))
            } else {
                tracing::warn!(pane = pane.0, "cursor-position query failed");
                (0, 0)
            };
            out.push(PaneOutput {
                pane,
                data: capture_to_bytes_with_cursor(&lines, cx, cy),
            });
        }
    }
    out
}

/// Parses a `'#{cursor_x} #{cursor_y}'` reply line into `(col, row)`.
fn parse_cursor_pos(output: &[String]) -> Option<(u16, u16)> {
    let line = output.first()?;
    let (x, y) = line.trim().split_once(' ')?;
    Some((x.parse().ok()?, y.parse().ok()?))
}

/// Joins `capture-pane -p -e` reply lines into VT bytes for seeding a pane's
/// screen: a cursor-home + clear-screen prefix so the snapshot repaints from a
/// clean grid (the reply can arrive after live `%output` has already moved the
/// cursor — without the reset the rows would stack and duplicate), then the rows
/// CRLF-joined (the reply omits line terminators).
fn capture_to_bytes(lines: &[String]) -> Vec<u8> {
    let mut bytes = b"\x1b[H\x1b[2J".to_vec();
    bytes.extend_from_slice(lines.join("\r\n").as_bytes());
    bytes
}

/// Like [`capture_to_bytes`] but appends a CSI cursor-position escape (`ESC[row;colH`,
/// 1-origin) so the rendered cursor matches the real tmux pane cursor after the
/// snapshot is painted.
fn capture_to_bytes_with_cursor(lines: &[String], cx: u16, cy: u16) -> Vec<u8> {
    let mut bytes = capture_to_bytes(lines);
    bytes.extend_from_slice(format!("\x1b[{};{}H", cy + 1, cx + 1).as_bytes());
    bytes
}

/// Returns the active `(window, pane)` from a `CommandComplete` whose id matches
/// `pending` (parsing the `@N %M` reply line), clearing `pending`.
///
/// Used to seed the `ActivePane` marker on attach, since tmux does not emit
/// `%window-pane-changed` then. Returns `None` when no matching reply is in the
/// batch or the line does not parse.
pub(crate) fn take_active_pane(
    pending: &mut Option<CommandId>,
    events: &[TransportEvent],
) -> Option<(WindowId, PaneId)> {
    for event in events {
        if let TransportEvent::Protocol(ClientEvent::CommandComplete { id, ok, output, .. }) = event
            && *pending == Some(*id)
        {
            *pending = None;
            if *ok {
                return output.iter().find_map(|line| parse_active_pane(line));
            }
            tracing::warn!("active-pane query command failed");
            return None;
        }
    }
    None
}

/// Returns parsed key bindings from a `CommandComplete` matching `pending`
/// (running `parse_list_keys` on the reply), clearing `pending`. Returns `None`
/// when no matching reply is in the batch.
pub(crate) fn take_keybindings(
    pending: &mut Option<CommandId>,
    events: &[TransportEvent],
) -> Option<Vec<KeyBinding>> {
    for event in events {
        if let TransportEvent::Protocol(ClientEvent::CommandComplete { id, ok, output, .. }) = event
            && *pending == Some(*id)
        {
            *pending = None;
            if *ok {
                return Some(parse_list_keys(output));
            }
            tracing::warn!("list-keys command failed");
            return None;
        }
    }
    None
}

/// Returns the prefix-key set from a `CommandComplete` matching `pending`
/// (running `parse_prefix` on the first reply line), clearing `pending`.
pub(crate) fn take_prefix_keys(
    pending: &mut Option<CommandId>,
    events: &[TransportEvent],
) -> Option<HashSet<String>> {
    for event in events {
        if let TransportEvent::Protocol(ClientEvent::CommandComplete { id, ok, output, .. }) = event
            && *pending == Some(*id)
        {
            *pending = None;
            if *ok {
                return Some(
                    output
                        .first()
                        .map(|line| parse_prefix(line))
                        .unwrap_or_default(),
                );
            }
            tracing::warn!("prefix query command failed");
            return None;
        }
    }
    None
}

/// Returns the `ModeKeys` from a `CommandComplete` matching `pending`
/// (parsing `#{mode-keys}`), clearing `pending`.
pub(crate) fn take_mode_keys(
    pending: &mut Option<CommandId>,
    events: &[TransportEvent],
) -> Option<ModeKeys> {
    for event in events {
        if let TransportEvent::Protocol(ClientEvent::CommandComplete { id, ok, output, .. }) = event
            && *pending == Some(*id)
        {
            *pending = None;
            if *ok {
                return Some(
                    output
                        .first()
                        .map(|l| ModeKeys::parse(l))
                        .unwrap_or_default(),
                );
            }
            tracing::warn!("mode-keys query failed");
            return None;
        }
    }
    None
}

/// Returns the new session id if `events` contains a session-change to an id
/// different from `current`, i.e. a real `switch-client`. Returns `None` on the
/// first attach (`current == None`) or when the id is unchanged, so the initial
/// enumeration is not duplicated and only an actual switch triggers a rebuild.
///
/// `%session-changed` and `%session-renamed` are always treated as a switch.
/// `%client-session-changed` is only treated as a switch when its `client`
/// field equals `own_client`; if `own_client` is `None` (not yet known),
/// `%client-session-changed` is ignored to avoid spurious teardown from
/// foreign-client events arriving before the own client name is resolved.
///
/// The switch decision lives here (driven from the per-frame drain) rather than
/// in the `on_session_changed` observer, because the teardown + re-enumeration
/// it triggers need the event batch and the live `NonSend` client, which an
/// observer cannot access.
pub(crate) fn detect_session_switch(
    events: &[TransportEvent],
    current: Option<SessionId>,
    own_client: Option<&str>,
) -> Option<SessionId> {
    let current = current?;
    for event in events {
        let next = match event {
            TransportEvent::Protocol(ClientEvent::Notification(ControlEvent::SessionChanged {
                session,
                ..
            })) => *session,
            TransportEvent::Protocol(ClientEvent::Notification(
                ControlEvent::ClientSessionChanged {
                    client, session, ..
                },
            )) => {
                if own_client == Some(client.as_str()) {
                    *session
                } else {
                    continue;
                }
            }
            _ => continue,
        };
        if next != current {
            return Some(next);
        }
    }
    None
}

/// True when the batch contains a `%session-window-changed` — the session's
/// current window changed (`next-window` / `previous-window` / `select-window`).
///
/// NOTE: tmux emits *only* `%session-window-changed` for such a switch, never a
/// `%window-pane-changed`, so the caller must re-query the active pane
/// (`active_pane_command`) to move `ActiveWindow`/`ActivePane`. Without that the
/// switch never reaches the projection and the UI stays on the old window.
pub(crate) fn detect_window_switch(events: &[TransportEvent]) -> bool {
    events.iter().any(|event| {
        matches!(
            event,
            TransportEvent::Protocol(ClientEvent::Notification(
                ControlEvent::SessionWindowChanged { .. }
            ))
        )
    })
}

/// True when the batch contains a `%window-add` — a window was created
/// (`new-window`).
///
/// NOTE: tmux does NOT emit a `%layout-change` for a freshly added window
/// (verified against tmux 3.6a: `new-window` sends only `%window-add` +
/// `%session-window-changed` + `%output`), so the new window's pane layout
/// never arrives via notifications. The caller must re-enumerate
/// (`list-windows`) to fetch the layout and project the pane; without it the
/// new window has no pane entity and renders black.
pub(crate) fn detect_window_added(events: &[TransportEvent]) -> bool {
    events.iter().any(|event| {
        matches!(
            event,
            TransportEvent::Protocol(ClientEvent::Notification(ControlEvent::WindowAdd { .. }))
        )
    })
}

/// Parses an `@N %M` line into `(WindowId, PaneId)`.
fn parse_active_pane(line: &str) -> Option<(WindowId, PaneId)> {
    let mut parts = line.split_whitespace();
    let window = parts.next()?.strip_prefix('@')?.parse().ok()?;
    let pane = parts.next()?.strip_prefix('%')?.parse().ok()?;
    Some((WindowId(window), PaneId(pane)))
}

fn trigger_notification(commands: &mut Commands, own_client: Option<&str>, event: &ControlEvent) {
    match event {
        ControlEvent::SessionChanged { session, name }
        | ControlEvent::SessionRenamed { session, name } => {
            commands.trigger(TmuxSessionChanged {
                session: *session,
                name: name.clone(),
            });
        }
        ControlEvent::ClientSessionChanged {
            client,
            session,
            name,
        } if own_client == Some(client.as_str()) => {
            commands.trigger(TmuxSessionChanged {
                session: *session,
                name: name.clone(),
            });
        }
        ControlEvent::WindowAdd { window } => {
            commands.trigger(TmuxWindowAdded {
                window: *window,
                index: 0,
                name: String::new(),
            });
        }
        ControlEvent::WindowClose { window } | ControlEvent::UnlinkedWindowClose { window } => {
            commands.trigger(TmuxWindowClosed { window: *window });
        }
        ControlEvent::WindowRenamed { window, name } => {
            commands.trigger(TmuxWindowRenamed {
                window: *window,
                name: name.clone(),
            });
        }
        ControlEvent::LayoutChange {
            window,
            visible_layout,
            ..
        } => {
            commands.trigger(TmuxLayoutChanged {
                window: *window,
                layout: visible_layout.clone(),
            });
        }
        ControlEvent::WindowPaneChanged { window, pane } => {
            commands.trigger(TmuxActivePaneChanged {
                window: *window,
                pane: *pane,
            });
        }
        ControlEvent::SubscriptionChanged {
            name,
            window: Some(window),
            value,
            ..
        } if name == WINDOW_FLAGS_SUBSCRIPTION => {
            commands.trigger(TmuxWindowFlagsChanged {
                window: *window,
                flags: WindowFlags::parse(value),
            });
        }
        _ => {}
    }
}

fn trigger_seed(commands: &mut Commands, output: &[String]) {
    let rows = match parse_window_rows(output) {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(error = %error, "failed to parse list-windows reply");
            return;
        }
    };
    let mut ids = Vec::with_capacity(rows.len());
    for row in &rows {
        commands.trigger(TmuxWindowAdded {
            window: row.id,
            index: row.index,
            name: row.name.clone(),
        });
        commands.trigger(TmuxWindowFlagsChanged {
            window: row.id,
            flags: row.flags,
        });
        commands.trigger(TmuxLayoutChanged {
            window: row.id,
            layout: row.layout.clone(),
        });
        if row.active {
            commands.trigger(TmuxActiveWindowChanged { window: row.id });
        }
        ids.push(row.id);
    }
    commands.trigger(TmuxWindowsRetained { windows: ids });
}

/// Emits a `tracing` line describing a single transport event.
fn log_transport_event(event: &TransportEvent) {
    match event {
        TransportEvent::Protocol(ClientEvent::CommandComplete { id, ok, .. }) => {
            tracing::debug!(?id, ok, "tmux command complete");
        }
        TransportEvent::Protocol(ClientEvent::Notification(notification)) => {
            tracing::debug!(?notification, "tmux notification");
        }
        TransportEvent::Closed { reason } => {
            tracing::info!(reason, "tmux transport closed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enumerate::EnumerationState;
    use bevy::prelude::*;
    use crossbeam_channel::unbounded;
    use tmux_control::{CommandId, ControlEvent};
    use tmux_control_parser::{SessionId, WindowId};

    fn window_add(id: u32) -> TransportEvent {
        TransportEvent::Protocol(ClientEvent::Notification(ControlEvent::WindowAdd {
            window: WindowId(id),
        }))
    }

    #[test]
    fn drain_then_advance_state_attaches() {
        let (tx, rx) = unbounded();
        tx.send(window_add(1)).unwrap();
        let drained = drain_transport(&rx);
        let next = advance_state(&ConnectionState::Connecting, &drained);
        assert_eq!(next, Some(ConnectionState::Attached));
    }

    #[test]
    fn take_client_name_extracts_from_matching_reply() {
        let events = vec![TransportEvent::Protocol(ClientEvent::CommandComplete {
            id: CommandId(7),
            number: 0,
            ok: true,
            output: vec!["main-client".to_string()],
        })];
        let mut pending = Some(CommandId(7));
        assert_eq!(
            take_client_name(&mut pending, &events),
            Some("main-client".to_string())
        );
        assert_eq!(pending, None);
    }

    #[test]
    fn take_pane_captures_seeds_matching_reply_as_output() {
        let events = vec![TransportEvent::Protocol(ClientEvent::CommandComplete {
            id: CommandId(5),
            number: 0,
            ok: true,
            output: vec!["line one".to_string(), "line two".to_string()],
        })];
        let mut capture_pending = HashMap::from([(CommandId(5), PaneId(88))]);
        let no_cursor_pending = HashSet::new();
        let mut awaiting = HashMap::new();
        let out = take_pane_captures(
            &mut capture_pending,
            &mut awaiting,
            &no_cursor_pending,
            &events,
        );
        assert_eq!(
            out,
            vec![PaneOutput {
                pane: PaneId(88),
                data: b"\x1b[H\x1b[2Jline one\r\nline two".to_vec(),
            }]
        );
        assert!(capture_pending.is_empty());
        assert!(awaiting.is_empty());
    }

    #[test]
    fn take_active_pane_parses_window_and_pane() {
        let events = vec![TransportEvent::Protocol(ClientEvent::CommandComplete {
            id: CommandId(9),
            number: 0,
            ok: true,
            output: vec!["@7 %88".to_string()],
        })];
        let mut pending = Some(CommandId(9));
        assert_eq!(
            take_active_pane(&mut pending, &events),
            Some((WindowId(7), PaneId(88)))
        );
        assert_eq!(pending, None);
    }

    #[test]
    fn take_pane_captures_drops_failed_reply_without_output() {
        let events = vec![TransportEvent::Protocol(ClientEvent::CommandComplete {
            id: CommandId(5),
            number: 0,
            ok: false,
            output: vec![],
        })];
        let mut capture_pending = HashMap::from([(CommandId(5), PaneId(88))]);
        let no_cursor_pending = HashSet::new();
        let mut awaiting = HashMap::new();
        let out = take_pane_captures(
            &mut capture_pending,
            &mut awaiting,
            &no_cursor_pending,
            &events,
        );
        assert!(out.is_empty());
        assert!(
            capture_pending.is_empty(),
            "failed capture is still cleared"
        );
        assert!(
            awaiting.is_empty(),
            "failed capture must not populate awaiting"
        );
    }

    #[test]
    fn take_pane_captures_caches_when_cursor_pending() {
        let events = vec![TransportEvent::Protocol(ClientEvent::CommandComplete {
            id: CommandId(5),
            number: 0,
            ok: true,
            output: vec!["line one".to_string()],
        })];
        let mut capture_pending = HashMap::from([(CommandId(5), PaneId(88))]);
        let cursor_pending = HashSet::from([PaneId(88)]);
        let mut awaiting = HashMap::new();
        let out = take_pane_captures(
            &mut capture_pending,
            &mut awaiting,
            &cursor_pending,
            &events,
        );
        assert!(out.is_empty(), "should cache, not emit");
        assert_eq!(
            awaiting.get(&PaneId(88)),
            Some(&vec!["line one".to_string()])
        );
    }

    #[test]
    fn take_cursor_positions_emits_with_cursor_escape() {
        let cursor_event = TransportEvent::Protocol(ClientEvent::CommandComplete {
            id: CommandId(6),
            number: 0,
            ok: true,
            output: vec!["3 5".to_string()],
        });
        let mut cursor_pending = HashMap::from([(CommandId(6), PaneId(88))]);
        let mut panes_with_cursor = HashSet::from([PaneId(88)]);
        let mut awaiting = HashMap::from([(PaneId(88), vec!["hello".to_string()])]);
        let out = take_cursor_positions(
            &mut cursor_pending,
            &mut panes_with_cursor,
            &mut awaiting,
            &[cursor_event],
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].pane, PaneId(88));
        // ESC[H ESC[2J + "hello" + ESC[6;4H  (cy=5→row 6, cx=3→col 4, 1-origin)
        assert_eq!(out[0].data, b"\x1b[H\x1b[2Jhello\x1b[6;4H".to_vec());
        assert!(cursor_pending.is_empty());
        assert!(panes_with_cursor.is_empty());
        assert!(awaiting.is_empty());
    }

    #[test]
    fn take_client_name_unmatched_id_returns_none_and_keeps_pending() {
        let events = vec![TransportEvent::Protocol(ClientEvent::CommandComplete {
            id: CommandId(8),
            number: 0,
            ok: true,
            output: vec!["main-client".to_string()],
        })];
        let mut pending = Some(CommandId(7));
        assert_eq!(take_client_name(&mut pending, &events), None);
        assert_eq!(pending, Some(CommandId(7)));
    }

    #[test]
    fn take_version_extracts_first_line() {
        let id = CommandId(7);
        let mut pending = Some(id);
        let events = vec![TransportEvent::Protocol(ClientEvent::CommandComplete {
            id,
            number: 0,
            ok: true,
            output: vec!["3.6a".to_string()],
        })];
        assert_eq!(
            take_version(&mut pending, &events),
            Some("3.6a".to_string())
        );
        assert_eq!(pending, None);
    }

    #[test]
    fn take_client_name_trims_whitespace_from_output() {
        let events = vec![TransportEvent::Protocol(ClientEvent::CommandComplete {
            id: CommandId(3),
            number: 0,
            ok: true,
            output: vec!["  /dev/ttys001  ".to_string()],
        })];
        let mut pending = Some(CommandId(3));
        assert_eq!(
            take_client_name(&mut pending, &events),
            Some("/dev/ttys001".to_string())
        );
    }

    #[test]
    fn client_session_changed_triggers_session_changed() {
        use crate::events::TmuxSessionChanged;
        use std::sync::{Arc, Mutex};
        use tmux_control_parser::SessionId;

        #[derive(Resource, Default, Clone)]
        struct Seen(Arc<Mutex<Vec<(u32, String)>>>);

        #[derive(Resource)]
        struct Batch(Vec<TransportEvent>);

        fn run(mut commands: Commands, mut pending: ResMut<EnumerationState>, batch: Res<Batch>) {
            trigger_events(&mut commands, &mut pending.pending, &batch.0, Some("main"));
        }

        let mut app = App::new();
        app.init_resource::<Seen>();
        app.init_resource::<EnumerationState>();
        app.insert_resource(Batch(vec![TransportEvent::Protocol(
            ClientEvent::Notification(ControlEvent::ClientSessionChanged {
                client: "main".to_string(),
                session: SessionId(9),
                name: "beta".to_string(),
            }),
        )]));
        app.add_observer(|ev: On<TmuxSessionChanged>, seen: Res<Seen>| {
            seen.0.lock().unwrap().push((ev.session.0, ev.name.clone()));
        });
        app.add_systems(Update, run);
        let seen = app.world().resource::<Seen>().clone();
        app.update();

        assert_eq!(*seen.0.lock().unwrap(), vec![(9, "beta".to_string())]);
    }

    fn client_session_changed(client: &str, session: SessionId) -> TransportEvent {
        TransportEvent::Protocol(ClientEvent::Notification(
            ControlEvent::ClientSessionChanged {
                client: client.to_string(),
                session,
                name: "s".to_string(),
            },
        ))
    }

    fn session_changed(session: SessionId) -> TransportEvent {
        TransportEvent::Protocol(ClientEvent::Notification(ControlEvent::SessionChanged {
            session,
            name: "s".to_string(),
        }))
    }

    #[test]
    fn foreign_client_session_changed_is_ignored() {
        let events = vec![client_session_changed("other-client", SessionId(9))];
        assert_eq!(
            detect_session_switch(&events, Some(SessionId(1)), Some("ozmux-0")),
            None
        );
    }

    #[test]
    fn own_client_session_changed_is_a_switch() {
        let events = vec![client_session_changed("ozmux-0", SessionId(9))];
        assert_eq!(
            detect_session_switch(&events, Some(SessionId(1)), Some("ozmux-0")),
            Some(SessionId(9))
        );
    }

    #[test]
    fn client_session_changed_ignored_when_own_name_unknown() {
        let events = vec![client_session_changed("ozmux-0", SessionId(9))];
        assert_eq!(
            detect_session_switch(&events, Some(SessionId(1)), None),
            None
        );
    }

    #[test]
    fn plain_session_changed_is_a_switch_regardless_of_name() {
        let events = vec![session_changed(SessionId(9))];
        assert_eq!(
            detect_session_switch(&events, Some(SessionId(1)), None),
            Some(SessionId(9))
        );
    }

    #[test]
    fn detect_session_switch_reports_new_id_only_on_change() {
        use tmux_control_parser::SessionId;
        let changed = vec![TransportEvent::Protocol(ClientEvent::Notification(
            ControlEvent::SessionChanged {
                session: SessionId(2),
                name: "b".to_string(),
            },
        ))];
        assert_eq!(detect_session_switch(&changed, None, None), None);
        assert_eq!(
            detect_session_switch(&changed, Some(SessionId(2)), None),
            None
        );
        assert_eq!(
            detect_session_switch(&changed, Some(SessionId(1)), None),
            Some(SessionId(2))
        );
        assert_eq!(detect_session_switch(&[], Some(SessionId(1)), None), None);

        let client_changed = vec![TransportEvent::Protocol(ClientEvent::Notification(
            ControlEvent::ClientSessionChanged {
                client: "main".to_string(),
                session: SessionId(3),
                name: "c".to_string(),
            },
        ))];
        assert_eq!(
            detect_session_switch(&client_changed, Some(SessionId(1)), Some("main")),
            Some(SessionId(3))
        );
        assert_eq!(
            detect_session_switch(&client_changed, Some(SessionId(3)), Some("main")),
            None
        );
    }

    #[test]
    fn detect_window_switch_flags_session_window_changed() {
        use tmux_control_parser::{SessionId, WindowId};
        let switched = vec![TransportEvent::Protocol(ClientEvent::Notification(
            ControlEvent::SessionWindowChanged {
                session: SessionId(1),
                window: WindowId(4),
            },
        ))];
        assert!(detect_window_switch(&switched));
        assert!(!detect_window_switch(&[]));

        let session_changed = vec![TransportEvent::Protocol(ClientEvent::Notification(
            ControlEvent::SessionChanged {
                session: SessionId(2),
                name: "b".to_string(),
            },
        ))];
        assert!(!detect_window_switch(&session_changed));
    }

    #[test]
    fn detect_window_added_flags_window_add() {
        use tmux_control_parser::WindowId;
        let added = vec![TransportEvent::Protocol(ClientEvent::Notification(
            ControlEvent::WindowAdd {
                window: WindowId(7),
            },
        ))];
        assert!(detect_window_added(&added));
        assert!(!detect_window_added(&[]));

        let closed = vec![TransportEvent::Protocol(ClientEvent::Notification(
            ControlEvent::WindowClose {
                window: WindowId(7),
            },
        ))];
        assert!(!detect_window_added(&closed));
    }

    #[test]
    fn unlinked_window_close_triggers_window_closed() {
        use crate::events::TmuxWindowClosed;
        use std::sync::{Arc, Mutex};
        use tmux_control_parser::WindowId;

        #[derive(Resource, Clone)]
        struct Sink(Arc<Mutex<Vec<WindowId>>>);

        let mut app = App::new();
        let sink = Sink(Arc::new(Mutex::new(Vec::new())));
        app.insert_resource(sink.clone());
        app.add_observer(|ev: On<TmuxWindowClosed>, sink: Res<Sink>| {
            sink.0.lock().unwrap().push(ev.window);
        });
        app.add_systems(Update, |mut commands: Commands| {
            trigger_notification(
                &mut commands,
                None,
                &ControlEvent::UnlinkedWindowClose {
                    window: WindowId(3),
                },
            );
        });
        app.update();

        assert_eq!(*sink.0.lock().unwrap(), vec![WindowId(3)]);
    }

    #[test]
    fn seed_reply_triggers_per_row_events_then_retain() {
        use crate::events::{TmuxLayoutChanged, TmuxWindowAdded, TmuxWindowsRetained};
        use std::sync::{Arc, Mutex};

        #[derive(Resource, Default, Clone)]
        struct Log(Arc<Mutex<Vec<String>>>);

        #[derive(Resource)]
        struct Batch(Vec<TransportEvent>);

        fn run(
            mut commands: Commands,
            mut enumeration: ResMut<EnumerationState>,
            batch: Res<Batch>,
        ) {
            trigger_events(&mut commands, &mut enumeration.pending, &batch.0, None);
        }

        let mut app = App::new();
        app.init_resource::<Log>();
        app.init_resource::<EnumerationState>();
        app.world_mut().resource_mut::<EnumerationState>().pending = Some(CommandId(1));
        app.insert_resource(Batch(vec![TransportEvent::Protocol(
            ClientEvent::CommandComplete {
                id: CommandId(1),
                number: 0,
                ok: true,
                output: vec!["1\t@1\t0\tabcd,80x24,0,0,5\t0000,80x24,0,0,5\t\tmain".to_string()],
            },
        )]));
        app.add_observer(|ev: On<TmuxWindowAdded>, log: Res<Log>| {
            log.0.lock().unwrap().push(format!("add@{}", ev.window.0));
        });
        app.add_observer(|ev: On<TmuxLayoutChanged>, log: Res<Log>| {
            log.0
                .lock()
                .unwrap()
                .push(format!("layout@{}", ev.window.0));
        });
        app.add_observer(|ev: On<TmuxWindowsRetained>, log: Res<Log>| {
            log.0
                .lock()
                .unwrap()
                .push(format!("retain{}", ev.windows.len()));
        });
        app.add_systems(Update, run);

        let log = app.world().resource::<Log>().clone();
        app.update();

        assert_eq!(*log.0.lock().unwrap(), vec!["add@1", "layout@1", "retain1"]);
        assert_eq!(app.world().resource::<EnumerationState>().pending, None);
    }

    #[test]
    fn take_keybindings_parses_matching_reply() {
        let events = vec![TransportEvent::Protocol(ClientEvent::CommandComplete {
            id: CommandId(11),
            number: 0,
            ok: true,
            output: vec!["bind-key -T root M-i split-window -h".to_string()],
        })];
        let mut pending = Some(CommandId(11));
        let got = take_keybindings(&mut pending, &events).expect("a parsed reply");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].key, "M-i");
        assert_eq!(got[0].command, "split-window -h");
        assert_eq!(pending, None);
    }

    #[test]
    fn take_keybindings_unmatched_keeps_pending() {
        let events = vec![TransportEvent::Protocol(ClientEvent::CommandComplete {
            id: CommandId(2),
            number: 0,
            ok: true,
            output: vec![],
        })];
        let mut pending = Some(CommandId(11));
        assert!(take_keybindings(&mut pending, &events).is_none());
        assert_eq!(pending, Some(CommandId(11)));
    }

    #[test]
    fn take_prefix_keys_parses_matching_reply() {
        let events = vec![TransportEvent::Protocol(ClientEvent::CommandComplete {
            id: CommandId(12),
            number: 0,
            ok: true,
            output: vec!["C-b None".to_string()],
        })];
        let mut pending = Some(CommandId(12));
        let got = take_prefix_keys(&mut pending, &events).expect("a parsed reply");
        assert_eq!(got, std::collections::HashSet::from(["C-b".to_string()]));
        assert_eq!(pending, None);
    }

    #[test]
    fn take_mode_keys_parses_vi() {
        let events = vec![TransportEvent::Protocol(ClientEvent::CommandComplete {
            id: CommandId(21),
            number: 0,
            ok: true,
            output: vec!["vi".to_string()],
        })];
        let mut pending = Some(CommandId(21));
        assert_eq!(take_mode_keys(&mut pending, &events), Some(ModeKeys::Vi));
        assert_eq!(pending, None);
    }

    #[test]
    fn take_mode_keys_defaults_emacs_on_other() {
        let events = vec![TransportEvent::Protocol(ClientEvent::CommandComplete {
            id: CommandId(22),
            number: 0,
            ok: true,
            output: vec!["emacs".to_string()],
        })];
        let mut pending = Some(CommandId(22));
        assert_eq!(take_mode_keys(&mut pending, &events), Some(ModeKeys::Emacs));
        assert_eq!(pending, None);
    }

    #[test]
    fn session_renamed_maps_to_session_changed() {
        use crate::events::TmuxSessionChanged;
        use std::sync::{Arc, Mutex};
        use tmux_control_parser::SessionId;

        #[derive(Resource, Default, Clone)]
        struct Captured(Arc<Mutex<Vec<(u32, String)>>>);

        let mut app = App::new();
        app.init_resource::<Captured>();
        app.add_observer(|ev: On<TmuxSessionChanged>, captured: Res<Captured>| {
            captured
                .0
                .lock()
                .unwrap()
                .push((ev.session.0, ev.name.clone()));
        });
        app.add_systems(Update, |mut commands: Commands| {
            trigger_notification(
                &mut commands,
                None,
                &ControlEvent::SessionRenamed {
                    session: SessionId(1),
                    name: "renamed".to_string(),
                },
            );
        });

        let captured = app.world().resource::<Captured>().clone();
        app.update();

        assert_eq!(
            *captured.0.lock().unwrap(),
            vec![(1, "renamed".to_string())]
        );
    }

    #[test]
    fn window_flags_subscription_triggers_flags_changed() {
        use crate::components::WindowFlags;
        use crate::enumerate::WINDOW_FLAGS_SUBSCRIPTION;
        use crate::events::TmuxWindowFlagsChanged;
        use std::sync::{Arc, Mutex};

        #[derive(Resource, Default, Clone)]
        struct Captured(Arc<Mutex<Vec<(WindowId, WindowFlags)>>>);

        #[derive(Resource)]
        struct Batch(Vec<TransportEvent>);

        fn run(mut commands: Commands, mut pending: ResMut<EnumerationState>, batch: Res<Batch>) {
            trigger_events(&mut commands, &mut pending.pending, &batch.0, None);
        }

        let line = format!("%subscription-changed {WINDOW_FLAGS_SUBSCRIPTION} $1 @2 0 - : *Z");
        let notification = ControlEvent::parse(line.as_bytes()).unwrap();
        let event = TransportEvent::Protocol(ClientEvent::Notification(notification));

        let mut app = App::new();
        app.init_resource::<Captured>();
        app.init_resource::<EnumerationState>();
        app.insert_resource(Batch(vec![event]));
        app.add_observer(|ev: On<TmuxWindowFlagsChanged>, captured: Res<Captured>| {
            captured.0.lock().unwrap().push((ev.window, ev.flags));
        });
        app.add_systems(Update, run);

        let captured = app.world().resource::<Captured>().clone();
        app.update();

        assert_eq!(
            *captured.0.lock().unwrap(),
            vec![(WindowId(2), WindowFlags::ZOOM)]
        );
    }
}
