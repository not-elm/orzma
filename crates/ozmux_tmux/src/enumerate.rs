//! Parsing the `list-windows -F` reply used to enumerate windows on attach.

use crate::components::WindowFlags;
use crate::input::quote;
use crate::state_restore::{GridCapture, PaneState, restore_to_bytes};
use bevy::ecs::component::Component;
use std::collections::HashMap;
use tmux_control::{CommandId, TmuxResult};
use tmux_control_parser::{PaneId, WindowId, WindowLayout};

/// The `-F` format ozmux sends to enumerate windows. Tab-separated, with the
/// free-text `window_name` LAST so a `splitn(7, '\t')` keeps it intact.
pub const LIST_WINDOWS_FORMAT: &str = "#{window_active}\t#{window_id}\t#{window_index}\t#{window_layout}\t#{window_visible_layout}\t#{window_raw_flags}\t#{window_name}";

/// One parsed row of the `list-windows` reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowRow {
    /// tmux window id (`@N`).
    pub id: WindowId,
    /// Whether this is the session's active window.
    pub active: bool,
    /// tmux display index (#{window_index}), e.g. 0, 1, 2.
    pub index: u32,
    /// Window name.
    pub name: String,
    /// tmux per-window flags (`#{window_raw_flags}`).
    pub flags: WindowFlags,
    /// Parsed structural layout (panes + geometry). Sourced from
    /// `window_visible_layout` when non-empty; falls back to `window_layout`.
    pub layout: WindowLayout,
}

/// Parses the lines of a `list-windows -F LIST_WINDOWS_FORMAT` reply.
///
/// Each line is `active \t id \t index \t layout \t visible_layout \t raw_flags \t name`.
/// When `visible_layout` is non-empty it is used for `WindowRow.layout`; otherwise
/// `layout` is the fallback. Blank lines are skipped.
/// Returns a descriptive `Err(String)` on a malformed row.
pub fn parse_window_rows(lines: &[String]) -> Result<Vec<WindowRow>, String> {
    let mut rows = Vec::new();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        rows.push(parse_row(line)?);
    }
    Ok(rows)
}

fn parse_row(line: &str) -> Result<WindowRow, String> {
    let mut fields = line.splitn(7, '\t');
    let active = fields.next().is_some_and(|f| f == "1");
    let id = fields
        .next()
        .and_then(parse_window_id)
        .ok_or_else(|| format!("bad window id in row: {line}"))?;
    let index = fields
        .next()
        .and_then(|f| f.parse::<u32>().ok())
        .ok_or_else(|| format!("bad window index in row: {line}"))?;
    let layout_field = fields
        .next()
        .ok_or_else(|| format!("missing layout in row: {line}"))?;
    let visible_field = fields
        .next()
        .ok_or_else(|| format!("missing visible layout in row: {line}"))?;
    let chosen = if visible_field.trim().is_empty() {
        layout_field
    } else {
        visible_field
    };
    let layout = WindowLayout::parse(chosen.as_bytes())
        .map_err(|e| format!("bad layout in row {line}: {e}"))?;
    let flags = WindowFlags::parse(
        fields
            .next()
            .ok_or_else(|| format!("missing flags in row: {line}"))?,
    );
    let name = fields
        .next()
        .ok_or_else(|| format!("missing name in row: {line}"))?
        .to_string();
    Ok(WindowRow {
        id,
        active,
        index,
        name,
        flags,
        layout,
    })
}

fn parse_window_id(field: &str) -> Option<WindowId> {
    Some(WindowId(field.strip_prefix('@')?.parse().ok()?))
}

/// Returns whether `version` supports per-window `refresh-client -C @win:WxH`
/// (tmux ≥ 3.4). Parses leniently: the leading `major.minor`, tolerating a
/// `next-` prefix and a trailing letter suffix like `3.6a`.
pub(crate) fn version_supports_per_window_refresh(version: &str) -> bool {
    parse_major_minor(version).is_some_and(|mm| mm >= (3, 4))
}

