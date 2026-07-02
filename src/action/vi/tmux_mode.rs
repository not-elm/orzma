//! tmux-mode VI applier: translates the shared VI action events into targeted
//! `send-keys -X` commands on the pane's tmux-side copy mode, opens the
//! search/jump prompt, bridges yanks to the clipboard, and clears
//! `CopyModeState` on exit. Guarded on `TmuxPane`.

use crate::action::vi::{
    ViExitRequest, ViMotionRequest, ViPromptRequest, ViScrollKind, ViScrollRequest,
    ViSearchStepRequest, ViSelectionToggleRequest, ViYankRequest,
};
use crate::mode::tmux::copy_mode::CopyModeSnapshot;
use crate::ui::copy_mode::CopyModeState;
use crate::ui::copy_search::{CopyPrompt, CopyPromptState};
use bevy::prelude::*;
use ozma_tty_engine::{SelectionType, ViMotion};
use ozmux_tmux::{
    CopyModeQueries, CopyQueryKind, PaneId, ShowBuffer, TmuxClient, TmuxCommand, TmuxPane,
};

/// Registers the tmux-mode VI apply observers.
pub(super) struct TmuxModeViPlugin;

impl Plugin for TmuxModeViPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_vi_motion)
            .add_observer(on_vi_scroll)
            .add_observer(on_vi_selection_toggle)
            .add_observer(on_vi_yank)
            .add_observer(on_vi_exit)
            .add_observer(on_vi_prompt)
            .add_observer(on_vi_search_step);
    }
}

/// `send-keys -X -t %<id> <command>` — one copy-mode command on a pane.
struct CopyModeX<'a> {
    pane: PaneId,
    command: &'a str,
}
impl TmuxCommand for CopyModeX<'_> {
    fn into_raw_command(self) -> String {
        format!("send-keys -X -t %{} {}", self.pane.0, self.command)
    }
}

/// The tmux `-X` command for a motion (spec §3 table). `None` for `ViMotion`
/// variants the keymap never produces (e.g. `SemanticLeftEnd`) — the match
/// must stay exhaustive over the engine enum.
fn motion_x(motion: ViMotion) -> Option<&'static str> {
    Some(match motion {
        ViMotion::Left => "cursor-left",
        ViMotion::Down => "cursor-down",
        ViMotion::Up => "cursor-up",
        ViMotion::Right => "cursor-right",
        ViMotion::First => "start-of-line",
        ViMotion::Last => "end-of-line",
        ViMotion::FirstOccupied => "back-to-indentation",
        ViMotion::SemanticRight => "next-word",
        ViMotion::SemanticLeft => "previous-word",
        ViMotion::SemanticRightEnd => "next-word-end",
        ViMotion::WordRight => "next-space",
        ViMotion::WordLeft => "previous-space",
        ViMotion::WordRightEnd => "next-space-end",
        ViMotion::High => "top-line",
        ViMotion::Middle => "middle-line",
        ViMotion::Low => "bottom-line",
        ViMotion::ParagraphUp => "previous-paragraph",
        ViMotion::ParagraphDown => "next-paragraph",
        ViMotion::Bracket => "next-matching-bracket",
        _ => return None,
    })
}

/// The tmux `-X` command for a scroll (spec §3 table).
fn scroll_x(kind: ViScrollKind) -> &'static str {
    match kind {
        ViScrollKind::PageUp => "page-up",
        ViScrollKind::PageDown => "page-down",
        ViScrollKind::HalfUp => "halfpage-up",
        ViScrollKind::HalfDown => "halfpage-down",
        ViScrollKind::LineUp => "scroll-up",
        ViScrollKind::LineDown => "scroll-down",
        ViScrollKind::Top => "history-top",
        ViScrollKind::Bottom => "history-bottom",
    }
}

fn send_x(client: &mut TmuxClient, pane: PaneId, command: &str) -> bool {
    match client.send(CopyModeX { pane, command }) {
        Ok(_) => true,
        Err(e) => {
            tracing::warn!(?e, command, "copy-mode -X send failed");
            false
        }
    }
}

fn selection_present(snapshots: &Query<&CopyModeSnapshot>, entity: Entity) -> bool {
    snapshots
        .get(entity)
        .map(|s| s.0.selection_present)
        .unwrap_or(false)
}

fn on_vi_motion(
    ev: On<ViMotionRequest>,
    mut client: Option<Single<&mut TmuxClient>>,
    panes: Query<&TmuxPane>,
) {
    let Ok(pane) = panes.get(ev.entity) else {
        return;
    };
    let Some(client) = client.as_deref_mut() else {
        return;
    };
    if let Some(command) = motion_x(ev.motion) {
        send_x(client, pane.id, command);
    }
}

fn on_vi_scroll(
    ev: On<ViScrollRequest>,
    mut client: Option<Single<&mut TmuxClient>>,
    panes: Query<&TmuxPane>,
) {
    let Ok(pane) = panes.get(ev.entity) else {
        return;
    };
    let Some(client) = client.as_deref_mut() else {
        return;
    };
    send_x(client, pane.id, scroll_x(ev.kind));
}