fn parse_major_minor(version: &str) -> Option<(u32, u32)> {
    let trimmed = version
        .trim()
        .trim_start_matches(|c: char| !c.is_ascii_digit());
    let mut parts = trimmed.split('.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor_digits: String = parts
        .next()?
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    let minor: u32 = minor_digits.parse().ok()?;
    Some((major, minor))
}

/// The name of the control-mode subscription that streams every window's
/// `#{window_raw_flags}` back as `%subscription-changed`.
pub(crate) const WINDOW_FLAGS_SUBSCRIPTION: &str = "ozmux-window-flags";

pub(crate) fn rename_command(verb: &str, sigil: char, id: u32, name: &str) -> String {
    format!("{verb} -t {sigil}{id} -- {}", quote(name))
}

/// What an in-flight command's reply will populate, keyed by its `CommandId`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PendingReply {
    /// `list-windows` enumeration → per-row projection seed.
    ListWindows,
    /// `display-message #{client_name}`.
    ClientName,
    /// `display-message #{version}`.
    Version,
    /// `display-message #{window_id} #{pane_id}` active-pane query.
    ActivePane,
    /// `aggressive-resize` option query → warn if `on`.
    AggressiveResize,
    /// The default (base) capture of a pane restore.
    RestoreBase { pane: PaneId },
    /// The `-a` saved-primary capture of a pane restore.
    RestoreSavedPrimary { pane: PaneId },
    /// The terminal-state (`PANE_STATE_FORMAT`) query of a pane restore.
    RestoreState { pane: PaneId },
    /// The pending-output capture of a pane restore.
    RestorePending { pane: PaneId },
}

/// One in-flight reply slot of a pane restore.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Slot<T> {
    NotRequested,
    Pending,
    Ok(T),
    Failed,
}

impl<T> Slot<T> {
    fn is_pending(&self) -> bool {
        matches!(self, Slot::Pending)
    }
    fn ok(self) -> Option<T> {
        match self {
            Slot::Ok(value) => Some(value),
            _ => None,
        }
    }
}

/// Accumulates the restore replies for one pane until every requested slot
/// resolves, then synthesizes the seed bytes exactly once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PaneRestore {
    pub(crate) pane_height: u16,
    pub(crate) base: Slot<Vec<String>>,
    pub(crate) saved_primary: Slot<Vec<String>>,
    pub(crate) state: Slot<PaneState>,
    pub(crate) pending: Slot<Vec<String>>,
}

impl PaneRestore {
    /// Buffer for the adopt-time full restore (all four commands issued).
    pub(crate) fn new_full(pane_height: u16) -> Self {
        Self {
            pane_height,
            base: Slot::Pending,
            saved_primary: Slot::Pending,
            state: Slot::Pending,
            pending: Slot::Pending,
        }
    }

    /// Buffer for the light re-seed (visible capture + state only).
    pub(crate) fn new_light(pane_height: u16) -> Self {
        Self {
            pane_height,
            base: Slot::Pending,
            saved_primary: Slot::NotRequested,
            state: Slot::Pending,
            pending: Slot::NotRequested,
        }
    }

    /// Whether every requested slot has resolved (`Ok` or `Failed`).
    pub(crate) fn complete(&self) -> bool {
        !self.base.is_pending()
            && !self.saved_primary.is_pending()
            && !self.state.is_pending()
            && !self.pending.is_pending()
    }

    /// Synthesizes the seed bytes from whatever slots succeeded.
    pub(crate) fn into_bytes(self) -> Vec<u8> {
        let full = !matches!(&self.saved_primary, Slot::NotRequested);
        let state = self.state.ok();
        let pending = self.pending.ok().unwrap_or_default();
        let base = self.base.ok().unwrap_or_default();
        let grid = if full {
            GridCapture::Full {
                base,
                saved_primary: self.saved_primary.ok().unwrap_or_default(),
                pane_height: self.pane_height,
            }
        } else {
            GridCapture::VisibleOnly { rows: base }
        };
        restore_to_bytes(&grid, state.as_ref(), &pending)
    }
}

/// Correlates in-flight enumeration/query commands by [`CommandId`] and the
/// per-pane restore buffers, so each drained reply routes to its handler.
#[derive(Component, Default)]
pub(crate) struct EnumerationState {
    pub(crate) pending: HashMap<CommandId, PendingReply>,
    pub(crate) aggressive_resize_checked: bool,
    pub(crate) restores: HashMap<PaneId, PaneRestore>,
}

impl EnumerationState {
    /// Records `reply` under the id `send` returned, logging on send failure.
    pub(crate) fn register(&mut self, send: TmuxResult<CommandId>, reply: PendingReply) {
        match send {
            Ok(id) => {
                // NOTE: singleton query kinds keep the old `Option` last-write-wins
                // — a re-issued query must supersede any still-in-flight one of the
                // same kind, or BOTH ids stay in `pending` and dispatch twice (a
                // re-sent list-windows on %window-add while the attach enumeration
                // is still in flight would fire trigger_seed twice, and a re-queried
                // active-pane would fire TmuxActivePaneChanged twice). The per-pane
                // Restore kinds are legitimately multi and exempt.
                if !matches!(
                    reply,
                    PendingReply::RestoreBase { .. }
                        | PendingReply::RestoreSavedPrimary { .. }
                        | PendingReply::RestoreState { .. }
                        | PendingReply::RestorePending { .. }
                ) {
                    self.pending.retain(|_, r| *r != reply);
                }
                self.pending.insert(id, reply);
            }
            Err(error) => tracing::warn!(?error, ?reply, "failed to send tmux query"),
        }
    }

    /// Whether a reply of `reply`'s kind is already in flight (replaces the old
    /// `Option::is_some` singleton guard for client-name / aggressive-resize).
    pub(crate) fn has_pending(&self, reply: PendingReply) -> bool {
        self.pending.values().any(|r| *r == reply)
    }

    /// Drops the in-flight entries a session switch invalidates: the pane-restore
    /// buffers and the enumeration ids `send_session_enumeration` re-issues. A
    /// `HashMap` keyed by `CommandId` does not get the old `Option` fields' free
    /// last-write-wins overwrite, so a stale pre-switch `list-windows`/active-pane
    /// reply would otherwise mis-seed the new session.
    pub(crate) fn clear_for_session_switch(&mut self) {
        self.pending.retain(|_, r| {
            !matches!(
                r,
                PendingReply::ListWindows
                    | PendingReply::ActivePane
                    | PendingReply::AggressiveResize
                    | PendingReply::RestoreBase { .. }
                    | PendingReply::RestoreSavedPrimary { .. }
                    | PendingReply::RestoreState { .. }
                    | PendingReply::RestorePending { .. }
            )
        });
        self.aggressive_resize_checked = false;
        self.restores.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state_restore::parse_pane_state;

    #[test]
    fn format_is_tab_separated_with_name_last() {
        assert!(LIST_WINDOWS_FORMAT.contains('\t'));
        assert!(LIST_WINDOWS_FORMAT.ends_with("#{window_name}"));
    }

    #[test]
    fn parses_one_active_window() {
        let lines = vec!["1\t@1\t0\tb25f,80x24,0,0,0\tb25f,80x24,0,0,0\t\tmain".to_string()];
        let rows = parse_window_rows(&lines).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, WindowId(1));
        assert!(rows[0].active);
        assert_eq!(rows[0].name, "main");
        assert_eq!(rows[0].layout.root.dims().width, 80);
    }

    #[test]
    fn parses_multiple_windows_active_flag() {
        let lines = vec![
            "0\t@1\t0\tb25f,80x24,0,0,0\tb25f,80x24,0,0,0\t\tone".to_string(),
            "1\t@2\t1\tb25f,80x24,0,0,1\tb25f,80x24,0,0,1\t*\ttwo".to_string(),
        ];
        let rows = parse_window_rows(&lines).unwrap();
        assert_eq!((rows[0].active, rows[1].active), (false, true));
        assert_eq!((rows[0].id, rows[1].id), (WindowId(1), WindowId(2)));
    }

    #[test]
    fn name_with_tabs_is_preserved_as_last_field() {
        let lines =
            vec!["1\t@1\t0\tb25f,80x24,0,0,0\tb25f,80x24,0,0,0\t\tmy\tnamed\twin".to_string()];
        let rows = parse_window_rows(&lines).unwrap();
        assert_eq!(rows[0].name, "my\tnamed\twin");
    }