/// NOTE: the snapshot is refreshed by polling, so it is ~1 frame stale; a
/// same-frame begin-then-toggle sees the old state. Selection KIND is not in
/// the snapshot, so a kind change degrades to clear (next press starts the
/// new kind) — an accepted approximation (spec §5).
fn on_vi_selection_toggle(
    ev: On<ViSelectionToggleRequest>,
    mut client: Option<Single<&mut TmuxClient>>,
    panes: Query<&TmuxPane>,
    snapshots: Query<&CopyModeSnapshot>,
) {
    let Ok(pane) = panes.get(ev.entity) else {
        return;
    };
    let Some(client) = client.as_deref_mut() else {
        return;
    };
    if selection_present(&snapshots, ev.entity) {
        send_x(client, pane.id, "clear-selection");
        return;
    }
    match ev.ty {
        SelectionType::Lines => {
            send_x(client, pane.id, "select-line");
        }
        SelectionType::Block => {
            if send_x(client, pane.id, "begin-selection") {
                send_x(client, pane.id, "rectangle-toggle");
            }
        }
        _ => {
            send_x(client, pane.id, "begin-selection");
        }
    }
}

fn on_vi_yank(
    ev: On<ViYankRequest>,
    mut commands: Commands,
    mut copy_queries: ResMut<CopyModeQueries>,
    mut client: Option<Single<&mut TmuxClient>>,
    panes: Query<&TmuxPane>,
    snapshots: Query<&CopyModeSnapshot>,
) {
    let Ok(pane) = panes.get(ev.entity) else {
        return;
    };
    // NOTE: with no selection tmux creates no buffer and `show-buffer` would
    // bridge a STALE buffer into the clipboard — skip entirely (spec §5).
    if !selection_present(&snapshots, ev.entity) {
        return;
    }
    let Some(client) = client.as_deref_mut() else {
        return;
    };
    if !send_x(client, pane.id, "copy-selection-and-cancel") {
        return;
    }
    match client.send(ShowBuffer) {
        Ok(buf_id) => copy_queries.register(buf_id, pane.id, CopyQueryKind::Buffer),
        Err(e) => tracing::warn!(?e, "show-buffer send failed"),
    }
    commands.entity(ev.entity).remove::<CopyModeState>();
}

fn on_vi_exit(
    ev: On<ViExitRequest>,
    mut commands: Commands,
    mut client: Option<Single<&mut TmuxClient>>,
    panes: Query<&TmuxPane>,
) {
    let Ok(pane) = panes.get(ev.entity) else {
        return;
    };
    let Some(client) = client.as_deref_mut() else {
        return;
    };
    if send_x(client, pane.id, "cancel") {
        commands.entity(ev.entity).remove::<CopyModeState>();
    }
}

fn on_vi_prompt(
    ev: On<ViPromptRequest>,
    mut copy_prompt: ResMut<CopyPrompt>,
    panes: Query<&TmuxPane>,
) {
    let Ok(pane) = panes.get(ev.entity) else {
        return;
    };
    copy_prompt.open = Some(CopyPromptState {
        kind: ev.kind,
        pane: pane.id,
        text: String::new(),
    });
}