    #[test]
    fn bad_window_id_errors() {
        let lines = vec!["1\t1\t0\tb25f,80x24,0,0,0\tb25f,80x24,0,0,0\t\tx".to_string()];
        assert!(parse_window_rows(&lines).is_err());
    }

    #[test]
    fn empty_input_is_empty() {
        assert_eq!(parse_window_rows(&[]).unwrap(), vec![]);
    }

    #[test]
    fn parse_row_captures_window_index() {
        // Format order: active \t id \t index \t layout \t visible \t raw_flags \t name
        let line = "1\t@2\t3\tb25d,80x24,0,0,0\tb25d,80x24,0,0,0\t\tmy-win";
        let rows = parse_window_rows(&[line.to_string()]).expect("parse");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].index, 3);
        assert_eq!(rows[0].name, "my-win");
        assert!(rows[0].active);
    }

    #[test]
    fn parse_row_reads_raw_flags_before_name() {
        // active, id, index, layout, visible_layout, raw_flags, name
        let line = "1\t@2\t0\tb25f,80x24,0,0,1\tb25f,80x24,0,0,1\t*Z\tmy editor".to_string();
        let rows = parse_window_rows(&[line]).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, WindowId(2));
        assert!(rows[0].active);
        assert_eq!(rows[0].name, "my editor");
        assert_eq!(rows[0].flags, WindowFlags::ZOOM);
    }

    #[test]
    fn parse_row_prefers_visible_layout_when_present() {
        // field order: active id index window_layout window_visible_layout flags name
        // window_layout = 80x24, visible_layout = 40x12; must adopt visible dims.
        // The parser is lenient about checksum mismatches — 0000 is accepted for both.
        let row = "1\t@1\t0\t0000,80x24,0,0,1\t0000,40x12,0,0,1\t*\tbash";
        let parsed = parse_window_rows(&[row.to_string()]).expect("row parses");
        let dims = parsed[0].layout.root.dims();
        assert_eq!(
            (dims.width, dims.height),
            (40, 12),
            "must use visible_layout"
        );
    }

    #[test]
    fn parse_row_falls_back_to_window_layout_when_visible_empty() {
        // visible_layout field is empty — must fall back to window_layout (80x24).
        let row = "1\t@1\t0\t0000,80x24,0,0,1\t\t*\tbash";
        let parsed = parse_window_rows(&[row.to_string()]).expect("row parses");
        let dims = parsed[0].layout.root.dims();
        assert_eq!(
            (dims.width, dims.height),
            (80, 24),
            "fallback to window_layout"
        );
    }

    #[test]
    fn version_supports_per_window_refresh_is_lenient_about_suffixes() {
        assert!(version_supports_per_window_refresh("3.6a"));
        assert!(version_supports_per_window_refresh("3.4"));
        assert!(version_supports_per_window_refresh("next-3.7"));
        assert!(!version_supports_per_window_refresh("3.3"));
        assert!(!version_supports_per_window_refresh("2.9"));
        assert!(!version_supports_per_window_refresh("garbage"));
    }

    #[test]
    fn clear_for_session_switch_drops_enumeration_but_keeps_client_name() {
        let mut state = EnumerationState::default();
        state
            .pending
            .insert(CommandId(1), PendingReply::ListWindows);
        state.pending.insert(CommandId(2), PendingReply::ActivePane);
        state.pending.insert(CommandId(3), PendingReply::ClientName);
        state
            .pending
            .insert(CommandId(4), PendingReply::RestoreBase { pane: PaneId(7) });
        state
            .pending
            .insert(CommandId(5), PendingReply::AggressiveResize);
        state.aggressive_resize_checked = true;
        state.clear_for_session_switch();
        assert_eq!(
            state.pending.get(&CommandId(3)),
            Some(&PendingReply::ClientName),
            "client-name entry must survive"
        );
        assert!(
            !state.pending.contains_key(&CommandId(1)),
            "stale list-windows dropped"
        );
        assert!(
            !state.pending.contains_key(&CommandId(2)),
            "stale active-pane dropped"
        );
        assert!(
            !state.pending.contains_key(&CommandId(4)),
            "restore dropped"
        );
        assert!(
            !state.pending.contains_key(&CommandId(5)),
            "stale aggressive-resize dropped so new session is re-checked"
        );
        assert!(!state.aggressive_resize_checked, "aggressive guard reset");
    }

    #[test]
    fn full_restore_completes_only_when_all_four_slots_resolve() {
        let mut r = PaneRestore::new_full(4);
        assert!(!r.complete());
        r.base = Slot::Ok(vec!["a".into()]);
        r.saved_primary = Slot::Ok(vec![]);
        r.state = Slot::Ok(parse_pane_state(""));
        assert!(!r.complete());
        r.pending = Slot::Failed;
        assert!(r.complete());
    }

    #[test]
    fn light_restore_needs_only_base_and_state() {
        let mut r = PaneRestore::new_light(4);
        r.base = Slot::Ok(vec!["a".into()]);
        assert!(!r.complete());
        r.state = Slot::Ok(parse_pane_state(""));
        assert!(r.complete());
    }

    #[test]
    fn into_bytes_survives_partial_failure() {
        let mut r = PaneRestore::new_full(2);
        r.base = Slot::Ok(vec!["row".into()]);
        r.saved_primary = Slot::Failed;
        r.state = Slot::Failed;
        r.pending = Slot::Failed;
        let bytes = r.into_bytes();
        assert!(String::from_utf8_lossy(&bytes).contains("row"));
    }

    #[test]
    fn clear_for_session_switch_drops_restore_buffers_and_pending_kinds() {
        let mut state = EnumerationState::default();
        state.restores.insert(PaneId(1), PaneRestore::new_full(4));
        state
            .pending
            .insert(CommandId(9), PendingReply::RestoreBase { pane: PaneId(1) });
        state.clear_for_session_switch();
        assert!(state.restores.is_empty());
        assert!(state.pending.is_empty());
    }

    #[test]
    fn register_supersedes_in_flight_singleton_but_keeps_concurrent_kinds() {
        let mut state = EnumerationState::default();
        state.register(Ok(CommandId(1)), PendingReply::ListWindows);
        state.register(Ok(CommandId(2)), PendingReply::ListWindows);
        assert!(
            !state.pending.contains_key(&CommandId(1)),
            "the superseded list-windows id is dropped (old Option last-write-wins)"
        );
        assert_eq!(
            state.pending.get(&CommandId(2)),
            Some(&PendingReply::ListWindows),
            "only the latest list-windows id remains, so trigger_seed fires once"
        );

        state.register(Ok(CommandId(3)), PendingReply::ActivePane);
        state.register(Ok(CommandId(4)), PendingReply::ActivePane);
        assert!(
            !state.pending.contains_key(&CommandId(3)),
            "stale active-pane dropped"
        );
        assert_eq!(
            state.pending.get(&CommandId(4)),
            Some(&PendingReply::ActivePane)
        );

        state.register(
            Ok(CommandId(5)),
            PendingReply::RestoreBase { pane: PaneId(3) },
        );
        state.register(
            Ok(CommandId(6)),
            PendingReply::RestoreBase { pane: PaneId(3) },
        );
        assert_eq!(
            state.pending.get(&CommandId(5)),
            Some(&PendingReply::RestoreBase { pane: PaneId(3) }),
            "two concurrent identical-value restores for the same pane are both kept, not superseded"
        );
        assert_eq!(
            state.pending.get(&CommandId(6)),
            Some(&PendingReply::RestoreBase { pane: PaneId(3) })
        );

        state.register(
            Ok(CommandId(7)),
            PendingReply::RestoreBase { pane: PaneId(1) },
        );
        state.register(
            Ok(CommandId(8)),
            PendingReply::RestoreBase { pane: PaneId(2) },
        );
        assert_eq!(
            state.pending.get(&CommandId(7)),
            Some(&PendingReply::RestoreBase { pane: PaneId(1) }),
            "per-pane restores are independent — both kept"
        );
        assert_eq!(
            state.pending.get(&CommandId(8)),
            Some(&PendingReply::RestoreBase { pane: PaneId(2) })
        );
    }
}