fn on_vi_search_step(
    ev: On<ViSearchStepRequest>,
    mut client: Option<Single<&mut TmuxClient>>,
    panes: Query<&TmuxPane>,
) {
    let Ok(pane) = panes.get(ev.entity) else {
        return;
    };
    let Some(client) = client.as_deref_mut() else {
        return;
    };
    let command = if ev.forward {
        "search-again"
    } else {
        "search-reverse"
    };
    send_x(client, pane.id, command);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mode::tmux::copy_mode::CopyModeSnapshot;
    use ozmux_tmux::CopyState;
    use tmux_control_parser::CellDims;

    fn snapshot(selection_present: bool) -> CopyModeSnapshot {
        CopyModeSnapshot(CopyState {
            pane_in_mode: true,
            scroll_position: 0,
            pane_height: 5,
            history_size: 0,
            cursor_x: 0,
            cursor_y: 0,
            selection_present,
            rectangle: false,
            sel_start_x: 0,
            sel_start_y: 0,
            sel_end_x: 0,
            sel_end_y: 0,
        })
    }

    fn app_with(observer_registrar: fn(&mut App)) -> (App, Entity, Entity) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<CopyModeQueries>();
        observer_registrar(&mut app);
        let client = app.world_mut().spawn(TmuxClient::new_adopted()).id();
        let pane = app
            .world_mut()
            .spawn(TmuxPane {
                id: PaneId(7),
                dims: CellDims {
                    width: 10,
                    height: 5,
                    xoff: 0,
                    yoff: 0,
                },
            })
            .id();
        (app, client, pane)
    }

    fn outgoing(app: &mut App, client: Entity) -> String {
        let mut c = app.world_mut().get_mut::<TmuxClient>(client).unwrap();
        String::from_utf8(c.take_outgoing()).unwrap()
    }

    #[test]
    fn copy_mode_x_renders_targeted_command() {
        assert_eq!(
            CopyModeX {
                pane: PaneId(3),
                command: "cursor-left"
            }
            .into_raw_command(),
            "send-keys -X -t %3 cursor-left"
        );
    }

    #[test]
    fn motion_sends_x_command() {
        let (mut app, client, pane) = app_with(|a| {
            a.add_observer(on_vi_motion);
        });
        app.world_mut().trigger(ViMotionRequest {
            entity: pane,
            motion: ViMotion::Bracket,
        });
        app.update();
        assert!(outgoing(&mut app, client).contains("send-keys -X -t %7 next-matching-bracket"));
    }

    #[test]
    fn scroll_sends_x_command() {
        let (mut app, client, pane) = app_with(|a| {
            a.add_observer(on_vi_scroll);
        });
        app.world_mut().trigger(ViScrollRequest {
            entity: pane,
            kind: ViScrollKind::HalfDown,
        });
        app.update();
        assert!(outgoing(&mut app, client).contains("send-keys -X -t %7 halfpage-down"));
    }

    #[test]
    fn selection_toggle_clears_when_selection_present() {
        let (mut app, client, pane) = app_with(|a| {
            a.add_observer(on_vi_selection_toggle);
        });
        app.world_mut().entity_mut(pane).insert(snapshot(true));
        app.world_mut().trigger(ViSelectionToggleRequest {
            entity: pane,
            ty: SelectionType::Simple,
        });
        app.update();
        let out = outgoing(&mut app, client);
        assert!(out.contains("clear-selection"), "got {out:?}");
        assert!(!out.contains("begin-selection"));
    }

    #[test]
    fn rect_selection_begins_then_toggles_rectangle() {
        let (mut app, client, pane) = app_with(|a| {
            a.add_observer(on_vi_selection_toggle);
        });
        app.world_mut().trigger(ViSelectionToggleRequest {
            entity: pane,
            ty: SelectionType::Block,
        });
        app.update();
        let out = outgoing(&mut app, client);
        let begin = out.find("begin-selection").expect("begin-selection sent");
        let rect = out.find("rectangle-toggle").expect("rectangle-toggle sent");
        assert!(
            begin < rect,
            "begin-selection must precede rectangle-toggle"
        );
    }

    #[test]
    fn yank_without_selection_sends_nothing() {
        let (mut app, client, pane) = app_with(|a| {
            a.add_observer(on_vi_yank);
        });
        app.world_mut()
            .entity_mut(pane)
            .insert((snapshot(false), CopyModeState));
        app.world_mut().trigger(ViYankRequest { entity: pane });
        app.update();
        assert!(outgoing(&mut app, client).is_empty());
        assert!(app.world().entity(pane).contains::<CopyModeState>());
    }

    #[test]
    fn yank_with_selection_copies_bridges_and_unmarks() {
        let (mut app, client, pane) = app_with(|a| {
            a.add_observer(on_vi_yank);
        });
        app.world_mut()
            .entity_mut(pane)
            .insert((snapshot(true), CopyModeState));
        app.world_mut().trigger(ViYankRequest { entity: pane });
        app.update();
        let out = outgoing(&mut app, client);
        assert!(out.contains("copy-selection-and-cancel"), "got {out:?}");
        assert!(out.contains("show-buffer"), "got {out:?}");
        assert!(!app.world().entity(pane).contains::<CopyModeState>());
    }

    #[test]
    fn exit_cancels_and_unmarks() {
        let (mut app, client, pane) = app_with(|a| {
            a.add_observer(on_vi_exit);
        });
        app.world_mut().entity_mut(pane).insert(CopyModeState);
        app.world_mut().trigger(ViExitRequest { entity: pane });
        app.update();
        assert!(outgoing(&mut app, client).contains("send-keys -X -t %7 cancel"));
        assert!(!app.world().entity(pane).contains::<CopyModeState>());
    }

    #[test]
    fn prompt_opens_copy_prompt() {
        let (mut app, _client, pane) = app_with(|a| {
            a.init_resource::<CopyPrompt>();
            a.add_observer(on_vi_prompt);
        });
        app.world_mut().trigger(ViPromptRequest {
            entity: pane,
            kind: ozmux_tmux::PromptKind::SearchForward,
        });
        app.update();
        let prompt = app.world().resource::<CopyPrompt>();
        assert!(prompt.open.is_some());
    }

    #[test]
    fn search_step_sends_again_or_reverse() {
        let (mut app, client, pane) = app_with(|a| {
            a.add_observer(on_vi_search_step);
        });
        app.world_mut().trigger(ViSearchStepRequest {
            entity: pane,
            forward: true,
        });
        app.world_mut().trigger(ViSearchStepRequest {
            entity: pane,
            forward: false,
        });
        app.update();
        let out = outgoing(&mut app, client);
        assert!(out.contains("search-again"));
        assert!(out.contains("search-reverse"));
    }
}
